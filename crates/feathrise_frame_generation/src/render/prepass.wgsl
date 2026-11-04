// 保留反向Z深度与归一化运动，GPU采样到拥有COPY_SRC用途的精确内部尺寸图像。
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput // 固定全屏三角形。
@group(0) @binding(0) var depth: texture_depth_2d; // 原始深度只读采样，不要求复制用途。
@group(0) @binding(1) var motion: texture_2d<f32>; // 原始运动纹理只读采样。
struct Output { // 单pass双颜色输出。
    @location(0) depth: f32, // R32Float保存原始深度精度。
    @location(1) motion: vec2<f32>, // Rg16Float保存原始运动分量。
} // 输出结构结束。
@fragment // 只遍历真实内部渲染范围。
fn fragment(input: FullscreenVertexOutput) -> Output { // 像素位置与原始左上有效区域一一对应。
    let pixel = vec2<i32>(input.position.xy); // 不执行缩放或过滤。
    return Output(textureLoad(depth, pixel, 0), textureLoad(motion, pixel, 0).xy); // 数据值保持原样。
} // 片元入口结束。
