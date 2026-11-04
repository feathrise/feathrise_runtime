//! 雨天地表PBR纯函数：作者潜力控制覆盖，坡度排除陡面，材质粗糙度保留干湿差异。
#define_import_path feathrise_environment::wet_surface

const WATER_SURFACE_REFLECTANCE: f32 = 0.354077; // Bevy F0=0.16*r²，水IOR=1.33给出F0约2.006%，交由标准Fresnel增强掠射反射。

fn water_reflection_roughness( // 共用现有PBR环境镜面，不新增纹理或反射通道。
    author_roughness: f32, // 晴天河湖保留作者粗糙度，积水传入水膜粗糙度。
    rain: f32, // 降雨量控制未解析微波的镜面展宽。
    wind: f32, // 同源风强只调节雨中微波，不覆盖晴天作者水材质。
    pixel_meters: f32, // 一致控制流已求得的片元世界脚印，不在分支内求导。
) -> f32 { // 固定ALU预算，所有画质共享物理主体。
    let rain_weight = clamp(rain, 0.0, 1.0); // 小雨至大雨连续演化，避开WGSL保留字。
    if rain_weight <= 0.0 { return author_roughness; } // 雨停精确保留作者晴天基线。
    let base = clamp(author_roughness, 0.0, 1.0); // 合法材质范围防止反射分布失效。
    let unresolved = smoothstep(0.04, 0.32, max(pixel_meters, 0.0)); // 雨圈开始亚像素化时逐渐扩大镜面瓣，避免远景闪点。
    let variance = rain_weight * (0.014 + 0.026 * clamp(wind, 0.0, 1.0) + 0.040 * unresolved); // 只加入有限微面方差，不凭空增加反射能量。
    return sqrt(min(base * base + variance, 1.0)); // 在微面粗糙度域累加后返回感知粗糙度，最多为一。
} // 完成雨风镜面分布与像素抗闪。

struct WetSurfaceSample { // 无纹理或天气全局依赖，便于生产函数独立验证。
    color_scale: f32, // 保留原纹理细节的线性乘色。
    roughness: f32, // 混合后的非零感知粗糙度。
    puddle: f32, // 连续积水覆盖权重。
    normal: vec3<f32>, // 水膜压平细法线但保留低坡地形方向。
} // 完成地表湿润结果。

fn wet_surface_noise(position: vec2<f32>) -> f32 { // 固定世界尺度软边，不让遮罩跟随镜头移动。
    let cell = floor(position); // 以世界网格确定四个角点。
    let local = fract(position); // 单元内连续位置。
    let blend = local * local * (vec2<f32>(3.0) - 2.0 * local); // 三次平滑跨格无硬缝。
    let hashes = fract(sin(vec4<f32>(dot(cell, vec2<f32>(127.1, 311.7)), dot(cell + vec2<f32>(1.0, 0.0), vec2<f32>(127.1, 311.7)), dot(cell + vec2<f32>(0.0, 1.0), vec2<f32>(127.1, 311.7)), dot(cell + vec2<f32>(1.0), vec2<f32>(127.1, 311.7)))) * 43758.5453); // 固定四角噪声，没有迭代增长。
    return mix(mix(hashes.x, hashes.y, blend.x), mix(hashes.z, hashes.w, blend.x), blend.y); // 返回零至一的平滑值。
} // 完成固定噪声。

fn evaluate_wet_surface( // 所有参数来自当前地表和同帧完整天气。
    dry_roughness: f32, // 原地表粗糙度保留石板与泥土的差别。
    normal: vec3<f32>, // 原地表细法线。
    geometry_normal: vec3<f32>, // 坡度只能用真实几何法线，不能受纹理法线影响。
    potential: f32, // 作者积水潜力，零值严格禁止积水。
    wetness: f32, // 区域湿润历史。
    amount: f32, // 区域慢速积水历史。
    noise: f32, // 固定世界细噪声，仅破碎积水边缘。
) -> WetSurfaceSample { // 不改变碰撞、水位或地形网格。
    let wet = clamp(wetness, 0.0, 1.0); // 无效天气已在CPU处理，此处限制合法数值范围。
    let slope = smoothstep(0.94, 0.997, geometry_normal.y); // 大于约20度的地形不出现静止积水。
    let eligible = smoothstep(0.0, 0.08, potential) * slope; // 零作者值保持精确零覆盖。
    let threshold = 1.02 - clamp(amount, 0.0, 1.0) * 0.96; // 雨水增加时从最强作者区域向边缘扩张。
    let puddle = eligible * smoothstep(threshold, threshold + 0.10, potential - noise * 0.14) * smoothstep(0.0, 0.08, amount); // 零积水量绝不出现水膜。
    let soil_darkness = mix(0.80, 0.68, clamp(dry_roughness, 0.0, 1.0)); // 粗糙泥土比光滑石板吸水变深更明显。
    let color_scale = mix(mix(1.0, soil_darkness, wet), 0.66, puddle); // 可见底色保留细节，不替换成纯蓝水面。
    let damp_roughness = mix(dry_roughness, max(0.22, dry_roughness * 0.65), wet); // 湿土仍保留较高粗糙度。
    let roughness = mix(damp_roughness, 0.12, puddle); // 浅积水保留非零粗糙度，减少高光闪烁。
    let water_normal = normalize(mix(geometry_normal, vec3<f32>(0.0, 1.0, 0.0), 0.80)); // 低坡浅水接近水平且不修改几何。
    return WetSurfaceSample(color_scale, roughness, puddle, normalize(mix(normal, water_normal, puddle * 0.94))); // 只压平水膜覆盖的底层细法线。
} // 完成湿润和积水PBR评估。
