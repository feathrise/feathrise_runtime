
// 单帧固定三个vec4；字段顺序与Rust `TreeWindGpuFrame` 完全一致。
struct TreeWindFrame {
    direction_strength_noise: vec4<f32>,
    motion: vec4<f32>,
    secondary: vec4<f32>,
}

// 两帧参数与树高边界固定组成八个vec4，即128字节。
struct TreeWindUniform {
    current: TreeWindFrame,
    previous: TreeWindFrame,
    aabb_min: vec4<f32>,
    aabb_max: vec4<f32>,
}

// 只采用非bindless材质路径，十份共享材质固定使用100/101/102。
@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<uniform> tree_wind_uniform: TreeWindUniform;
@group(constants::MATERIAL_BIND_GROUP) @binding(101)
var tree_wind_noise_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102)
var tree_wind_noise_sampler: sampler;

const TREE_WIND_TAU: f32 = 6.283185307179586;

// 异常或默认零方向安全回退世界X，禁止normalize(0)污染顶点。
fn safe_tree_wind_direction(direction: vec2<f32>) -> vec2<f32> {
    let length_squared = dot(direction, direction);
    if length_squared > 0.000001 {
        return direction * inverseSqrt(length_squared);
    }
    return vec2<f32>(1.0, 0.0);
}

// 用实例旋转后的局部X/Z轴把世界风向还原到当前树的局部平面。
fn tree_wind_direction_world_to_local(
    direction_xz: vec2<f32>,
    world_from_local: mat4x4<f32>,
) -> vec3<f32> {
    let world_direction = vec3<f32>(direction_xz.x, 0.0, direction_xz.y);
    let world_axis_x = normalize(world_from_local[0].xyz);
    let world_axis_z = normalize(world_from_local[2].xyz);
    let local_planar = vec2<f32>(
        dot(world_direction, world_axis_x),
        dot(world_direction, world_axis_z),
    );
    let safe_planar = safe_tree_wind_direction(local_planar);
    return vec3<f32>(safe_planar.x, 0.0, safe_planar.y);
}

// 主渲染、阴影、深度、延迟和运动向量唯一共享的局部顶点位移核心。
fn displace_tree_wind_local(
    frame: TreeWindFrame,
    local_position: vec3<f32>,
    world_from_local: mat4x4<f32>,
    time: f32,
    aabb_min_y: f32,
    aabb_max_y: f32,
) -> vec3<f32> {
    // 所有位移幅度严格为零时直接返回原顶点，保证F1零风无采样误差。
    if frame.direction_strength_noise.z == 0.0 &&
        frame.motion.y == 0.0 &&
        frame.motion.w == 0.0 &&
        frame.secondary.z == 0.0 {
        return local_position;
    }

    // 地表及其下方根须权重钳为零，树梢权重钳为一。
    let tree_height = max(aabb_max_y - aabb_min_y, 0.001);
    let height_weight = clamp((local_position.y - aabb_min_y) / tree_height, 0.0, 1.0);
    if height_weight == 0.0 {
        return local_position;
    }

    let world_direction = safe_tree_wind_direction(frame.direction_strength_noise.xy);
    let sample_time = select(time, time + 3600.0, time < 0.0); // Bevy Globals一小时回绕时，上一帧运动向量仍取回绕前的真实时间。
    let local_direction = tree_wind_direction_world_to_local(world_direction, world_from_local);
    let local_right = vec3<f32>(-local_direction.z, 0.0, local_direction.x);
    let base_world_position = world_from_local * vec4<f32>(local_position, 1.0);
    let noise_uv = base_world_position.xz * frame.direction_strength_noise.w + // 原有世界稳定噪声位置。
        world_direction * (sample_time * frame.motion.x + frame.secondary.w * 0.045); // 天气变速时补偿噪声滚动相位。
    let noise = textureSampleLevel(
        tree_wind_noise_texture,
        tree_wind_noise_sampler,
        noise_uv,
        0.0,
    );

    // 全部LOD共享按高度平方增长的宏观弯曲，切档时树梢轮廓保持连续。
    let macro_noise = noise.r * 2.0 - 1.0;
    let macro_weight = height_weight * height_weight;
    var displaced = local_position +
        local_direction * macro_noise * frame.direction_strength_noise.z * macro_weight;

    // 高中档叶簇额外使用微观噪声、横向S曲线和轻微上下起伏。
    @if(TREE_WIND_DETAILED) let micro_noise = noise.g * 2.0 - 1.0;
    @if(TREE_WIND_DETAILED) displaced += local_direction * micro_noise * frame.motion.y * height_weight;

    @if(TREE_WIND_DETAILED) let phase_x = noise.b * TREE_WIND_TAU;
    @if(TREE_WIND_DETAILED) let phase_y = noise.a * TREE_WIND_TAU;
    @if(TREE_WIND_DETAILED) let height_lag = height_weight * frame.secondary.x;
    @if(TREE_WIND_DETAILED) let curve_phase = sample_time * frame.motion.z + frame.secondary.w * 2.8 + phase_x + height_lag; // 与草地共享动画时间偏移。
    @if(TREE_WIND_DETAILED) let cross_phase = (sample_time * frame.motion.z + frame.secondary.w * 2.8) * 0.83 + phase_y - height_lag * 0.5; // 横摆不因变速跳相。
    @if(TREE_WIND_DETAILED) let curve_offset = local_direction * sin(curve_phase) +
        local_right * cos(cross_phase) * 0.5;
    @if(TREE_WIND_DETAILED) displaced += curve_offset * frame.motion.w * height_weight;

    @if(TREE_WIND_DETAILED) let bop_phase = sample_time * frame.secondary.y + frame.secondary.w * 2.1 + phase_y + height_lag; // 上下微动读取同一补偿。
    @if(TREE_WIND_DETAILED) displaced.y += sin(bop_phase) * frame.secondary.z * height_weight;

    return displaced;
}
