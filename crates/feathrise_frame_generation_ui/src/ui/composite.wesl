// 独立UI采集逐像素保留线性预乘颜色；sRGB目标由硬件编码，普通输出在RGBA16F中预乘混合。
// 复用Bevy全屏三角形坐标。
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

@group(0) @binding(0) var source: texture_2d<f32>; // 真实场景或透明UI主纹理。
@fragment // 所有复制和还原管线共用相同的无损采样入口。
fn fragment(input: FullscreenVertexOutput) -> @location(0) vec4<f32> { // 不做最终颜色相减，也不猜Alpha。
    return textureLoad(source, vec2<i32>(input.position.xy), 0); // 全输出尺寸逐像素读取，UI软边Alpha原样保留。
} // 片元入口结束。
