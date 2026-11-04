//! SSAO 与 TAA 跨系统共享预通道所有权回归测试。

use bevy::{
    anti_alias::{fxaa::Fxaa, taa::TemporalAntiAliasing}, // 验证 FXAA/TAA 互斥后的最终组件状态。
    camera::RenderTarget,                                // 构造正式玩法窗口目标。
    core_pipeline::prepass::{DepthPrepass, NormalPrepass}, // 验证共享深度和 SSAO 独占法线均被正确释放。
    pbr::ScreenSpaceAmbientOcclusion,                      // 验证 SSAO 最终归还作者控制权。
    prelude::*,                                            // 轻量 App、Camera3d、Msaa 与调度类型。
    window::WindowRef,                                     // 构造主窗口引用。
};
use feathrise_scene_core::view::PrimarySceneView; // 标记唯一玩法主相机。

use super::{ambient_occlusion::apply_ambient_occlusion, camera::apply_antialiasing}; // 复用正式两个运行时系统。
use crate::{AmbientOcclusionQuality, AntiAliasingMode, RenderSettings}; // 使用正式强类型交互规则。

/// 当 SSAO 先于 TAA 持有深度预通道时，依次关闭两个功能不得残留无主组件。
#[test]
fn ssao_then_taa_release_shared_depth_without_leaking_prepasses() {
    let mut app = App::new(); // 组件所有权测试不需要初始化真实渲染器。
    app.init_resource::<RenderSettings>(); // 使用正式默认画质资源。
    app.add_systems(
        Update,
        (
            apply_antialiasing,      // 先确定最终 Msaa/FXAA/TAA 状态。
            ApplyDeferred,           // 让 TAA required 组件同帧落地。
            apply_ambient_occlusion, // 再按最终采样状态维护 SSAO。
            ApplyDeferred,           // 让 SSAO required 组件同帧落地。
        )
            .chain(),
    ); // 复现 GraphicsSettingsPlugin 的关键系统顺序。
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),                      // 建立三维相机 required 组件。
            PrimarySceneView,                             // 标记玩法主相机。
            RenderTarget::Window(WindowRef::Primary), // 输出到主窗口。
            Msaa::Sample4,                            // 模拟默认高档相机初始采样数。
        ))
        .id(); // 保存实体用于各阶段断言。

    {
        let mut settings = app.world_mut().resource_mut::<RenderSettings>(); // 同批设置初始兼容组合。
        settings.graphics.set_anti_aliasing(AntiAliasingMode::Fxaa); // 先用单采样 FXAA。
        settings
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::Low); // 再开启低档 SSAO。
    }
    app.update(); // SSAO 首先取得原本不存在的深度和法线预通道。
    assert!(app.world().get::<Fxaa>(camera).is_some()); // 第一阶段使用 FXAA。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 深度当前只由 SSAO 引入。
    assert!(app.world().get::<NormalPrepass>(camera).is_some()); // 法线当前只由 SSAO 引入。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_anti_aliasing(AntiAliasingMode::Taa); // 保持 SSAO 启用并改由 TAA 共享深度。
    app.update(); // TAA 记录作者状态时必须排除 SSAO 临时深度。
    assert!(app.world().get::<TemporalAntiAliasing>(camera).is_some()); // 第二阶段启用 TAA。
    assert!(
        app.world()
            .get::<ScreenSpaceAmbientOcclusion>(camera)
            .is_some()
    ); // SSAO 同时保持启用。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_ambient_occlusion_quality(AmbientOcclusionQuality::Off); // 先关闭 SSAO。
    app.update(); // TAA 仍需要共享深度，只有法线应被释放。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // TAA 活跃期间深度必须保留。
    assert!(app.world().get::<NormalPrepass>(camera).is_none()); // SSAO 独占法线必须立即移除。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_anti_aliasing(AntiAliasingMode::Off); // 最后关闭 TAA。
    app.update(); // 两个后处理都归还控制权。
    assert!(app.world().get::<TemporalAntiAliasing>(camera).is_none()); // TAA 组件必须移除。
    assert!(
        app.world()
            .get::<ScreenSpaceAmbientOcclusion>(camera)
            .is_none()
    ); // SSAO 组件保持关闭。
    assert!(app.world().get::<DepthPrepass>(camera).is_none()); // 作者原本无深度，最终不得泄漏预通道。
    assert!(app.world().get::<NormalPrepass>(camera).is_none()); // 作者原本无法线，最终不得泄漏预通道。
}
