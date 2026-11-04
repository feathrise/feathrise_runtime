//! 雨向 GPU 发布回归：验证半精度容器不损失 f32 位模式，并校验正式转换片元 Shader。

#[test] // 包括负矩阵元素、极小深度及空背景，不能只测试可精确半精度表示的常数。
fn half_float_channels_preserve_every_depth_and_matrix_bit() {
    // 11/11/10 整数分量在 half 中必须逐个精确。
    for component in 0..2048u32 {
        // 穷尽最大的通道整数域。
        assert_eq!(
            half::f16::from_f32(component as f32).to_f32() as u32,
            component
        ); // GPU RGBA16F 转换不会改变整数。
    } // 整数精度验证完成。
    for value in [
        0.0f32,
        1.0,
        -1.0,
        0.000_000_019,
        0.127_319_47,
        -1387.238,
        65536.25,
        f32::MIN_POSITIVE,
    ] {
        // 覆盖常见矩阵与深度范围。
        let bits = value.to_bits(); // 与 WGSL bitcast 对齐。
        let encoded = [bits & 2047, (bits >> 11) & 2047, bits >> 22]
            .map(|part| half::f16::from_f32(part as f32).to_f32() as u32); // 模拟真实纹理格式存储。
        let decoded = encoded[0] | (encoded[1] << 11) | (encoded[2] << 22); // 与消费片元解码同一位宽。
        assert_eq!(
            decoded, bits,
            "遮雨矩阵或深度不能因RGBA16F损失精度：{value}"
        ); // 必须位级一致。
    } // 深度和矩阵精度验证完成。
} // 编码回归结束。

#[test] // 检测转换 Shader 的真实 WGSL 类型、纹理读取及动态矩阵下标。
fn depth_capture_fragment_passes_naga_validation() {
    // 外部边界仅提供 Bevy FullscreenVertexOutput 类型。
    let shader = include_str!("convert.wgsl"); // 使用真正生产源码，不复制片元实现。
    let source = shader.replace("#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput", "struct FullscreenVertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }; "); // 与官方 fullscreen_vertex_shader 精确匹配接口。
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source))); // 保留可定位源码的诊断。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("正式深度发布Shader必须通过类型及入口验证"); // 包含正式片元入口的语义验证。
} // Shader 回归结束。
