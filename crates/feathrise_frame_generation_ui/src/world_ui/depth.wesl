// 将当前主场景深度映射到无抖动的输出分辨率，供独立名牌和血条进行硬件深度测试。
// 复用全屏三角形。
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

@group(0) @binding(0) var scene_depth: texture_depth_2d; // 主透明阶段之后的深度，包含实际写深度的草叶和场景。
@group(0) @binding(1) var<uniform> mapping: vec4<f32>; // xy内部/输出尺寸，zw当前抖动的屏幕位移。
@fragment // 不插值深度，避免前景与背景之间产生虚构表面。
fn fragment(input: FullscreenVertexOutput) -> @builtin(frag_depth) f32 { // 保持Bevy反向Z原值。
    let bounds = vec2<i32>(round(vec2<f32>(textureDimensions(scene_depth)) * mapping.xy)); // 只允许读取内部有效区域。
    let center = input.position.xy * mapping.xy + mapping.zw; // 输出像素中心映射回当前场景。
    let lo = clamp(vec2<i32>(floor(center - mapping.xy * 0.5 + vec2<f32>(0.00001))), vec2<i32>(0), bounds - vec2<i32>(1)); // 该输出像素实际覆盖的左上源texel。
    let hi = clamp(vec2<i32>(floor(center + mapping.xy * 0.5 - vec2<f32>(0.00001))), vec2<i32>(0), bounds - vec2<i32>(1)); // 最多跨相邻源texel，不能任意膨胀整圈内部像素。
    let upper = max(textureLoad(scene_depth, lo, 0), textureLoad(scene_depth, vec2<i32>(hi.x, lo.y), 0)); // 反向Z取较近前景，减少边缘漏洞。
    let lower = max(textureLoad(scene_depth, vec2<i32>(lo.x, hi.y), 0), textureLoad(scene_depth, hi, 0)); // 有界四次采样，无深度插值或历史。
    return max(upper, lower); // 原生无抖动时四个坐标相同，精确保留原深度。
} // 深度重映射结束。
