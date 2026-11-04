// 局部雨向深度发布：同一张 RGBA16F 图片原子携带精确 f32 矩阵及反向深度。
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct CaptureUniform { // CPU ShaderType 的唯一固定矩阵。
    world_to_rain: mat4x4<f32>, // 世界到雨向 clip 坐标。
} // 结构结束。
@group(0) @binding(0) var source_depth: texture_depth_2d; // Bevy 原始 Depth32Float，最近表面值最大。
@group(0) @binding(1) var<uniform> capture: CaptureUniform; // 与源相机同帧投影。

fn encode_f32(value: f32) -> vec4<f32> { // 半精度可精确表示 0..2047，拆位不损失深度精度。
    let bits = bitcast<u32>(value); // 不对深度或矩阵进行有损数值转换。
    return vec4<f32>(f32(bits & 2047u), f32((bits >> 11u) & 2047u), f32(bits >> 22u), 1.0); // 11+11+10 位及有效标记。
} // 编码结束。

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> { // 单个绘制完整输出 N×(N+1)。
    let pixel = vec2<u32>(in.position.xy); // 精确像素读取，不插值深度。
    if pixel.y == 0u { // 首行仅存元数据，消费侧显式偏移深度行。
        if pixel.x < 16u { // 列主序对齐 Mat4::to_cols_array。
            return encode_f32(capture.world_to_rain[pixel.x / 4u][pixel.x % 4u]); // 矩阵每个 f32 一像素。
        } // 矩阵区结束。
        if pixel.x == 16u { return vec4<f32>(1.0, 0.0, 0.0, 1.0); } // 完整发布版本有效标记。
        return vec4<f32>(0.0); // 其余 metadata 像素保持无效。
    } // 元数据分支结束。
    return encode_f32(textureLoad(source_depth, vec2<i32>(i32(pixel.x), i32(pixel.y) - 1), 0)); // 无表面 reverseZ=0 自然表示无遮挡。
} // 原子转换结束。
