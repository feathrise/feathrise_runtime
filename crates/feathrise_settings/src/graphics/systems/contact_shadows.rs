//! 玩法窗口相机接触阴影应用与作者状态恢复。

use bevy::{
    anti_alias::taa::TemporalAntiAliasing, // 识别仍持有共享深度预通道的 TAA。
    camera::RenderTarget,                  // 区分玩法窗口与纹理、无颜色目标。
    core_pipeline::prepass::DepthPrepass,  // 接触阴影 required 深度预通道。
    pbr::{ContactShadows, ScreenSpaceAmbientOcclusion}, // 接触阴影参数与共享深度的 SSAO。
    prelude::*,                            // ECS 与 Camera3d 常用类型。
};
use feathrise_scene_core::view::PrimarySceneView; // 只允许唯一玩法主相机进入接触阴影范围。

use crate::{
    ContactShadowQuality,
    RenderSettings, // 玩家档位与统一设置资源。
    GraphicsRuntimeLeaseRegistry,
    RenderScaleQuality,
    components::{
        AuthoredGameplayAmbientOcclusion, AuthoredGameplayContactShadows, TaaSupportOwnership,
    }, // 三个后处理共享深度预通道的作者所有权。
};

/// 将接触阴影应用到玩法窗口相机，并在关闭、目标切换或标记移除时恢复作者状态。
#[allow(clippy::type_complexity)] // Query 元组显式保存 TAA、SSAO 与接触阴影共享深度的作者资格。
pub(crate) fn apply_contact_shadows(
    mut commands: Commands, // 仅在参数、预通道或所有权真实变化时提交延迟命令。
    settings: Res<RenderSettings>, // 当前持久化画质设置。
    runtime_leases: Option<Res<GraphicsRuntimeLeaseRegistry>>, // 独立系统测试未装完整插件时等价于无租约。
    cameras: Query<
        (
            Entity,                                    // 当前相机实体。
            Has<PrimarySceneView>,                         // 标识当前是否仍是玩法主相机。
            &Camera,                                   // 自定义 viewport 不进入固定比例兼容压制。
            &RenderTarget,                             // 纹理和无颜色目标必须保持作者配置。
            Option<&ContactShadows>,                   // 当前接触阴影参数。
            Has<DepthPrepass>,                         // 当前共享深度预通道存在状态。
            Has<TemporalAntiAliasing>,                 // TAA 可能仍需要深度预通道。
            Option<&TaaSupportOwnership>,              // TAA 接管前的真实作者深度资格。
            Has<ScreenSpaceAmbientOcclusion>,          // SSAO 可能仍需要深度预通道。
            Option<&AuthoredGameplayAmbientOcclusion>, // SSAO 接管前的真实作者深度资格。
            Option<&AuthoredGameplayContactShadows>,   // 已保存的接触阴影作者状态。
        ),
        With<Camera3d>, // 枚举有界三维相机集合，低比例时也能压制菜单等窗口相机的作者接触阴影。
    >,
) {
    let non_native_scale = settings.graphics.render_scale != RenderScaleQuality::Native; // 低比例只写入深度纹理的局部 viewport。
    let desired = desired_contact_shadows(settings.graphics.contact_shadow_quality); // 每帧只构造一次玩家目标参数。
    for (
        entity,
        is_gameplay_camera,
        camera,
        target,
        current,
        has_depth_prepass,
        has_taa,
        taa_ownership,
        has_ssao,
        ambient_occlusion_ownership,
        authored,
    ) in &cameras
    {
        let eligible = is_gameplay_camera && matches!(target, RenderTarget::Window(_)); // 主菜单和全部纹理目标均被排除。
        let suspended = runtime_leases
            .as_deref()
            .and_then(|registry| registry.requirements_for_camera(entity))
            .is_some_and(|requirements| requirements.suspend_contact_shadows); // 合并同一相机全部租约。
        let scaled_window = (non_native_scale || suspended)
            && matches!(target, RenderTarget::Window(_))
            && camera.viewport.is_none(); // 低比例和显式租约共用保留作者备份的安全压制路径。
        if scaled_window {
            suppress_contact_shadows_for_scaled_view(
                &mut commands,
                entity,
                current,
                has_depth_prepass,
                has_taa,
                taa_ownership,
                has_ssao,
                ambient_occlusion_ownership,
                authored,
            ); // 低比例期间保留作者备份但绝不恢复会读取无效深度区域的组件。
        } else if eligible && desired.is_some() {
            let desired = desired.as_ref().expect("接触阴影目标已由启用档验证存在"); // 收窄为 Low/High 参数。
            apply_enabled_contact_shadows(
                &mut commands,
                entity,
                current,
                has_depth_prepass,
                taa_ownership,
                ambient_occlusion_ownership,
                authored,
                desired,
            ); // 幂等应用固定预算，并持续修复被其他后处理释放的 required 深度。
        } else if let Some(authored) = authored {
            restore_authored_contact_shadows(
                &mut commands,
                entity,
                current,
                has_depth_prepass,
                has_taa
                    || taa_ownership.is_some()
                    || has_ssao
                    || ambient_occlusion_ownership.is_some(),
                authored,
            ); // 关闭、目标切换和 PrimarySceneView 移除共用同一恢复路径。
        }
    }
}

/// 低比例期间临时压制窗口视图的全尺寸深度采样，并保留回到原生比例后的作者恢复依据。
#[allow(clippy::too_many_arguments)] // 参数逐一对应 Query 快照，保持共享深度所有权判断可审计。
fn suppress_contact_shadows_for_scaled_view(
    commands: &mut Commands,                     // 相机组件写入入口。
    entity: Entity,                              // 当前受固定比例接管的完整窗口相机。
    current: Option<&ContactShadows>,            // 当前可能来自作者或全局设置的接触阴影。
    has_depth_prepass: bool,                     // 当前深度预通道存在状态。
    has_taa: bool,                               // TAA 可能仍持有共享深度。
    taa_ownership: Option<&TaaSupportOwnership>, // TAA 接管前的真实作者深度资格。
    has_ssao: bool,                              // SSAO 可能仍持有共享深度。
    ambient_occlusion_ownership: Option<&AuthoredGameplayAmbientOcclusion>, // SSAO 接管前的真实作者深度资格。
    authored: Option<&AuthoredGameplayContactShadows>, // 当前是否已有可复用的作者备份。
) {
    if current.is_none() {
        return; // 已压制或作者本来无组件时不创建无意义备份，也不产生稳定帧写入。
    }
    if authored.is_none() {
        let depth_prepass = authored_depth_prepass(
            has_depth_prepass,
            taa_ownership,
            ambient_occlusion_ownership,
        ); // 首次遇到作者组件时保存其参数和保守的深度资格。
        commands
            .entity(entity)
            .insert(AuthoredGameplayContactShadows::from_current(
                current,
                depth_prepass,
            )); // 回到原生比例或退出缩放范围后能够精确恢复作者参数。
    }
    commands.entity(entity).remove::<ContactShadows>(); // 先消除会在俯视地面形成内部尺寸矩形的屏幕空间采样。

    let keep_authored_depth = authored.is_none_or(|state| state.depth_prepass); // 新捕获作者组件时保守保留 required 深度，既有备份则使用真实作者资格。
    let keep_depth = keep_authored_depth
        || has_taa
        || taa_ownership.is_some()
        || has_ssao
        || ambient_occlusion_ownership.is_some(); // 不得误删 TAA、SSAO 或作者独立持有的共享深度。
    if has_depth_prepass && !keep_depth {
        commands.entity(entity).remove::<DepthPrepass>(); // 全局接触阴影独占的临时深度可在压制时释放。
    }
}

/// 返回玩家档位对应的 Bevy 接触阴影参数；关闭档不构造组件。
fn desired_contact_shadows(quality: ContactShadowQuality) -> Option<ContactShadows> {
    match quality {
        ContactShadowQuality::Off => None, // 关闭时归还作者控制权。
        ContactShadowQuality::Low => Some(ContactShadows {
            linear_steps: 8, // 低档把默认十六步减半以控制逐光源步进成本。
            thickness: 0.1,  // 保持 Bevy 默认十厘米表面厚度估计。
            length: 0.2,     // 短射线聚焦脚底与小物体接地处。
        }),
        ContactShadowQuality::High => Some(ContactShadows::default()), // 高档保持 Bevy 16 步、0.30 米基线。
    }
}

/// 判断当前参数是否已与固定档完全一致，避免稳定帧重复触发视图管线变化。
fn contact_shadows_match(current: Option<&ContactShadows>, desired: &ContactShadows) -> bool {
    current.is_some_and(|current| {
        current.linear_steps == desired.linear_steps
            && current.thickness == desired.thickness
            && current.length == desired.length
    }) // 三个公开参数都一致才视为稳定档位。
}

/// 从 TAA、SSAO 备份中还原接触阴影接管前的真实作者深度资格。
fn authored_depth_prepass(
    has_depth_prepass: bool,                     // 当前世界中的实际组件状态。
    taa_ownership: Option<&TaaSupportOwnership>, // TAA 若已接管则排除其 required 深度。
    ambient_occlusion_ownership: Option<&AuthoredGameplayAmbientOcclusion>, // SSAO 若已接管则排除其 required 深度。
) -> bool {
    ambient_occlusion_ownership.map_or_else(
        || taa_ownership.map_or(has_depth_prepass, |state| state.depth_prepass), // 没有 SSAO 时回退 TAA 或当前状态。
        |state| state.depth_prepass, // SSAO 备份已经排除了更早启用的 TAA 或作者组件。
    )
}

/// 首次启用时保存作者状态，后续只在档位或 required 深度真实变化时写入。
#[allow(clippy::too_many_arguments)] // 参数逐一对应 Query 快照，保持所有权判断可审计。
fn apply_enabled_contact_shadows(
    commands: &mut Commands,                     // 相机组件写入入口。
    entity: Entity,                              // 当前玩法窗口相机。
    current: Option<&ContactShadows>,            // 当前接触阴影参数。
    has_depth_prepass: bool,                     // 当前深度预通道存在状态。
    taa_ownership: Option<&TaaSupportOwnership>, // TAA 接管前的真实作者深度资格。
    ambient_occlusion_ownership: Option<&AuthoredGameplayAmbientOcclusion>, // SSAO 接管前的真实作者深度资格。
    authored: Option<&AuthoredGameplayContactShadows>, // 当前是否已由全局接触阴影接管。
    desired: &ContactShadows,                          // Low 或 High 目标参数。
) {
    if authored.is_none() {
        let depth_prepass = authored_depth_prepass(
            has_depth_prepass,
            taa_ownership,
            ambient_occlusion_ownership,
        ); // 排除先启用的 TAA 与 SSAO 临时深度。
        commands
            .entity(entity)
            .insert(AuthoredGameplayContactShadows::from_current(
                current,
                depth_prepass,
            )); // 每轮启用只记录一次基线，切档不得覆盖作者状态。
    }
    if !contact_shadows_match(current, desired) {
        commands.entity(entity).insert(*desired); // 质量变化时更新固定步进预算与射线长度。
    }
    if !has_depth_prepass {
        commands.entity(entity).insert(DepthPrepass); // TAA 或 SSAO 关闭时可能释放深度，本功能同帧重新取得所有权。
    }
}

/// 恢复接管前的接触阴影与深度预通道状态，并清理本轮私有所有权组件。
fn restore_authored_contact_shadows(
    commands: &mut Commands,                   // 相机组件写入入口。
    entity: Entity,                            // 当前待归还相机。
    current: Option<&ContactShadows>,          // 当前全局或作者接触阴影参数。
    has_depth_prepass: bool,                   // 当前深度预通道存在状态。
    keep_depth_for_other_feature: bool,        // true 表示 TAA 或 SSAO 仍持有共享深度。
    authored: &AuthoredGameplayContactShadows, // 首次接管前的完整作者状态。
) {
    match authored.contact_shadows() {
        Some(value) if !contact_shadows_match(current, &value) => {
            commands.entity(entity).insert(value); // 精确恢复作者步进、厚度和射线长度。
        }
        None if current.is_some() => {
            commands.entity(entity).remove::<ContactShadows>(); // 作者原本无组件时移除全局接触阴影。
        }
        _ => {} // 当前组件已与作者状态一致时不重复写入。
    }

    let author_contact_shadows_requires_depth = authored.present; // 作者接触阴影自身继续持有 required 深度。
    let wants_depth = authored.depth_prepass
        || author_contact_shadows_requires_depth
        || keep_depth_for_other_feature; // 合并作者组件、作者预通道、TAA 与 SSAO 所有权。
    match (wants_depth, has_depth_prepass) {
        (true, false) => {
            commands.entity(entity).insert(DepthPrepass); // 恢复作者或其他活跃后处理需要的深度预通道。
        }
        (false, true) => {
            commands.entity(entity).remove::<DepthPrepass>(); // 仅移除本轮接触阴影临时引入的深度预通道。
        }
        _ => {} // 深度状态已经正确时保持稳定。
    }
    commands
        .entity(entity)
        .remove::<AuthoredGameplayContactShadows>(); // 恢复完成后立即归还作者控制权。
}

#[cfg(test)]
mod tests {
    use super::*; // 复用正式参数映射、过滤和恢复路径。
    use bevy::window::WindowRef; // 构造主窗口输出目标。

    /// 建立只运行接触阴影应用与命令落地的轻量 ECS 测试应用。
    fn test_app() -> App {
        let mut app = App::new(); // 组件测试不需要初始化真实 GPU 渲染器。
        app.init_resource::<RenderSettings>(); // 使用默认关闭接触阴影的兼容基线。
        app.add_systems(Update, (apply_contact_shadows, ApplyDeferred).chain()); // 同帧观察 required 深度与恢复结果。
        app
    }

    /// 生成符合正式作用范围的玩法窗口相机。
    fn spawn_gameplay_window_camera(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                Camera3d::default(),                      // 建立三维相机及 required 组件。
                PrimarySceneView,                             // 标记唯一玩法主相机。
                RenderTarget::Window(WindowRef::Primary), // 输出到主窗口。
            ))
            .id() // 返回实体用于逐档断言。
    }

    #[test]
    fn low_high_and_off_restore_absent_author_components() {
        let mut app = test_app(); // 使用正式玩法相机过滤和恢复路径。
        let camera = spawn_gameplay_window_camera(&mut app); // 作者初始没有接触阴影或深度预通道。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .contact_shadow_quality = ContactShadowQuality::Low; // 选择低档固定预算。
        app.update(); // 安装低档组件与 required 深度。
        let low = app.world().get::<ContactShadows>(camera).unwrap(); // 读取正式映射结果。
        assert_eq!(low.linear_steps, 8); // 低档固定八步。
        assert_eq!(low.thickness, 0.1); // 低档保持默认厚度。
        assert_eq!(low.length, 0.2); // 低档限制为二十厘米射线。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 接触阴影必须获得深度预通道。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .contact_shadow_quality = ContactShadowQuality::High; // 切换到高档。
        app.update(); // 原地更新三个参数。
        let high = app.world().get::<ContactShadows>(camera).unwrap(); // 读取高档结果。
        assert_eq!(high.linear_steps, 16); // 高档保持 Bevy 默认十六步。
        assert_eq!(high.thickness, 0.1); // 高档保持默认厚度。
        assert_eq!(high.length, 0.3); // 高档保持三十厘米射线。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .contact_shadow_quality = ContactShadowQuality::Off; // 关闭全局接触阴影。
        app.update(); // 恢复作者原本无组件状态。
        assert!(app.world().get::<ContactShadows>(camera).is_none()); // 全局接触阴影必须移除。
        assert!(app.world().get::<DepthPrepass>(camera).is_none()); // 仅由本功能引入的深度必须移除。
        assert!(
            app.world()
                .get::<AuthoredGameplayContactShadows>(camera)
                .is_none()
        ); // 归还后不得残留作者备份。
    }

    #[test]
    fn non_native_render_scale_never_applies_contact_shadows() {
        let mut app = test_app(); // 使用正式运行时兜底，覆盖绕过设置 setter 的外部写入。
        let camera = spawn_gameplay_window_camera(&mut app); // 生成会被接触阴影接管的玩法窗口相机。
        {
            let mut settings = app.world_mut().resource_mut::<RenderSettings>(); // 同帧构造原生比例高接触阴影。
            settings.graphics.contact_shadow_quality = ContactShadowQuality::High; // 模拟已经保存的高档接触阴影。
            settings.graphics.render_scale = RenderScaleQuality::Native; // 完整深度纹理允许安全启用。
        }
        app.update(); // 首帧应用高档接触阴影。
        assert!(app.world().get::<ContactShadows>(camera).is_some()); // 原生比例必须保持正式效果。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .render_scale = RenderScaleQuality::Quality; // 故意绕过 setter 制造 85% 与接触阴影同时启用的冲突。
        app.update(); // 运行时系统必须优先消除会产生俯视矩形的接触阴影路径。
        assert!(app.world().get::<ContactShadows>(camera).is_none()); // 局部 viewport 下不得保留全尺寸深度采样效果。
        assert!(app.world().get::<DepthPrepass>(camera).is_none()); // 作者原本无预通道时应一并归还临时深度。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .render_scale = RenderScaleQuality::Native; // 恢复完整深度预通道契约。
        app.update(); // 后续稳定配置应自动重新继承仍保存的高档设置。
        assert!(app.world().get::<ContactShadows>(camera).is_some()); // 原生比例恢复后接触阴影重新生效。
    }

    #[test]
    fn scaled_window_temporarily_suppresses_and_restores_authored_contact_shadows() {
        let mut app = test_app(); // 验证非玩法窗口相机的作者效果也不会穿过固定比例兼容层。
        let authored_value = ContactShadows {
            linear_steps: 11, // 使用不同于全局低高档的作者步数，确保恢复来源可辨认。
            thickness: 0.24,  // 保存作者自定义厚度。
            length: 0.42,     // 保存作者自定义射线长度。
        };
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立可能代表主菜单的三维相机。
                RenderTarget::Window(WindowRef::Primary), // 完整窗口目标会进入固定比例路径。
                authored_value,                           // 作者原本显式启用接触阴影。
            ))
            .id(); // 不带 PrimarySceneView，证明压制覆盖全部受缩放窗口视图。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .render_scale = RenderScaleQuality::Quality; // 模拟外部来源直接写入 85% 设置。
        app.update(); // 临时保存作者参数并移除不兼容组件。
        assert!(app.world().get::<ContactShadows>(camera).is_none()); // 低比例下作者组件同样必须被压制。
        assert!(
            app.world()
                .get::<AuthoredGameplayContactShadows>(camera)
                .is_some()
        ); // 压制期间必须保留精确恢复凭据。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .render_scale = RenderScaleQuality::Native; // 回到完整深度纹理契约。
        app.update(); // 非玩法相机应走普通作者状态归还路径。
        let restored = app.world().get::<ContactShadows>(camera).unwrap(); // 读取恢复后的作者参数。
        assert_eq!(restored.linear_steps, authored_value.linear_steps); // 作者步数必须精确恢复。
        assert_eq!(restored.thickness, authored_value.thickness); // 作者厚度必须精确恢复。
        assert_eq!(restored.length, authored_value.length); // 作者射线长度必须精确恢复。
        assert!(
            app.world()
                .get::<AuthoredGameplayContactShadows>(camera)
                .is_none()
        ); // 回到原生比例后立即归还所有权。
    }

    #[test]
    fn only_marker_window_camera_inherits_contact_shadows() {
        let mut app = test_app(); // 使用正式 PrimarySceneView 与窗口双重过滤。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .contact_shadow_quality = ContactShadowQuality::High; // 在生成相机前选择高档。
        let gameplay = spawn_gameplay_window_camera(&mut app); // 应接管的玩法窗口相机。
        let menu = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 主菜单同样是三维窗口相机。
                RenderTarget::Window(WindowRef::Primary), // 输出到窗口但没有玩法标记。
            ))
            .id(); // 保存菜单相机用于隔离断言。
        let texture = app
            .world_mut()
            .spawn((
                Camera3d::default(),                     // 角色预览或小地图三维相机。
                PrimarySceneView,                            // 即使误带玩法标记也必须由目标边界隔离。
                RenderTarget::None { size: UVec2::ONE }, // 模拟纹理或无颜色目标。
            ))
            .id(); // 保存专用相机用于隔离断言。
        app.update(); // 让后生成相机继承当前档位。
        assert!(app.world().get::<ContactShadows>(gameplay).is_some()); // 玩法窗口相机继承高档。
        assert!(app.world().get::<ContactShadows>(menu).is_none()); // 主菜单不承担接触阴影成本。
        assert!(app.world().get::<ContactShadows>(texture).is_none()); // 纹理目标保持作者配置。
    }

    #[test]
    fn off_restores_authored_parameters_and_depth_exactly() {
        let mut app = test_app(); // 使用正式作者状态备份路径。
        let authored_value = ContactShadows {
            linear_steps: 12, // 作者使用 UI 未公开的十二步。
            thickness: 0.35,  // 作者为当前场景定制厚度估计。
            length: 0.45,     // 作者使用四十五厘米射线。
        }; // 构造需要完整恢复的作者参数。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立三维玩法相机。
                PrimarySceneView,                             // 允许全局接触阴影临时接管。
                RenderTarget::Window(WindowRef::Primary), // 输出到主窗口。
                authored_value,                           // 作者原本已有专用接触阴影。
            ))
            .id(); // 保存实体用于恢复断言。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .contact_shadow_quality = ContactShadowQuality::High; // 临时应用全局高档。
        app.update(); // 捕获并覆盖作者参数。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .contact_shadow_quality = ContactShadowQuality::Off; // 归还作者控制权。
        app.update(); // 执行精确恢复。
        let restored = app.world().get::<ContactShadows>(camera).unwrap(); // 读取恢复后的作者组件。
        assert_eq!(restored.linear_steps, authored_value.linear_steps); // 精确恢复作者步数。
        assert_eq!(restored.thickness, authored_value.thickness); // 精确恢复作者厚度。
        assert_eq!(restored.length, authored_value.length); // 精确恢复作者射线长度。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 作者组件 required 深度必须保留。
    }

    #[test]
    fn stable_contact_shadow_frame_does_not_rewrite_components() {
        let mut app = test_app(); // 使用正式状态比较路径。
        let camera = spawn_gameplay_window_camera(&mut app); // 生成唯一目标相机。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .contact_shadow_quality = ContactShadowQuality::Low; // 固定低档。
        app.update(); // 首帧安装组件、深度与作者备份。
        app.world_mut().clear_trackers(); // 清除首帧变更标记。
        app.update(); // 相同设置稳定帧不得重复写入。
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<ContactShadows>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记接触阴影参数变更",
        );
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<AuthoredGameplayContactShadows>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复保存接触阴影作者状态",
        );
    }
}
