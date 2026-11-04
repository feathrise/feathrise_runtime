// 当前帧世界UI透明层的空间双线性重采样；材质已使用无抖动投影，这里只做尺寸映射。
// 颜色契约：标签采用作者线性显示色，与屏幕UI一致；保留原材质fog和深度遮挡，不再进入场景tonemap、bloom或时间域处理。
// 独立显示层不承诺复现旧路径先混入场景再进行非线性色调映射的半透明边缘。
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput // 当前输出像素坐标。
struct WorldUiSample { // 与Rust ShaderType保持两个vec4共32字节。
    mapping: vec4<f32>, // xy是render/output，zw保留为零，不再存放时间抖动。
    bounds: vec4<f32>, // xy是实际写入的render有效范围。
} // GPU参数结构结束。
@group(0) @binding(0) var world_ui: texture_2d<f32>; // 线性RGBA16F真实预乘透明层。
@group(0) @binding(1) var<uniform> parameters: WorldUiSample; // 固定当前帧采样身份。
fn load_transparent_outside(pixel: vec2<i32>) -> vec4<f32> { // 全尺寸纹理中只有内部有效区允许采样。
    if any(pixel < vec2<i32>(0)) || any(pixel >= vec2<i32>(parameters.bounds.xy)) { return vec4<f32>(0.0); } // 未写区域与图像外都是真透明，不夹取边缘文字。
    return textureLoad(world_ui, pixel, 0); // 只读取当前帧已完成的世界标签像素。
} // 有效区采样结束。
@fragment // 直接输出线性premul，渲染管线在目标上执行同语义混合。
fn fragment(input: FullscreenVertexOutput) -> @location(0) vec4<f32> { // 独立UI直接按无抖动输出坐标采样。
    let coordinate = input.position.xy * parameters.mapping.xy - parameters.mapping.zw - vec2<f32>(0.5); // 像素中心转texel索引，zw固定为零。
    let pixel = vec2<i32>(floor(coordinate)); // 左上采样像素。
    let fraction = fract(coordinate); // 两轴小数权重。
    let upper = mix(load_transparent_outside(pixel), load_transparent_outside(pixel + vec2<i32>(1, 0)), fraction.x); // 上行当前帧预乘颜色插值。
    let lower = mix(load_transparent_outside(pixel + vec2<i32>(0, 1)), load_transparent_outside(pixel + vec2<i32>(1, 1)), fraction.x); // 下行当前帧预乘颜色插值。
    return mix(upper, lower, fraction.y); // 连同真实Alpha一起重采样，不做背景差分或时间插帧。
} // 世界UI空间采样结束。
