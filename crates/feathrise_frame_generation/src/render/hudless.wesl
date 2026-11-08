// 将色调映射后的线性场景逐像素写入 sRGB 附件，硬件完成与窗口一致的颜色编码。
// 复用全屏三角形。
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

@group(0) @binding(0) var source: texture_2d<f32>; // 已完成后处理的线性 RGBA16F 颜色。
@fragment // 不做第二次色调映射，也不在 CPU 上转换像素。
fn fragment(input: FullscreenVertexOutput) -> @location(0) vec4<f32> { // 与最终窗口使用同一个线性至 sRGB 转换。
    return textureLoad(source, vec2<i32>(input.position.xy), 0); // 输出尺寸相同，无滤波偏差。
} // HUDless 编码结束。
