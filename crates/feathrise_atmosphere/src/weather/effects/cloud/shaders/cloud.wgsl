//! 动态天气云透明片元入口。
//! 【功能设计】球体只提供世界位置；片元从相机到世界位置重建视线，在水平云层上做有界投影，
//! 以两次四通道周期噪声采样形成云团、云洞和云底层次，并在地平线平滑融入大气背景。

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view

struct DynamicCloudParams {
    top_color: vec4<f32>, // 较亮云顶线性色。
    bottom_color: vec4<f32>, // 厚云云底线性色。
    horizon_color: vec4<f32>, // 动态大气不可用时也与 ClearColor 同源。
    motion_cover: vec4<f32>, // xy 周期风偏移，z 有效云量，w 最大不透明度。
    projection: vec4<f32>, // x 投影缩放，y 最小高度分母，zw 地平线淡入区间。
    sun_direction: vec4<f32>, // xyz 为最终太阳光方向；夜间为零。
    sun_color_rim: vec4<f32>, // rgb 为最终日光色相，w 为低角度暖边强度。
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> params: DynamicCloudParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var cloud_noise: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var cloud_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let ray = in.world_position.xyz - view.world_from_view[3].xyz; // 载体跟随相机，但仍从真实世界位置恢复视线。
    let ray_length_squared = dot(ray, ray); // 防御退化插值，避免 normalize 零向量。
    if ray_length_squared <= 1e-8 {
        discard; // 无合法视线时不污染天空颜色。
    }
    let direction = ray * inverseSqrt(ray_length_squared); // 单位世界视线与球体 UV、经线和极点无关。
    if direction.y <= 0.0 {
        discard; // 动态云只覆盖天空半球，不给地下或水下铺一层灰色。
    }

    let projected = direction.xz / max(direction.y, params.projection.y); // 低仰角分母有界，杜绝无限拉伸与镜像条带。
    let uv_b = projected * (params.projection.x * 0.53)
        - params.motion_cover.xy * 0.37
        + vec2<f32>(0.371, -0.193); // 第二层使用不同尺度与相位打散周期感。
    let noise_b = textureSample(cloud_noise, cloud_sampler, uv_b); // 固定第二次采样提供独立结构变化。
    let warp = (noise_b.rg - vec2<f32>(0.5)) * 0.12; // 低频连续位移扭曲主云轮廓，不增加第三次采样。
    let uv_a = projected * params.projection.x + params.motion_cover.xy + warp; // 主云随世界风移动，边缘不再呈同心重复纹理。
    let noise_a = textureSample(cloud_noise, cloud_sampler, uv_a); // 一次取得大、中、细、侵蚀四个尺度。

    let broad = noise_a.r * 0.68 + noise_b.r * 0.32; // 两套低频共同决定大小云团与云洞。
    let edge = (noise_a.g - 0.5) * 0.14 + (noise_a.b - noise_b.a) * 0.07; // 高频只侵蚀轮廓，不铺满灰色细节。
    let field = broad + edge; // 云密度不包含固定最低不透明度。
    let coverage_curve = pow(clamp(params.motion_cover.z, 0.0, 1.0), 0.72); // 中高云量更快形成连贯阴云层。
    let threshold = mix(0.77, 0.34, coverage_curve); // 暴雨连贯但保留真实云洞，不靠整屏最低Alpha。
    let antialias_width = max(mix(0.07, 0.04, coverage_curve), fwidth(field) * 1.35); // 远景脚印自动带限柔边。
    let cloud_body = smoothstep(threshold - antialias_width, threshold + antialias_width, field); // 清晰但不硬切的云团边缘。
    let cloud_core = smoothstep(threshold + 0.12, threshold + 0.32, field); // 厚云核心决定云底暗度而非整个灰板。

    let horizon_weight = 1.0 - smoothstep(params.projection.z, params.projection.w, direction.y); // 只在低仰角进入大气衔接。
    let horizon_visibility = smoothstep(0.003, 0.035, direction.y); // 正地平线处平滑归零，避免球面硬边。
    let shaped_alpha = cloud_body
        * params.motion_cover.w
        * horizon_visibility; // 雨天低空也只显示真实云体，云洞严格为零Alpha。
    if shaped_alpha <= 0.001 {
        discard; // 完全透明区域不执行后续混色也不占用无意义片元输出。
    }

    let height_light = smoothstep(0.02, 0.55, direction.y); // 高仰角更容易看到受天光照亮的云顶。
    let thin_edge = cloud_body * (1.0 - cloud_core); // 只有薄边接受朝阳暖光，厚云核心保留压暗。
    let light_mix = clamp(0.38 + height_light * 0.30 - cloud_core * 0.23 + thin_edge * 0.12, 0.0, 1.0); // 天光亮顶与厚云暗底不再反转。
    let cloud_color = mix(params.bottom_color.rgb, params.top_color.rgb, light_mix); // 云底与云顶分层塑造厚度。
    var lit_color = cloud_color; // 正午、午夜和无有效太阳灯不需要额外日光计算。
    if params.sun_color_rim.w > 0.0 { // Uniform分支让夜间片元不计算无用的太阳方向幂。
        let sun_facing = pow(max(dot(direction, params.sun_direction.xyz), 0.0), 8.0); // 只在朝向真实太阳时形成局部渐变。
        let rim = thin_edge * sun_facing * params.sun_color_rim.w * 0.23; // 最终照度和低角度共同限制暖边亮度。
        lit_color = cloud_color + params.sun_color_rim.rgb * rim; // 不给背阳厚云凭空加光。
    } // 夜间保持完全由最终天空与天气决定的暗云色。
    let final_color = mix(lit_color, params.horizon_color.rgb, horizon_weight * 0.58); // 低仰角平滑进入大气雾色。
    return vec4<f32>(final_color, shaped_alpha); // 非预乘 Alpha 交给 Bevy Blend 管线合成动态天空。
}
