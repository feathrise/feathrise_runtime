//! 图形运行时租约的合并、精确恢复与目标相机隔离回归测试。

mod frame_generation; // 独立帧生成与两种超分租约组合的公共契约回归。
mod water_reflection; // 水面显式租HDR与深度时的作者基线与RTX共享所有权回归。
mod author_view; // Window四球与普通画质的动态单采样交接。

use bevy::{
    anti_alias::fxaa::Fxaa,      // 验证租约强制FXAA以及释放后玩家设置重新接管。
    camera::{Hdr, RenderTarget}, // 区分HDR状态与窗口/纹理渲染目标。
    core_pipeline::tonemapping::Tonemapping, // 验证租约结束后作者色调映射恢复。
    window::WindowRef,           // 构造唯一应被普通画质系统接管的主窗口相机。
};

use super::super::systems::{apply_antialiasing, apply_bloom}; // 复现正式画质系统在租约前收敛玩家设置的顺序。
use super::*; // 直接锁定公开注册语义和私有附件恢复事务。
use crate::{AntiAliasingMode, BloomQuality, RenderSettings}; // 测试租约期间更新的玩家画质状态。

#[test]
fn multiple_leases_merge_and_released_ids_cannot_be_updated() {
    let camera = Entity::from_raw_u32(7).expect("测试实体索引合法"); // 构造无需World的稳定目标实体。
    let mut registry = GraphicsRuntimeLeaseRegistry::default(); // 从无租约状态开始。
    let first = registry.acquire(GraphicsRuntimeRequirements::solari_warmup(camera)); // 第一份要求相机附件但保留标准阴影。
    let second = registry.acquire(
        GraphicsRuntimeRequirements::new(camera)
            .with_standard_shadows_suspended(true)
            .with_dlss_super_resolution_suspended(true),
    ); // 第二份要求暂停标准阴影与独立SR，模拟RR交接声明。
    let merged = registry
        .requirements_for_camera(camera)
        .expect("同一相机的两份租约必须合并");
    assert!(merged.require_hdr); // 第一份HDR要求必须保留。
    assert!(merged.suspend_standard_shadows); // 第二份阴影要求必须保留。
    assert!(merged.suspend_dlss_super_resolution); // RR暂停SR的声明必须参与按或合并。
    assert_eq!(registry.active_lease_count(), 2); // 两个调用方保持独立所有权。
    assert!(registry.release(first)); // 第一份正常释放。
    assert!(!registry.update(first, GraphicsRuntimeRequirements::new(camera))); // 迟到更新不得复活。
    assert!(registry.requirements_for_camera(camera).is_some()); // 第二份仍有效。
    assert!(registry.release(second)); // 最后一份正常释放。
    assert!(registry.requirements_for_camera(camera).is_none()); // 目标不再有任何覆盖。
}

#[test]
fn deferred_raster_compatibility_changes_only_the_three_minimum_requirements() {
    let camera = Entity::from_raw_u32(8).expect("测试辅助相机实体合法");
    let requirements = GraphicsRuntimeRequirements::deferred_raster_compatibility(camera);

    assert_eq!(requirements.target_camera, camera);
    assert!(requirements.force_single_sample);
    assert!(requirements.require_depth_prepass);
    assert!(requirements.require_deferred_prepass);
    assert!(!requirements.force_native_render_scale);
    assert!(!requirements.force_fxaa);
    assert!(!requirements.suspend_fxaa);
    assert!(!requirements.require_hdr);
    assert!(!requirements.require_motion_vector_prepass);
    assert!(!requirements.require_deferred_double_buffer);
    assert!(!requirements.require_depth_double_buffer);
    assert!(!requirements.require_main_texture_storage_binding);
    assert!(!requirements.suspend_ssao);
    assert!(!requirements.suspend_contact_shadows);
    assert!(!requirements.suspend_standard_shadows); // 辅助RTT只取得光栅可见性，任何玩法光追/后处理字段都必须保持关闭。
}

#[test]
fn fsr3_lease_owns_the_compact_color_copy_contract() {
    let camera = Entity::from_raw_u32(9).expect("测试玩法相机实体合法");
    let requirements = GraphicsRuntimeRequirements::fsr3_super_resolution(camera);

    assert_eq!(requirements.target_camera, camera);
    assert!(requirements.force_native_render_scale);
    assert!(requirements.force_single_sample);
    assert!(requirements.suspend_fxaa);
    assert!(requirements.require_hdr);
    assert!(requirements.require_depth_prepass);
    assert!(requirements.require_motion_vector_prepass);
    assert!(requirements.require_main_texture_storage_binding);
    assert!(requirements.require_main_texture_copy_source); // Bevy全尺寸主纹理必须允许GPU复制有效低分辨率区域。
    assert!(requirements.suspend_ssao);
    assert!(requirements.suspend_contact_shadows);
}

#[test]
fn extra_camera_attachments_restore_exactly_after_last_lease() {
    let mut app = App::new(); // 附件所有权测试不初始化真实RenderApp。
    app.init_resource::<GraphicsRuntimeLeaseRegistry>()
        .add_systems(
            Update,
            (
                restore_released_graphics_runtime_leases,
                ApplyDeferred,
                apply_graphics_runtime_leases,
                ApplyDeferred,
            )
                .chain(),
        ); // 复现正式恢复早于应用的顺序。
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), Msaa::Sample4, DepthPrepass))
        .id(); // 作者只显式持有深度，其他Solari附件均缺失。
    let authored_main_texture_usages = *app
        .world()
        .get::<CameraMainTextureUsages>(camera)
        .expect("Camera要求组件应自动提供默认主纹理用途"); // required component也是必须精确恢复的作者基线。
    let lease = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::solari_warmup(camera)); // 请求完整Solari附件。

    app.update(); // 首帧捕获作者状态并补齐租约要求。

    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off)); // 租约显式拥有单采样要求。
    assert!(app.world().get::<DeferredPrepass>(camera).is_some()); // 租约补齐GBuffer。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 作者深度继续存在。
    assert!(app.world().get::<MotionVectorPrepass>(camera).is_some()); // 租约补齐运动矢量。
    assert!(
        app.world()
            .get::<CameraMainTextureUsages>(camera)
            .is_some_and(|usages| usages.0.contains(TextureUsages::STORAGE_BINDING))
    ); // 主纹理用途包含计算写入位。
    assert!(
        app.world()
            .get::<CameraMainTextureUsages>(camera)
            .is_some_and(|usages| usages.0.contains(TextureUsages::COPY_DST))
    ); // NGX清空输出所需的传输目标用途随租约生效。

    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(lease)
    ); // 释放唯一租约。
    app.update(); // 恢复命令同帧落地。

    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample4)); // 作者采样数精确恢复。
    assert!(app.world().get::<DeferredPrepass>(camera).is_none()); // 租约GBuffer已移除。
    assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 作者深度精确保留。
    assert!(app.world().get::<MotionVectorPrepass>(camera).is_none()); // 租约运动矢量已移除。
    assert_eq!(
        app.world()
            .get::<CameraMainTextureUsages>(camera)
            .map(|usages| usages.0),
        Some(authored_main_texture_usages.0)
    ); // Bevy相机required component恢复全部原用途位且不残留STORAGE_BINDING。
    assert!(
        app.world()
            .get::<GraphicsRuntimeLeaseSnapshot>(camera)
            .is_none()
    ); // 私有所有权凭据不泄漏。
}

#[test]
fn deferred_auxiliary_texture_camera_restores_msaa_and_prepasses_exactly() {
    let mut app = App::new();
    app.init_resource::<GraphicsRuntimeLeaseRegistry>()
        .add_systems(
            Update,
            (
                restore_released_graphics_runtime_leases,
                ApplyDeferred,
                apply_graphics_runtime_leases,
                ApplyDeferred,
            )
                .chain(),
        ); // 复现正式租约的恢复和应用边界，不启动窗口或RenderApp。
    let auxiliary = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::None { size: UVec2::ONE },
            Msaa::Sample8, // 作者纹理视图使用独立8×采样，释放后必须原样恢复。
            DepthPrepass,  // 作者原本已有Depth，租约不得在回退时误删。
        ))
        .id();
    let unrelated = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::None { size: UVec2::ONE },
            Msaa::Sample2,
        ))
        .id(); // 未取得票据的离屏相机始终保持作者值。
    let lease = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::deferred_raster_compatibility(
            auxiliary,
        ));

    app.update();
    assert_eq!(app.world().get::<Msaa>(auxiliary), Some(&Msaa::Off));
    assert!(app.world().get::<DepthPrepass>(auxiliary).is_some());
    assert!(app.world().get::<DeferredPrepass>(auxiliary).is_some());
    assert_eq!(
        app.world()
            .get::<PbrDeferredLightingDepthId>(auxiliary)
            .map(PbrDeferredLightingDepthId::get),
        Some(PbrDeferredLightingDepthId::default().get())
    ); // 不等待Bevy下一帧派生系统，首个提取帧已具备默认pass id。
    assert!(app.world().get::<MotionVectorPrepass>(auxiliary).is_none()); // 最小兼容租约不传播Solari运动历史。
    assert_eq!(app.world().get::<Msaa>(unrelated), Some(&Msaa::Sample2));
    assert!(app.world().get::<DepthPrepass>(unrelated).is_none());
    assert!(app.world().get::<DeferredPrepass>(unrelated).is_none());

    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(lease)
    );
    app.update();
    assert_eq!(app.world().get::<Msaa>(auxiliary), Some(&Msaa::Sample8));
    assert!(app.world().get::<DepthPrepass>(auxiliary).is_some());
    assert!(app.world().get::<DeferredPrepass>(auxiliary).is_none());
    assert!(
        app.world()
            .get::<PbrDeferredLightingDepthId>(auxiliary)
            .is_none()
    ); // 作者原本缺失时不得泄漏Bevy只插不删的派生组件。
    assert!(
        app.world()
            .get::<GraphicsRuntimeLeaseSnapshot>(auxiliary)
            .is_none()
    ); // MSAA与预通道一并恢复后归还所有权。
    assert!(
        app.world()
            .get::<GraphicsRuntimeLeaseMsaaSnapshot>(auxiliary)
            .is_none()
    ); // 单采样所有权凭据同样不能泄漏。
    assert!(
        app.world()
            .get::<GraphicsRuntimeLeaseDeferredLightingSnapshot>(auxiliary)
            .is_none()
    ); // Deferred pass-id所有权也必须完整归还。
}

#[test]
fn deferred_auxiliary_camera_preserves_authored_lighting_pass_id() {
    let mut app = App::new();
    app.init_resource::<GraphicsRuntimeLeaseRegistry>()
        .add_systems(
            Update,
            (
                restore_released_graphics_runtime_leases,
                ApplyDeferred,
                apply_graphics_runtime_leases,
                ApplyDeferred,
            )
                .chain(),
        );
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Msaa::Sample4,
            PbrDeferredLightingDepthId::new(7),
        ))
        .id();
    let lease = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::deferred_raster_compatibility(
            camera,
        ));

    app.update();
    assert_eq!(
        app.world()
            .get::<PbrDeferredLightingDepthId>(camera)
            .map(PbrDeferredLightingDepthId::get),
        Some(7)
    ); // 租约只补可见路径，不覆盖作者自定义Deferred pass。

    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(lease)
    );
    app.update();
    assert_eq!(
        app.world()
            .get::<PbrDeferredLightingDepthId>(camera)
            .map(PbrDeferredLightingDepthId::get),
        Some(7)
    ); // 释放后存在性和具体u8值均精确恢复。
}

#[test]
fn msaa_snapshot_is_recaptured_after_single_sample_requirement_reactivates() {
    let mut app = App::new();
    app.init_resource::<GraphicsRuntimeLeaseRegistry>()
        .add_systems(
            Update,
            (
                restore_released_graphics_runtime_leases,
                ApplyDeferred,
                apply_graphics_runtime_leases,
                ApplyDeferred,
            )
                .chain(),
        );
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), Msaa::Sample4))
        .id();
    let lease = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::deferred_raster_compatibility(
            camera,
        ));
    app.update();
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off));

    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .update(lease, GraphicsRuntimeRequirements::new(camera))
    ); // 同一票据保留其他附件所有权，但暂时不再要求单采样。
    app.update();
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample4));
    assert!(
        app.world()
            .get::<GraphicsRuntimeLeaseMsaaSnapshot>(camera)
            .is_none()
    );

    app.world_mut().entity_mut(camera).insert(Msaa::Sample8); // 模拟租约未拥有采样数期间作者/画质的新权威值。
    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .update(
                lease,
                GraphicsRuntimeRequirements::deferred_raster_compatibility(camera),
            )
    );
    app.update();
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off));
    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(lease)
    );
    app.update();
    assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample8)); // 第二轮必须恢复新基线而不是首次Sample4。
}

#[test]
fn solari_lease_isolates_texture_camera_and_applies_latest_player_settings_on_release() {
    let mut app = App::new();
    app.init_resource::<RenderSettings>()
        .init_resource::<GraphicsRuntimeLeaseRegistry>()
        .add_systems(
            Update,
            (
                restore_released_graphics_runtime_leases,
                ApplyDeferred,
                apply_antialiasing,
                apply_bloom,
                ApplyDeferred,
                apply_graphics_runtime_leases,
                ApplyDeferred,
            )
                .chain(),
        ); // 复现正式插件“先恢复、玩家设置收敛、最后应用租约”的所有权顺序。
    let gameplay_camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),                      // 构造玩法三维相机及required组件。
            RenderTarget::Window(WindowRef::Primary), // 普通画质系统只接管主窗口目标。
            Msaa::Sample4,                            // 作者抗锯齿基线。
            Tonemapping::Reinhard,                    // 作者色调映射基线。
        ))
        .id();
    let authored_main_texture_usages = *app
        .world()
        .get::<CameraMainTextureUsages>(gameplay_camera)
        .expect("Camera要求组件应自动提供默认主纹理用途"); // 租约释放后必须恢复这一完整位集合。
    let texture_camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),                     // 模拟角色预览三维相机。
            RenderTarget::None { size: UVec2::ONE }, // 无窗口纹理目标不受普通画质接管。
            Msaa::Sample2,                           // 纹理相机独立抗锯齿。
            Hdr,                                     // 纹理相机独立HDR。
            Tonemapping::AcesFitted,                 // 纹理相机独立色调映射。
        ))
        .id(); // 模拟角色预览等具有独立作者状态的纹理相机。
    let lease = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::solari_warmup(gameplay_camera));

    app.update();

    assert_eq!(app.world().get::<Msaa>(gameplay_camera), Some(&Msaa::Off));
    assert!(app.world().get::<Fxaa>(gameplay_camera).is_some());
    assert!(app.world().get::<Hdr>(gameplay_camera).is_some());
    assert!(
        app.world()
            .get::<DeferredPrepass>(gameplay_camera)
            .is_some()
    );
    assert!(app.world().get::<DepthPrepass>(gameplay_camera).is_some());
    assert!(
        app.world()
            .get::<MotionVectorPrepass>(gameplay_camera)
            .is_some()
    );
    assert!(
        app.world()
            .get::<CameraMainTextureUsages>(gameplay_camera)
            .is_some_and(|usage| usage.0.contains(TextureUsages::STORAGE_BINDING))
    );
    assert!(
        app.world()
            .get::<CameraMainTextureUsages>(gameplay_camera)
            .is_some_and(|usage| usage.0.contains(TextureUsages::COPY_DST))
    );
    assert_eq!(
        app.world().get::<Msaa>(texture_camera),
        Some(&Msaa::Sample2)
    );
    assert!(app.world().get::<Fxaa>(texture_camera).is_none());
    assert!(app.world().get::<Hdr>(texture_camera).is_some());
    assert_eq!(
        app.world().get::<Tonemapping>(texture_camera),
        Some(&Tonemapping::AcesFitted)
    ); // 非目标纹理相机所有状态保持作者值。

    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .update(
                lease,
                GraphicsRuntimeRequirements::solari_warmup(gameplay_camera)
                    .with_fxaa_suspended(true),
            )
    ); // 模拟DLSS RR接管最终抗锯齿，但继续保留Solari严格单采样要求。
    app.update(); // 让更新后的租约在同一目标相机生效。
    assert_eq!(app.world().get::<Msaa>(gameplay_camera), Some(&Msaa::Off)); // RR路径仍禁止MSAA。
    assert!(app.world().get::<Fxaa>(gameplay_camera).is_none()); // RR输出不能再叠加FXAA。
    assert!(app.world().get::<Fxaa>(texture_camera).is_none()); // 纹理相机继续保持隔离。

    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .update(
                lease,
                GraphicsRuntimeRequirements::solari_warmup(gameplay_camera),
            )
    ); // 模拟关闭RR后回到基础Solari抗锯齿路径。
    app.update(); // 恢复租约强制的单帧FXAA。
    assert!(app.world().get::<Fxaa>(gameplay_camera).is_some()); // 基础Solari继续拥有边缘托底。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .anti_aliasing = AntiAliasingMode::Msaa8; // 租约期间修改玩家设置，只改变释放后的目标。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .bloom_quality = BloomQuality::Off;
    app.update();
    assert_eq!(app.world().get::<Msaa>(gameplay_camera), Some(&Msaa::Off));
    assert!(app.world().get::<Hdr>(gameplay_camera).is_some()); // 租约仍优先保证Solari相机契约。

    assert!(
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(lease)
    );
    app.update();

    assert_eq!(
        app.world().get::<Msaa>(gameplay_camera),
        Some(&Msaa::Sample8)
    ); // 释放后采用租约期间最新玩家档位，而不是回滚到旧设置值。
    assert!(app.world().get::<Fxaa>(gameplay_camera).is_none());
    assert!(app.world().get::<Hdr>(gameplay_camera).is_none());
    assert_eq!(
        app.world().get::<Tonemapping>(gameplay_camera),
        Some(&Tonemapping::Reinhard)
    ); // 租约临时HDR和色调映射结束后精确恢复作者状态。
    assert!(
        app.world()
            .get::<DeferredPrepass>(gameplay_camera)
            .is_none()
    );
    assert!(app.world().get::<DepthPrepass>(gameplay_camera).is_none());
    assert!(
        app.world()
            .get::<MotionVectorPrepass>(gameplay_camera)
            .is_none()
    );
    assert_eq!(
        app.world()
            .get::<CameraMainTextureUsages>(gameplay_camera)
            .map(|usages| usages.0),
        Some(authored_main_texture_usages.0)
    ); // required component保持存在，但STORAGE_BINDING、COPY_DST和其他租约改动已精确撤销。
    assert_eq!(
        app.world().get::<Msaa>(texture_camera),
        Some(&Msaa::Sample2)
    );
    assert!(app.world().get::<Hdr>(texture_camera).is_some());
}
