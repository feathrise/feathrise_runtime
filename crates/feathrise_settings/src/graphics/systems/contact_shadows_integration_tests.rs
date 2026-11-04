//! TAA、SSAO 与接触阴影跨系统共享深度预通道回归测试。

use bevy::{
    anti_alias::taa::TemporalAntiAliasing, // 验证实验性 TAA 最终组件状态。
    camera::RenderTarget,                  // 构造正式玩法窗口目标。
    core_pipeline::prepass::{DepthPrepass, NormalPrepass}, // 验证共享深度和 SSAO 独占法线的最终所有权。
    pbr::{ContactShadows, ScreenSpaceAmbientOcclusion},    // 验证两个屏幕空间遮蔽组件状态。
    prelude::*,                                            // 轻量 App、Camera3d、Msaa 与调度类型。
    window::WindowRef,                                     // 构造主窗口引用。
};
use feathrise_scene_core::view::PrimarySceneView; // 标记唯一玩法主相机。

use super::{
    ambient_occlusion::apply_ambient_occlusion, camera::apply_antialiasing,
    contact_shadows::apply_contact_shadows,
}; // 复用正式三个运行时系统。
use crate::{AmbientOcclusionQuality, AntiAliasingMode, ContactShadowQuality, RenderSettings}; // 使用正式强类型交互规则。

/// 建立与 GraphicsSettingsPlugin 相同关键顺序的轻量三后处理应用。
fn graphics_app() -> App {
    let mut app = App::new(); // 所有权测试不需要初始化真实 GPU 渲染器。
    app.init_resource::<RenderSettings>(); // 使用正式默认画质资源。
    app.add_systems(
        Update,
        (
            apply_antialiasing,      // 先确定最终 Msaa/FXAA/TAA 状态。
            ApplyDeferred,           // 让 TAA required 组件同帧落地。
            apply_ambient_occlusion, // 再维护 SSAO 和两个预通道。
            ApplyDeferred,           // 让 SSAO required 组件同帧落地。
            apply_contact_shadows,   // 最后维护接触阴影与共享深度。
            ApplyDeferred,           // 让接触阴影恢复结果同帧可见。
        )
            .chain(),
    ); // 复现正式插件的关键系统顺序。
    app
}

/// 生成默认高档采样状态的玩法窗口相机。
fn spawn_gameplay_camera(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Camera3d::default(),                      // 建立三维相机 required 组件。
            PrimarySceneView,                             // 标记玩法主相机。
            RenderTarget::Window(WindowRef::Primary), // 输出到主窗口。
            Msaa::Sample4,                            // 模拟默认高档相机初始采样数。
        ))
        .id() // 返回实体用于各阶段断言。
}

#[test]
fn contact_then_ssao_then_taa_release_shared_depth_without_leaking() {
    let mut app = graphics_app(); // 使用正式三系统排序。
    let camera = spawn_gameplay_camera(&mut app); // 作者原本没有任何预通道。
    {
        let mut settings = app.world_mut().resource_mut::<RenderSettings>(); // 同批建立首个兼容组合。
        settings.graphics.set_anti_aliasing(AntiAliasingMode::Fxaa); // 先切到单采样 FXAA。
        settings.graphics.contact_shadow_quality = ContactShadowQuality::Low; // 接触阴影首先取得深度。
    }
    app.update(); // 第一阶段只启用接触阴影。
    assert!(app.world().get::<ContactShadows>(camera).is_some()); // 接触阴影已安装。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 深度当前只由接触阴影引入。
    assert!(app.world().get::<NormalPrepass>(camera).is_none()); // 接触阴影不需要法线预通道。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_ambient_occlusion_quality(AmbientOcclusionQuality::Low); // 再启用可与 FXAA 共存的低档 SSAO。
    app.update(); // SSAO 必须识别当前深度并非作者组件。
    assert!(
        app.world()
            .get::<ScreenSpaceAmbientOcclusion>(camera)
            .is_some()
    ); // SSAO 与接触阴影同时存在。
    assert!(app.world().get::<NormalPrepass>(camera).is_some()); // SSAO 取得独占法线预通道。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_anti_aliasing(AntiAliasingMode::Taa); // 最后让 TAA 成为第三个深度所有者。
    app.update(); // TAA 记录作者状态时必须排除两个临时屏幕空间功能。
    assert!(app.world().get::<TemporalAntiAliasing>(camera).is_some()); // 三个功能现已同时活动。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .contact_shadow_quality = ContactShadowQuality::Off; // 先关闭接触阴影。
    app.update(); // TAA 与 SSAO 仍共同持有深度。
    assert!(app.world().get::<ContactShadows>(camera).is_none()); // 接触阴影组件已归还。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 其余两个所有者需要的深度必须保留。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_ambient_occlusion_quality(AmbientOcclusionQuality::Off); // 再关闭 SSAO。
    app.update(); // TAA 单独继续持有深度。
    assert!(
        app.world()
            .get::<ScreenSpaceAmbientOcclusion>(camera)
            .is_none()
    ); // SSAO 已移除。
    assert!(app.world().get::<NormalPrepass>(camera).is_none()); // SSAO 独占法线立即释放。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // TAA 深度仍保留。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_anti_aliasing(AntiAliasingMode::Off); // 最后关闭 TAA。
    app.update(); // 三个功能全部归还控制权。
    assert!(app.world().get::<TemporalAntiAliasing>(camera).is_none()); // TAA 组件必须移除。
    assert!(app.world().get::<DepthPrepass>(camera).is_none()); // 作者原本无深度，最终不得泄漏预通道。
}

#[test]
fn taa_then_contact_shadow_keeps_depth_when_taa_releases_first() {
    let mut app = graphics_app(); // 验证相反启用和释放顺序。
    let camera = spawn_gameplay_camera(&mut app); // 作者原本没有深度预通道。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_anti_aliasing(AntiAliasingMode::Taa); // TAA 首先取得深度。
    app.update(); // 安装 TAA 与 required 组件。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .contact_shadow_quality = ContactShadowQuality::High; // 接触阴影随后共享同一深度。
    app.update(); // 备份必须排除 TAA 临时引入的组件。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_anti_aliasing(AntiAliasingMode::Off); // TAA 首先归还深度。
    app.update(); // 后置接触阴影系统必须在同帧重新确认所有权。
    assert!(app.world().get::<TemporalAntiAliasing>(camera).is_none()); // TAA 已关闭。
    assert!(app.world().get::<ContactShadows>(camera).is_some()); // 接触阴影继续活动。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 接触阴影需要的深度不得被 TAA 误删。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .contact_shadow_quality = ContactShadowQuality::Off; // 最后关闭接触阴影。
    app.update(); // 最后一个所有者归还控制权。
    assert!(app.world().get::<ContactShadows>(camera).is_none()); // 接触阴影组件移除。
    assert!(app.world().get::<DepthPrepass>(camera).is_none()); // 作者原本无深度，最终不得泄漏。
}
