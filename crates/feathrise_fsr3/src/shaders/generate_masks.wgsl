// FSR 3 应用侧自动遮罩：比较透明合成前后颜色，并用 Bevy 运动矢量重投影上一帧。
// 存储格式占位符在共享管线首次创建时替换为 R8Unorm；不支持单通道存储写入的设备回退 RGBA8 红通道。

@group(0) @binding(0) var current_opaque: texture_2d<f32>;
@group(0) @binding(1) var current_composed: texture_2d<f32>;
@group(0) @binding(2) var previous_opaque: texture_2d<f32>;
@group(0) @binding(3) var previous_composed: texture_2d<f32>;
@group(0) @binding(4) var motion_vectors: texture_2d<f32>;
@group(0) @binding(5) var reactive_output: texture_storage_2d<__MASK_FORMAT__, write>;
@group(0) @binding(6) var composition_output: texture_storage_2d<__MASK_FORMAT__, write>;

// AMD 自动 T&C 参考算法使用 0.01 排除数值噪声；这里保留同一边界。
const COMPOSITION_EPSILON: f32 = 0.01;
// AMD 建议从 0.1 开始判断跨帧显著变化，避免相机抖动把静态边缘铺满遮罩。
const TEMPORAL_CHANGE_THRESHOLD: f32 = 0.1;
// 自动反应遮罩把可信信号放大五倍，并按 AMD 建议把最大值限制在 0.9。
const REACTIVE_SCALE: f32 = 5.0;
const REACTIVE_MAX: f32 = 0.9;

// 把预曝光 HDR 压到有界域后再比较，避免一个极亮像素把大范围遮罩推满。
fn bounded_hdr(color: vec3<f32>) -> vec3<f32> {
    let positive = max(color, vec3<f32>(0.0));
    return positive / (vec3<f32>(1.0) + positive);
}

// 所有邻域和重投影读取都钳制到有效内部尺寸，窗口边缘不会采到未定义纹素。
fn clamp_pixel(pixel: vec2<i32>, size: vec2<i32>) -> vec2<i32> {
    return clamp(pixel, vec2<i32>(0), size - vec2<i32>(1));
}

fn load_bounded(texture: texture_2d<f32>, pixel: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    return bounded_hdr(textureLoad(texture, clamp_pixel(pixel, size), 0).rgb);
}

fn max_component(value: vec3<f32>) -> f32 {
    return max(value.x, max(value.y, value.z));
}

// 透明贡献使用“合成后 - 不透明底图”，因此底图本身的高频边缘不会直接污染反应遮罩。
fn alpha_contribution(pixel: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    let opaque = load_bounded(current_opaque, pixel, size);
    let composed = load_bounded(current_composed, pixel, size);
    return abs(composed - opaque);
}

@compute @workgroup_size(8, 8, 1)
fn generate_fsr3_masks(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let unsigned_size = textureDimensions(current_opaque);
    if (any(invocation.xy >= unsigned_size)) {
        return;
    }

    let size = vec2<i32>(unsigned_size);
    let pixel = vec2<i32>(invocation.xy);
    // Bevy 保存 currentUV - previousUV；减去归一化矢量乘内部尺寸即可得到上一帧像素。
    let motion = clamp(textureLoad(motion_vectors, pixel, 0).xy, vec2<f32>(-1.0), vec2<f32>(1.0));
    let previous_pixel = clamp_pixel(
        vec2<i32>(round(vec2<f32>(pixel) - motion * vec2<f32>(unsigned_size))),
        size,
    );

    let opaque_now = load_bounded(current_opaque, pixel, size);
    let composed_now = load_bounded(current_composed, pixel, size);
    let opaque_before = load_bounded(previous_opaque, previous_pixel, size);
    let composed_before = load_bounded(previous_composed, previous_pixel, size);
    let alpha_now = abs(composed_now - opaque_now);
    let alpha_before = abs(composed_before - opaque_before);

    let alpha_presence = max_component(alpha_now);
    let alpha_temporal_change = max_component(abs(alpha_now - alpha_before));
    // 正确运动矢量会抵消刚体运动；剩余显著变化覆盖程序化植被、动画纹理和突变光照。
    let opaque_temporal_change = max_component(abs(opaque_now - opaque_before));
    let composition_changed = alpha_temporal_change >= TEMPORAL_CHANGE_THRESHOLD;
    let opaque_changed_without_reliable_history = opaque_temporal_change >= TEMPORAL_CHANGE_THRESHOLD;
    let has_composition = alpha_presence >= COMPOSITION_EPSILON;
    let composition_mask = select(
        0.0,
        1.0,
        has_composition || composition_changed || opaque_changed_without_reliable_history,
    );

    // 四邻域透明贡献梯度覆盖粒子、头发、水面和透明边缘，跨帧变化覆盖其内部快速动画。
    let alpha_left = alpha_contribution(pixel + vec2<i32>(-1, 0), size);
    let alpha_right = alpha_contribution(pixel + vec2<i32>(1, 0), size);
    let alpha_up = alpha_contribution(pixel + vec2<i32>(0, -1), size);
    let alpha_down = alpha_contribution(pixel + vec2<i32>(0, 1), size);
    let alpha_spatial_change = max(
        max_component(abs(alpha_now - alpha_left)),
        max(
            max_component(abs(alpha_now - alpha_right)),
            max(
                max_component(abs(alpha_now - alpha_up)),
                max_component(abs(alpha_now - alpha_down)),
            ),
        ),
    );
    let reactive_signal = max(alpha_temporal_change, alpha_spatial_change);
    let reactive_mask = min(clamp(reactive_signal * REACTIVE_SCALE, 0.0, 1.0), REACTIVE_MAX);

    textureStore(reactive_output, pixel, vec4<f32>(reactive_mask, 0.0, 0.0, 1.0));
    textureStore(composition_output, pixel, vec4<f32>(composition_mask, 0.0, 0.0, 1.0));
}
