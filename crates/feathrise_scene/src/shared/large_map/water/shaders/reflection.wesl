// 透明水面SSR：只读取当前视图色深，以有限射线替换命中的天空镜面部分。

@if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) import bevy_pbr::render::mesh_view_bindings::view;
@if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) import bevy_pbr::render::mesh_view_bindings::view_transmission_texture;
@if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) import bevy_pbr::render::mesh_view_bindings::view_transmission_sampler;
@if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) import bevy_pbr::prepass::utils::prepass_depth;

struct WaterReflectionSample { // 无可信几何时严格零权重。
    radiance: vec3<f32>, // 当前场景HDR，已包含相机曝光。
    confidence: f32, // 只控制场景与天空镜面交接。
}

@if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) struct WaterReflectionProbe { // 单步投影与深度证据。
    uv: vec2<f32>, // 整张颜色附件的采样坐标。
    local_uv: vec2<f32>, // 当前viewport内部坐标，用于边缘渐隐。
    delta: f32, // 射线与已绘制几何的正向视空间深度差。
    valid: f32, // -1越界，0天空，1有效几何。
}

@if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) fn water_reflection_probe(world: vec3<f32>) -> WaterReflectionProbe { // 每步仅一次深度读取，不查询物体集合。
    let clip = view.clip_from_world * vec4<f32>(world, 1.0); // 与当前水面绘制使用相同抖动投影。
    if clip.w <= 0.0001 || clip.z <= 0.0 || clip.z > clip.w { return WaterReflectionProbe(vec2<f32>(0.0), vec2<f32>(0.0), 0.0, -1.0); } // 相机背后或近平面之外安全退出。
    let ndc = clip.xyz / clip.w; // 标准反向Z裁剪空间。
    let local_uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5); // 图像Y向下，投影Y向上。
    if any(local_uv <= vec2<f32>(0.001)) || any(local_uv >= vec2<f32>(0.999)) { return WaterReflectionProbe(vec2<f32>(0.0), local_uv, 0.0, -1.0); } // 屏外内容不能从边缘Clamp伪造。
    let size = vec2<f32>(textureDimensions(view_transmission_texture)); // 当前实际颜色附件尺寸，不读窗口尺寸。
    let pixel = view.main_pass_viewport.xy + local_uv * view.main_pass_viewport.zw; // 分屏偏移与SR内部渲染尺寸一致，不误用最终输出viewport。
    if any(pixel < vec2<f32>(0.0)) || any(pixel >= size) { return WaterReflectionProbe(vec2<f32>(0.0), local_uv, 0.0, -1.0); } // 目标尚未同步尺寸时拒绝旧图。
    let uv = pixel / size; // 深度与颜色使用同一实际像素。
    let depth = prepass_depth(vec4<f32>(pixel, 0.0, 1.0), 0u); // MSAA通过Bevy标准函数取第零样本，边缘由可信度渐隐。
    if depth <= 0.0000001 { return WaterReflectionProbe(uv, local_uv, 0.0, 0.0); } // 无几何的天空由原IBL表示。
    let scene_h = view.view_from_clip * vec4<f32>(ndc.xy, depth, 1.0); // 将反向Z转换为真实视空间距离。
    if abs(scene_h.w) <= 0.0000001 { return WaterReflectionProbe(uv, local_uv, 0.0, 0.0); } // 非法投影不产生无限厚度。
    let scene_depth = -scene_h.z / scene_h.w; // Bevy前方沿负Z。
    let ray_depth = -(view.view_from_world * vec4<f32>(world, 1.0)).z; // 同一视空间下比较线性距离。
    return WaterReflectionProbe(uv, local_uv, ray_depth - scene_depth, 1.0); // 正值表示射线已穿过可见几何。
}

fn sample_water_reflection(world: vec3<f32>, normal: vec3<f32>, roughness: f32, quality: f32) -> WaterReflectionSample { // 仅消费水面最终风浪法线。
    let miss = WaterReflectionSample(vec3<f32>(0.0), 0.0); // 未命中精确保留既有PBR天空镜面。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) if quality < 0.5 || roughness >= 0.65 || view.clip_from_view[3][3] == 1.0 { return miss; } // 低档、粗糙水面和正交测绘关闭追踪。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) if any(textureDimensions(view_transmission_texture) <= vec2<u32>(1u)) { return miss; } // 缺少逐视图捕获时使用Bevy一像素回退。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) let view_vector = view.world_position - world; // 反射方向由当前绘制视图决定。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) if dot(view_vector, view_vector) < 0.000001 { return miss; } // 相机正处表面时避免零向量。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) let n = normalize(normal); // 最终微法线可能包含雨纹与玩家波。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) let v = normalize(view_vector); // 水面到相机方向。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) if dot(n, v) <= 0.01 { return miss; } // 背面不产生正面镜面倒影。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) let direction = reflect(-v, n); // 相机入射方向围绕真实波面反射。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) let origin = world + n * 0.035 + direction * 0.08; // 厘米级偏移减少同表面自交。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) var steps = 12u; @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) var maximum = 24.0; // 中档固定预算。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) if quality >= 1.5 { steps = 24u; maximum = 48.0; } // 高档增加搜索范围与步数。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) if quality >= 2.5 { steps = 40u; maximum = 72.0; } // 极高仍有明确硬上限。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) var previous_t = 0.0; // 二分区间从射线偏移后的起点开始。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) var previous_delta = -0.001; // 只接纳从几何前方到后方的穿越。
    @if(VIEW_TRANSMISSION_TEXTURE && DEPTH_PREPASS && !TONEMAP_IN_SHADER) for (var index = 1u; index <= steps; index += 1u) { // 每水片元最多40次深度读取。
        let fraction = f32(index) / f32(steps); // 固定预算内分配采样位置。
        let t = maximum * fraction * fraction; // 近处更密，优先保留角色和岸边建筑。
        let probe = water_reflection_probe(origin + direction * t); // 当前同帧色深证据。
        if probe.valid < 0.0 { return miss; } // 投影离屏后无法恢复不可见内容。
        if probe.valid > 0.5 && probe.delta >= 0.0 && previous_delta < 0.0 { // 找到有限前后穿越区间。
            var low = previous_t; var high = t; // 仅本区间执行固定四次细化。
            var hit = probe; // 保存高端实际几何证据。
            for (var refine = 0u; refine < 4u; refine += 1u) { // 二分最多增加四次深度读取。
                let middle = (low + high) * 0.5; // 不依赖帧历史或随机抖动。
                let candidate = water_reflection_probe(origin + direction * middle); // 使用完全相同投影和采样规则。
                if candidate.valid > 0.5 && candidate.delta >= 0.0 { high = middle; hit = candidate; } else { low = middle; } // 天空/几何前方保持低端。
            }
            let thickness = 0.12 + high * 0.015; // 有界厚度容差，不将远处深度断层当实体体积。
            if hit.delta > thickness { return miss; } // 薄物体跳跃和错误前景穿越回退天空。
            let edge = min(min(hit.local_uv.x, hit.local_uv.y), min(1.0 - hit.local_uv.x, 1.0 - hit.local_uv.y)); // 四边取最短距离。
            let confidence = smoothstep(0.01, 0.08, edge) * (1.0 - smoothstep(maximum * 0.75, maximum, high)) * (1.0 - smoothstep(0.25, 0.65, roughness)) * (1.0 - smoothstep(thickness * 0.5, thickness, hit.delta)); // 屏边、远端、粗糙和不可信深度统一渐隐。
            let radiance = textureSampleLevel(view_transmission_texture, view_transmission_sampler, hit.uv, 0.0).rgb; // 显式LOD允许非一致分支，不重复曝光。
            return WaterReflectionSample(max(radiance, vec3<f32>(0.0)), confidence); // HDR保留动态范围，不直接刷成镜面颜色。
        }
        previous_t = t; // 后续细化只回溯一个步长。
        previous_delta = select(-0.001, probe.delta, probe.valid > 0.5); // 天空不构成实体厚度。
    }
    return miss; // 深度预通道缺失或未命中均保留天空反射。
}

fn water_reflection_fresnel(normal: vec3<f32>, view_vector: vec3<f32>, reflectance: vec3<f32>, base_color: vec3<f32>, metallic: f32) -> vec3<f32> { // 保留作者材质的PBR反射率，不用固定水IOR覆盖已有预设。
    let cosine = clamp(dot(normalize(normal), normalize(view_vector)), 0.0, 1.0); // 正常入射弱、掠射增强。
    let grazing = 1.0 - cosine; let squared = grazing * grazing; // 固定乘法计算五次幂。
    let f0 = mix(0.16 * reflectance * reflectance, base_color, metallic); // 与Bevy calculate_F0相同的作者F0换算。
    return f0 + (vec3<f32>(1.0) - f0) * squared * squared * grazing; // 不增加独立亮度倍率，逐色通道保留作者反射率。
}
