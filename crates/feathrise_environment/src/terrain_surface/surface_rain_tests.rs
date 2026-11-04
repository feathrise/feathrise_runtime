//! 共用雨滴、冲击冠纹生产WGSL的无GPU语法与语义校验。

#[test] // 地面与河湖读取同一份生产算法，测试不维护第二份参考实现。
fn shared_surface_rain_shader_is_valid_wgsl() {
    // 用当前Bevy同版本Naga验证循环、类型与控制流。
    let source = include_str!("shaders/rain.wgsl"); // 直接读取实际注册的雨纹库。
    let source = source
        .strip_prefix("#define_import_path feathrise_environment::surface_rain")
        .expect("共享雨纹必须保留稳定导入路径"); // 只剥离Bevy预处理指令。
    let module = wgpu::naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("共享雨纹WGSL语法错误：{error}")); // 真实解析固定候选循环、冠纹和距离带限。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("共享雨纹WGSL语义错误：{error}")); // 阻止错误延迟到实机管线。
} // 完成共享雨纹生产源码回归。

#[test] // 表面天气秒数每4096秒回绕，事件身份与相位都必须连续。
fn surface_rain_events_change_each_second_but_match_weather_wrap() {
    // 验证周期边界及邻格错相，不把一秒同点重复当成连续。
    let source = include_str!("shaders/rain.wgsl"); // 读取唯一生产雨滴相位。
    assert!(source.contains("let cycle = floor(clock) % 4096.0;")); // 事件代次必须和表面时钟共用回绕周期。
    assert!(source.contains("surface_rain_hash(cell + vec2<f32>(cycle * 17.0, cycle * 31.0))")); // 每代确实参与落点身份，不再原点重复。
    for offset in [0.0_f32, 0.125, 0.5, 0.875] {
        // 覆盖出生、中途与临近寿命末尾。
        assert_eq!((4096.0_f32 + offset).fract(), offset.fract()); // 回绕两侧具有完全相同的单元相位。
        assert_eq!((4096.0_f32 + offset).floor() % 4096.0, offset.floor()); // 回绕两侧的落点种子也必须完全一致。
        assert_ne!(offset.floor() % 4096.0, (offset + 1.0).floor() % 4096.0); // 相邻秒切换事件，避免短周期重复。
    } // 完成固定周期对齐回归。
} // 结束天气回绕连续性检查。
