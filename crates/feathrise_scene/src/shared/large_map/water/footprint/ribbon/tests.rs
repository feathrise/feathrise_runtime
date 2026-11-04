//! 雨花同源三角面回归：非线性波高必须先变形顶点，岸外与无UV来源不授权。

use super::*; // 测试实际构造和动态采样入口。
use bevy::prelude::Vec4; // 构造真实GPU参数布局。

fn footprint() -> RibbonWaterFootprint {
    RibbonWaterFootprint::from_ribbon_vertices(
        vec![
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, 1.0),
            Vec3::new(0.0, 2.0, 2.0),
            Vec3::new(10.0, 4.0, 0.0),
            Vec3::new(10.0, 4.0, 1.0),
            Vec3::new(10.0, 4.0, 2.0),
        ], // 两行三列真实斜河网格。
        vec![2.0, 4.0], // 作者深度沿下游增长。
        3,              // 横向两格，中间顶点不能被两岸四边形替代。
    )
    .unwrap() // 夹具几何合法。
}

fn waves() -> RibbonWaterFootprint {
    footprint()
        .with_wave_sections(vec![Vec3::new(0.0, 1.0, 0.0), Vec3::new(10.0, 1.0, 0.0)])
        .unwrap() // 参数与两行实际Mesh UV完全相同。
}

fn frame() -> DynamicWaterGpuFrame {
    let mut frame = DynamicWaterGpuFrame::default(); // 无玩家波和次波以隔离三角面插值。
    frame.surface_settings = Vec4::new(0.1, 1.0, 0.0, 0.0); // 两岸十分之一宽度抑制，时间相位固定。
    frame.wave_directions_amplitudes[0].z = 1.0; // 一米主波放大解析曲面与实际片面差异。
    frame // 返回实际材质ABI。
}

#[test]
fn ribbon_splash_uses_displaced_triangle_not_analytic_point() {
    let mesh = waves(); // 构造带真实UV的两行网格。
    let position =
        ribbon_splash_position(&mesh, &frame(), 0.0, Vec2::new(5.0, 0.5), |_| 0.0).unwrap(); // 落点位于第一片对角线中点。
    let expected = 3.0 + 1.05_f32.sin() * 0.5; // 下游中点和上游岸点变形后各占一半。
    assert!((position.y - expected).abs() < 0.00001); // 必须贴合GPU三角片面。
    assert!((position.y - (3.0 + 0.525_f32.sin())).abs() > 0.05); // 点上直接采样连续正弦曲面会错误悬浮。
    assert_eq!(
        mesh.sample(Vec2::new(5.0, 0.5)).unwrap().surface_local_y,
        3.0
    ); // 原玩家接触API仍使用静止高度。
}

#[test]
fn ribbon_splash_preserves_banks_and_rejects_unknown_geometry() {
    let mesh = waves(); // 使用实际同源截面。
    let bank = ribbon_splash_position(&mesh, &frame(), 1.0, Vec2::new(5.0, 0.0), |_| 20.0).unwrap(); // 玩家波再大也不能抬起埋岸。
    assert_eq!(bank.y, 3.0); // 岸带衰减同时抑制背景与玩家波。
    assert!(ribbon_splash_position(&mesh, &frame(), 1.0, Vec2::new(5.0, -0.1), |_| 0.0).is_none()); // 岸外落点不发射。
    assert!(ribbon_splash_position(&footprint(), &frame(), 1.0, Vec2::ONE, |_| 0.0).is_none()); // 兼容构造器没有真实UV时不猜测。
}

#[test]
fn ribbon_splash_player_height_is_evaluated_at_mesh_vertices() {
    let mesh = waves(); // 同源三角剖分。
    let mut parameters = frame(); // 从合法GPU参数开始。
    parameters.wave_directions_amplitudes[0].z = 0.0; // 隔离调用方玩家波函数。
    let position = ribbon_splash_position(&mesh, &parameters, 0.0, Vec2::new(5.0, 0.5), |p| {
        p.x * p.x * 0.001
    })
    .unwrap(); // 非线性高度可检测错误的点上采样。
    assert!((position.y - 3.05).abs() < 0.00001); // 下游顶点0.1米按一半权重贡献。
}

#[test]
fn ribbon_splash_rejects_mismatched_or_waterfall_sections() {
    assert!(footprint().with_wave_sections(vec![Vec3::ZERO]).is_err()); // 截面数量错位不能发布。
    assert!(
        footprint()
            .with_wave_sections(vec![Vec3::new(0.0, 1.0, 2.0); 2])
            .is_err()
    ); // 瀑布使用法线位移，不允许走河流Y位移入口。
    assert!(
        footprint()
            .with_wave_sections(vec![Vec3::splat(f32::NAN); 2])
            .is_err()
    ); // 坏作者参数不能进入运行时。
}

#[test]
fn ribbon_splash_wrap_and_rapid_follow_gpu_parameters() {
    let mesh = footprint()
        .with_wave_sections(vec![Vec3::new(0.0, 1.0, 1.0), Vec3::new(10.0, 1.0, 1.0)])
        .unwrap(); // 两个截面都使用急流样式。
    let parameters = frame(); // 零流速使急流频率与振幅可独立检查。
    let position =
        ribbon_splash_position(&mesh, &parameters, 0.0, Vec2::new(10.0, 1.0), |_| 0.0).unwrap(); // 正好落在下游中点顶点。
    assert!((position.y - (4.0 + 1.6_f32.sin() * 1.7)).abs() < 0.00001); // production急流主频0.16、振幅倍数1.7。
    assert!((phase_offset(0.0, 0.735).sin() - phase_offset(3600.0, 0.735).sin()).abs() < 0.00001); // Globals回绕保持同相。
}
