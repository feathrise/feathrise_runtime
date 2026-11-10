//! 玩法窗口相机 SSAO 环境光遮蔽应用与作者状态恢复。

use bevy::{
    anti_alias::taa::TemporalAntiAliasing, // 判断深度预通道是否仍由 TAA 持有。
    camera::RenderTarget,                  // 区分窗口与角色预览、小场景纹理目标。
    core_pipeline::prepass::{DepthPrepass, NormalPrepass}, // SSAO 的两个 required 预通道。
    pbr::{ContactShadows, ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel}, // Bevy 0.19 屏幕空间遮蔽组件与内建 SSAO 档。
    prelude::*, // ECS、Camera3d 与 Msaa 常用类型。
};
use feathrise_scene_core::view::PrimarySceneView; // 只允许唯一玩法主相机进入 SSAO 接管范围。

use crate::{
    AmbientOcclusionQuality,
    RenderSettings, // 玩家档位与统一设置资源。
    GraphicsRuntimeLeaseRegistry,
    components::{
        AuthoredGameplayAmbientOcclusion, AuthoredGameplayContactShadows, TaaSupportOwnership,
    }, // SSAO/TAA/接触阴影预通道所有权。
};

/// 同时枚举当前玩法相机与仍需归还作者状态的旧玩法相机。
type AmbientOcclusionCameraFilter = (
    With<Camera3d>, // SSAO 只支持三维视图。
    Or<(
        With<PrimarySceneView>,                     // 当前玩法主相机。
        With<AuthoredGameplayAmbientOcclusion>, // 已接管但可能刚失去 PrimarySceneView 的相机。
    )>,
);

/// 将 SSAO 应用到玩法窗口相机，并在关闭、目标切换或标记移除时精确恢复作者状态。
#[allow(clippy::type_complexity)] // Query 元组显式保存共享深度预通道的全部作者资格，避免恢复时丢失所有权。
pub(crate) fn apply_ambient_occlusion(
    mut commands: Commands, // 仅在档位、预通道或所有权真实变化时提交延迟命令。
    settings: Res<RenderSettings>, // 当前已归一化的画质设置。
    runtime_leases: Option<Res<GraphicsRuntimeLeaseRegistry>>, // 独立系统测试未装完整插件时等价于无租约。
    cameras: Query<
        (
            Entity,                                    // 当前相机实体。
            Has<PrimarySceneView>,                         // 标识当前是否仍是玩法主相机。
            &RenderTarget,                             // 纹理和无颜色目标必须保持作者配置。
            &Msaa,                                     // Bevy 0.19 SSAO 要求严格等于 Msaa::Off。
            Option<&ScreenSpaceAmbientOcclusion>,      // 当前 SSAO 参数。
            Has<DepthPrepass>,                         // 当前深度预通道存在状态。
            Has<NormalPrepass>,                        // 当前法线预通道存在状态。
            Has<TemporalAntiAliasing>, // 关闭 SSAO 时不能误删 TAA 仍需要的深度预通道。
            Option<&TaaSupportOwnership>, // 识别深度预通道在 TAA 之前的真实作者状态。
            Has<ContactShadows>,       // 关闭 SSAO 时不能误删接触阴影仍需要的深度预通道。
            Option<&AuthoredGameplayContactShadows>, // 识别深度是否由全局接触阴影临时引入。
            Option<&AuthoredGameplayAmbientOcclusion>, // 已保存的 SSAO 作者状态。
        ),
        AmbientOcclusionCameraFilter,
    >,
) {
    let desired = desired_ambient_occlusion(settings.graphics.ambient_occlusion_quality); // 每帧只构造一次目标参数。
    for (
        entity,
        is_gameplay_camera,
        target,
        msaa,
        current,
        has_depth_prepass,
        has_normal_prepass,
        has_taa,
        taa_ownership,
        has_contact_shadows,
        contact_shadow_ownership,
        authored,
    ) in &cameras
    {
        let eligible = is_gameplay_camera && matches!(target, RenderTarget::Window(_)); // 主菜单和全部纹理目标均被排除。
        let compatible = *msaa == Msaa::Off; // 非法外部状态也只安全跳过，不把错误送入 RenderApp。
        let suspended = runtime_leases
            .as_deref()
            .and_then(|registry| registry.requirements_for_camera(entity))
            .is_some_and(|requirements| requirements.suspend_ssao); // 只影响租约目标，不波及纹理相机。
        let wants_ssao = eligible && compatible && desired.is_some() && !suspended; // 租约期保留玩家请求但归还SSAO组件。

        if wants_ssao {
            let desired = desired.as_ref().expect("SSAO 目标已由 wants_ssao 验证存在"); // 收窄为启用档参数。
            apply_enabled_ambient_occlusion(
                &mut commands,
                entity,
                current,
                has_depth_prepass,
                has_normal_prepass,
                taa_ownership,
                contact_shadow_ownership,
                authored,
                desired,
            ); // 幂等应用 Low/High，并持续修复被其他后处理释放的 required 预通道。
        } else if let Some(authored) = authored {
            restore_authored_ambient_occlusion(
                &mut commands,
                entity,
                current,
                has_depth_prepass,
                has_normal_prepass,
                has_taa
                    || taa_ownership.is_some()
                    || has_contact_shadows
                    || contact_shadow_ownership.is_some(),
                authored,
            ); // 关闭、MSAA、目标切换和 PrimarySceneView 移除共用同一恢复路径。
        }
    }
}

/// 返回玩家档位对应的 Bevy SSAO 参数；关闭档不构造组件。
fn desired_ambient_occlusion(
    quality: AmbientOcclusionQuality, // 当前持久化档位。
) -> Option<ScreenSpaceAmbientOcclusion> {
    let quality_level = match quality {
        AmbientOcclusionQuality::Off => return None, // 关闭时归还作者控制权。
        AmbientOcclusionQuality::Low => ScreenSpaceAmbientOcclusionQualityLevel::Low, // 4 spp 基础样本。
        AmbientOcclusionQuality::High => ScreenSpaceAmbientOcclusionQualityLevel::High, // 18 spp 基础样本。
    };
    Some(ScreenSpaceAmbientOcclusion {
        quality_level, // 使用 Bevy 已验证的固定质量档，避免自定义无限预算。
        ..default()    // 保持 0.25 的标准物体厚度估计。
    })
}

/// 首次启用时保存作者状态，后续只在档位或 required 组件真实变化时写入。
#[allow(clippy::too_many_arguments)] // 参数逐一对应 Query 快照，避免隐藏所有权判断所需状态。
fn apply_enabled_ambient_occlusion(
    commands: &mut Commands,                       // 相机组件写入入口。
    entity: Entity,                                // 当前玩法窗口相机。
    current: Option<&ScreenSpaceAmbientOcclusion>, // 当前 SSAO 参数。
    has_depth_prepass: bool,                       // 当前深度预通道存在状态。
    has_normal_prepass: bool,                      // 当前法线预通道存在状态。
    taa_ownership: Option<&TaaSupportOwnership>,   // TAA 接管前的真实深度作者资格。
    contact_shadow_ownership: Option<&AuthoredGameplayContactShadows>, // 接触阴影接管前的真实深度作者资格。
    authored: Option<&AuthoredGameplayAmbientOcclusion>, // 当前是否已经由全局 SSAO 接管。
    desired: &ScreenSpaceAmbientOcclusion,               // Low 或 High 目标参数。
) {
    if authored.is_none() {
        commands
            .entity(entity)
            .insert(AuthoredGameplayAmbientOcclusion {
                ambient_occlusion: current.cloned(), // 完整保存作者质量和厚度参数。
                depth_prepass: contact_shadow_ownership.map_or_else(
                    || taa_ownership.map_or(has_depth_prepass, |state| state.depth_prepass), // 没有接触阴影时排除 TAA 临时深度。
                    |state| state.depth_prepass, // 接触阴影备份已经排除更早启用的 TAA。
                ),
                normal_prepass: has_normal_prepass, // SSAO 接管前已有法线预通道视为作者状态。
            }); // 每轮启用只记录一次基线，切档不得覆盖作者状态。
    }
    if current != Some(desired) {
        commands.entity(entity).insert(desired.clone()); // 质量变化时更新 SSAO 并触发所需管线专用化。
    }
    if !has_depth_prepass {
        commands.entity(entity).insert(DepthPrepass); // TAA 关闭时可能释放深度，SSAO 同帧重新取得所有权。
    }
    if !has_normal_prepass {
        commands.entity(entity).insert(NormalPrepass); // SSAO 始终需要屏幕空间法线。
    }
}

/// 恢复接管前的 SSAO 与预通道状态，并清理本轮私有所有权组件。
#[allow(clippy::too_many_arguments)] // 显式状态让恢复规则可审计且便于 ECS 回归测试。
fn restore_authored_ambient_occlusion(
    commands: &mut Commands,                       // 相机组件写入入口。
    entity: Entity,                                // 当前待归还相机。
    current: Option<&ScreenSpaceAmbientOcclusion>, // 当前全局或作者 SSAO 参数。
    has_depth_prepass: bool,                       // 当前深度预通道存在状态。
    has_normal_prepass: bool,                      // 当前法线预通道存在状态。
    keep_depth_for_other_feature: bool,            // true 表示 TAA 或接触阴影仍持有深度预通道。
    authored: &AuthoredGameplayAmbientOcclusion,   // 首次接管前的完整作者状态。
) {
    match &authored.ambient_occlusion {
        Some(value) if current != Some(value) => {
            commands.entity(entity).insert(value.clone()); // 精确恢复作者 SSAO 质量和厚度。
        }
        None if current.is_some() => {
            commands
                .entity(entity)
                .remove::<ScreenSpaceAmbientOcclusion>(); // 作者原本无 SSAO 时移除全局组件。
        }
        _ => {} // 当前 SSAO 已与作者状态一致时不重复写入。
    }

    let author_ssao_requires_prepasses = authored.ambient_occlusion.is_some(); // 作者 SSAO 自身继续持有两个 required 预通道。
    let wants_depth =
        authored.depth_prepass || author_ssao_requires_prepasses || keep_depth_for_other_feature; // 合并作者状态、TAA 与接触阴影所有权。
    match (wants_depth, has_depth_prepass) {
        (true, false) => {
            commands.entity(entity).insert(DepthPrepass); // 恢复作者或仍活跃 TAA 需要的深度预通道。
        }
        (false, true) => {
            commands.entity(entity).remove::<DepthPrepass>(); // 仅移除本轮 SSAO 临时引入的深度预通道。
        }
        _ => {} // 深度状态已经正确时保持稳定。
    }
    let wants_normals = authored.normal_prepass || author_ssao_requires_prepasses; // 当前只有作者状态和作者 SSAO 可以继续持有法线预通道。
    match (wants_normals, has_normal_prepass) {
        (true, false) => {
            commands.entity(entity).insert(NormalPrepass); // 恢复作者已有法线预通道。
        }
        (false, true) => {
            commands.entity(entity).remove::<NormalPrepass>(); // 移除仅由全局 SSAO 引入的法线预通道。
        }
        _ => {} // 法线状态已经正确时保持稳定。
    }
    commands
        .entity(entity)
        .remove::<AuthoredGameplayAmbientOcclusion>(); // 恢复完成后立即归还作者控制权。
}

#[cfg(test)]
mod tests {
    use super::*; // 复用正式系统、所有权组件和 Bevy SSAO 参数。
    use bevy::{render::camera::TemporalJitter, window::WindowRef}; // 构造窗口目标与 TAA 支撑状态。

    /// 建立只运行 SSAO 应用与命令落地的轻量 ECS 测试应用。
    fn test_app() -> App {
        let mut app = App::new(); // SSAO 组件测试不需要初始化真实 GPU 渲染器。
        app.init_resource::<RenderSettings>(); // 使用默认关闭 SSAO 的兼容基线。
        app.add_systems(Update, (apply_ambient_occlusion, ApplyDeferred).chain()); // 同帧观察 required 组件与恢复结果。
        app
    }

    /// 生成符合正式作用范围的玩法窗口相机。
    fn spawn_gameplay_window_camera(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                Camera3d::default(),                      // 建立三维相机及 required 组件。
                PrimarySceneView,                             // 标记唯一玩法主相机。
                RenderTarget::Window(WindowRef::Primary), // 输出到主窗口。
                Msaa::Off,                                // 满足 Bevy 0.19 SSAO 硬约束。
            ))
            .id() // 返回实体用于逐档断言。
    }

    #[test]
    fn low_high_and_off_restore_absent_author_components() {
        let mut app = test_app(); // 使用正式玩法相机过滤和恢复路径。
        let camera = spawn_gameplay_window_camera(&mut app); // 作者初始没有 SSAO 或预通道。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::Low); // 选择低档并自动归一抗锯齿。
        app.update(); // 安装低档 SSAO 与 required 预通道。
        assert_eq!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .unwrap()
                .quality_level,
            ScreenSpaceAmbientOcclusionQualityLevel::Low
        ); // 低档映射到 Bevy Low。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // SSAO 必须获得深度预通道。
        assert!(app.world().get::<NormalPrepass>(camera).is_some()); // SSAO 必须获得法线预通道。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::High); // 切换到高档。
        app.update(); // 原地更新 SSAO 参数。
        assert_eq!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .unwrap()
                .quality_level,
            ScreenSpaceAmbientOcclusionQualityLevel::High
        ); // 高档映射到 Bevy High。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::Off); // 关闭全局 SSAO。
        app.update(); // 恢复作者原本无组件状态。
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_none()
        ); // 全局 SSAO 必须移除。
        assert!(app.world().get::<DepthPrepass>(camera).is_none()); // 仅由 SSAO 引入的深度预通道必须移除。
        assert!(app.world().get::<NormalPrepass>(camera).is_none()); // 仅由 SSAO 引入的法线预通道必须移除。
        assert!(
            app.world()
                .get::<AuthoredGameplayAmbientOcclusion>(camera)
                .is_none()
        ); // 归还控制权后不得残留作者备份。
    }

    #[test]
    fn only_marker_window_camera_inherits_ssao() {
        let mut app = test_app(); // 使用正式 PrimarySceneView 与窗口双重过滤。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::High); // 在生成相机前选择高档。
        let gameplay = spawn_gameplay_window_camera(&mut app); // 应接管的玩法窗口相机。
        let menu = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 主菜单同样是三维窗口相机。
                RenderTarget::Window(WindowRef::Primary), // 输出到窗口但没有玩法标记。
                Msaa::Off,                                // 即使采样兼容也不应接管。
            ))
            .id(); // 保存菜单相机用于隔离断言。
        let texture = app
            .world_mut()
            .spawn((
                Camera3d::default(),                     // 角色预览或小场景三维相机。
                PrimarySceneView,                            // 即使误带玩法标记也必须由目标边界隔离。
                RenderTarget::None { size: UVec2::ONE }, // 模拟纹理或无颜色目标。
                Msaa::Off,                               // 保持作者采样配置。
            ))
            .id(); // 保存专用相机用于隔离断言。
        app.update(); // 让后生成相机继承当前 SSAO 设置。
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(gameplay)
                .is_some()
        ); // 玩法窗口相机继承高档。
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(menu)
                .is_none()
        ); // 主菜单不承担玩法 SSAO 成本。
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(texture)
                .is_none()
        ); // 纹理目标保持作者配置。
    }

    #[test]
    fn camera_changing_to_texture_target_restores_author_state() {
        let mut app = test_app(); // 使用正式目标切换恢复路径。
        let camera = spawn_gameplay_window_camera(&mut app); // 作者初始没有 SSAO 或预通道。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::High); // 先启用玩法高档 SSAO。
        app.update(); // 安装全局组件并保存作者状态。
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_some()
        ); // 窗口阶段必须接管。

        app.world_mut()
            .entity_mut(camera)
            .insert(RenderTarget::None { size: UVec2::ONE }); // 模拟复用为角色预览或小场景纹理目标。
        app.update(); // 离开窗口目标后立即归还作者状态。
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_none()
        ); // 作者原本无 SSAO，纹理阶段不得残留。
        assert!(app.world().get::<DepthPrepass>(camera).is_none()); // 仅由全局 SSAO 引入的深度预通道必须清理。
        assert!(app.world().get::<NormalPrepass>(camera).is_none()); // 仅由全局 SSAO 引入的法线预通道必须清理。
        assert!(
            app.world()
                .get::<AuthoredGameplayAmbientOcclusion>(camera)
                .is_none()
        ); // 纹理阶段不得残留玩法专用所有权组件。
    }

    #[test]
    fn off_restores_authored_ssao_and_prepasses_exactly() {
        let mut app = test_app(); // 使用正式作者状态备份路径。
        let authored_ssao = ScreenSpaceAmbientOcclusion {
            quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Medium, // 作者使用未公开到 UI 的中档。
            radius: 0.42,
            constant_object_thickness: 0.75, // 作者为当前场景定制物体厚度。
        }; // 构造需要完整恢复的作者参数。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立三维玩法相机。
                PrimarySceneView,                             // 允许全局 SSAO 临时接管。
                RenderTarget::Window(WindowRef::Primary), // 输出到主窗口。
                Msaa::Off,                                // 满足 SSAO 采样约束。
                authored_ssao.clone(),                    // 作者原本已有专用 SSAO。
            ))
            .id(); // 保存实体用于恢复断言。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::High); // 临时应用全局高档。
        app.update(); // 捕获并覆盖作者参数。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::Off); // 归还作者控制权。
        app.update(); // 执行精确恢复。
        assert_eq!(
            app.world().get::<ScreenSpaceAmbientOcclusion>(camera),
            Some(&authored_ssao)
        ); // 作者质量和厚度必须完整恢复。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 作者 SSAO required 深度必须保留。
        assert!(app.world().get::<NormalPrepass>(camera).is_some()); // 作者 SSAO required 法线必须保留。
    }

    #[test]
    fn taa_depth_ownership_is_preserved_when_ssao_turns_off() {
        let mut app = test_app(); // 验证两个后处理共享深度预通道时的所有权边界。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立三维玩法相机。
                PrimarySceneView,                             // 允许 SSAO 接管。
                RenderTarget::Window(WindowRef::Primary), // 输出到主窗口。
                Msaa::Off,                                // TAA 与 SSAO 都使用单采样。
                TemporalAntiAliasing::default(),          // 模拟当前已启用 TAA。
                TaaSupportOwnership {
                    temporal_jitter_offset: None, // 作者原本没有抖动。
                    mip_bias: None,               // 作者原本没有 mip 偏移。
                    depth_prepass: false,         // 深度预通道仅由 TAA 临时引入。
                    motion_vector_prepass: false, // 运动矢量同样由 TAA 临时引入。
                }, // 提供 TAA 前真实作者状态。
            ))
            .id(); // 保存实体用于共享预通道断言。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::Low); // 启用低档 SSAO。
        app.update(); // 记录深度原本不属于作者并增加法线预通道。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::Off); // 只关闭 SSAO，TAA 保持活动。
        app.update(); // 恢复时必须识别 TAA 仍持有深度。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // TAA 深度不得被 SSAO 误删。
        assert!(app.world().get::<NormalPrepass>(camera).is_none()); // 仅由 SSAO 引入的法线应移除。
        assert!(app.world().get::<TemporalJitter>(camera).is_some()); // TAA required 抖动组件保持存在。
    }

    #[test]
    fn incompatible_msaa_state_is_skipped_without_render_component() {
        let mut app = test_app(); // 模拟其他代码绕过强类型 setter 直接制造非法组合。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立玩法窗口相机。
                PrimarySceneView,                             // 满足玩法身份。
                RenderTarget::Window(WindowRef::Primary), // 满足窗口目标。
                Msaa::Sample4,                            // 故意保留与 SSAO 不兼容的四倍采样。
            ))
            .id(); // 保存实体用于安全跳过断言。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .ambient_occlusion_quality = AmbientOcclusionQuality::High; // 绕过公开兼容 setter 模拟异常外部状态。
        app.update(); // 正式运行时系统仍不得把非法 SSAO 提交给 Bevy 提取阶段。
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_none()
        ); // 不兼容状态必须安全跳过。
        assert!(
            app.world()
                .get::<AuthoredGameplayAmbientOcclusion>(camera)
                .is_none()
        ); // 未接管相机不应附加作者备份。
    }

    #[test]
    fn stable_ssao_frame_does_not_rewrite_components() {
        let mut app = test_app(); // 使用正式状态比较路径。
        let camera = spawn_gameplay_window_camera(&mut app); // 生成唯一目标相机。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .set_ambient_occlusion_quality(AmbientOcclusionQuality::Low); // 固定低档。
        app.update(); // 首帧安装 SSAO、预通道与作者备份。
        app.world_mut().clear_trackers(); // 清除首帧变更标记。
        app.update(); // 相同设置稳定帧不得重复写入。
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<ScreenSpaceAmbientOcclusion>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记 SSAO 参数变更",
        );
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<AuthoredGameplayAmbientOcclusion>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复保存 SSAO 作者状态",
        );
    }
}
