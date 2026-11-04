// RGBA16F的RGB精确保存32位浮点的11/11/10位整数，避免远距离屋顶深度半精度量化。
fn rain_occlusion_decode(encoded: vec4<f32>) -> f32 { // 与生产写入保持同一位序。
    let bits = u32(encoded.r) | (u32(encoded.g) << 11u) | (u32(encoded.b) << 22u); // 所有编码整数均可由f16精确表示。
    return bitcast<f32>(bits); // 不把编码当成数值深度缩放。
} // 完成无损解码。
fn rain_occlusion_column(column: i32) -> vec4<f32> { // 首行四个像素对应矩阵的一列。
    let base = column * 4; // Mat4::to_cols_array使用列主序。
    return vec4<f32>( // 每个分量从同一完整纹理读取。
        rain_occlusion_decode(textureLoad(material_texture_1, vec2<i32>(base, 0), 0)), // 行0。
        rain_occlusion_decode(textureLoad(material_texture_1, vec2<i32>(base + 1, 0), 0)), // 行1。
        rain_occlusion_decode(textureLoad(material_texture_1, vec2<i32>(base + 2, 0), 0)), // 行2。
        rain_occlusion_decode(textureLoad(material_texture_1, vec2<i32>(base + 3, 0), 0)), // 行3。
    ); // 不读取独立属性矩阵，避免移动相机或热更新时身份错位。
} // 完成矩阵列读取。
