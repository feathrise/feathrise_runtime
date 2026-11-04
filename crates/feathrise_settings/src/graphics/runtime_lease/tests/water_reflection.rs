//! 水面SSR显式租用HDR与深度，验证F12色空间、实际附件合并及精确归还。

use super::super::*; // 直接运行正式恢复/应用系统，不扩大生产API。
use super::apply_bloom; // 正式HDR租约通过已有后处理所有权系统应用。
use crate::{BloomQuality, RenderSettings};
use bevy::camera::{Hdr, RenderTarget}; // 同时验证F12纹理目标和窗口目标。
use bevy::core_pipeline::tonemapping::Tonemapping; // 作者色调映射需要精确归还。
use bevy::post_process::bloom::Bloom; // 当前Bevy版本的Bloom模块。 // 禁用窗口Bloom以单独检查本功能HDR所有权。

#[test] // 作者原有深度和RTX临时深度都必须保留正确所有权。
fn water_reflection_hdr_depth_lease_restores_author_and_preserves_raytracing() {
    // 不启动渲染或游戏。
    for (authored_depth, authored_hdr, image_target) in [
        (false, false, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
        (false, true, true),
        (true, true, true),
    ] {
        // 独立覆盖作者深度/HDR组合与F12纹理目标。
        let mut app = App::new(); // 最小正式租约宿主。
        app.init_resource::<RenderSettings>(); // HDR系统仍读取普通Bloom设置。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::Off; // 隔离水面运行时HDR要求。
        app.init_resource::<GraphicsRuntimeLeaseRegistry>()
            .add_systems(
                Update,
                (
                    restore_released_graphics_runtime_leases,
                    ApplyDeferred,
                    apply_bloom,
                    ApplyDeferred,
                    apply_graphics_runtime_leases,
                    ApplyDeferred,
                )
                    .chain(),
            ); // 恢复、HDR及附件应用与生产一致。
        let camera = app
            .world_mut()
            .spawn((Camera3d::default(), Msaa::Sample4, Tonemapping::Reinhard))
            .id(); // SSR不应永久修改作者色调映射或采样。
        if image_target {
            app.world_mut()
                .entity_mut(camera)
                .insert(RenderTarget::Image(Handle::<Image>::default().into()));
        } // 不需要真实GPU图片即可验证F12入口资格。
        if authored_depth {
            app.world_mut().entity_mut(camera).insert(DepthPrepass);
        } // 明确记录作者深度基线。
        if authored_hdr {
            app.world_mut().entity_mut(camera).insert(Hdr);
        } // HDR与深度作者所有权彼此独立。
        let authored_bloom = image_target && authored_hdr; // 部分F12作者自身已经使用Bloom，租约不能取得它的所有权。
        if authored_bloom {
            app.world_mut().entity_mut(camera).insert(Bloom::default());
        } // 保留独立作者后处理。
        if image_target {
            app.world_mut()
                .resource_mut::<RenderSettings>()
                .graphics
                .bloom_quality = BloomQuality::High;
        } // 即使窗口请求高Bloom，未开启的F12也不能跟随。
        let unrelated = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Image(Handle::<Image>::default().into()),
            ))
            .id(); // 普通辅助RTT没有本功能显式租约。
        let mut requirements = GraphicsRuntimeRequirements::new(camera); // 与水面生产代码完全相同的最小通用请求。
        requirements.require_depth_prepass = true; // 不申请SSR组件或其他附件。
        requirements.require_hdr = true; // 与水PBR同在线性HDR内合成，避免F12双重色调映射。
        let reflection = app
            .world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .acquire(requirements); // 第一个独立所有者。
        app.update(); // 正式系统补齐深度。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 水面能取得深度输入。
        assert!(app.world().get::<Hdr>(camera).is_some()); // F12纹理目标也真正进入HDR。
        assert!(app.world().get::<Hdr>(unrelated).is_none()); // 其他RTT不被隐式传播。
        assert_eq!(app.world().get::<Bloom>(camera).is_some(), authored_bloom); // 显式HDR不新增或移除作者Bloom。
        assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample4)); // 最小要求保持作者采样。
        assert!(app.world().get::<DeferredPrepass>(camera).is_none()); // 没有顺带创建GBuffer。
        assert!(app.world().get::<MotionVectorPrepass>(camera).is_none()); // 没有顺带创建运动历史。
        let raytracing = app
            .world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .acquire(GraphicsRuntimeRequirements::solari_warmup(camera)); // RTX随后加入同一相机。
        app.update(); // 合并RTX要求。
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(reflection); // 低档/退出时水面只归还自身租约。
        app.update(); // RTX仍拥有所有必需附件。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 不删除RTX仍需要的深度。
        assert!(app.world().get::<Hdr>(camera).is_some()); // RTX仍持有HDR，SSR释放不能关闭。
        assert!(app.world().get::<DeferredPrepass>(camera).is_some()); // 不删除RTX GBuffer。
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(raytracing); // 最后一个所有者退出。
        app.update(); // 正式恢复最初作者基线。
        assert_eq!(
            app.world().get::<DepthPrepass>(camera).is_some(),
            authored_depth
        ); // 无作者深度则释放，有则保留。
        assert_eq!(app.world().get::<Hdr>(camera).is_some(), authored_hdr); // 精确归还作者原HDR状态。
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::Reinhard)
        ); // 恢复原有作者显示变换。
        assert_eq!(app.world().get::<Bloom>(camera).is_some(), authored_bloom); // 归还HDR也不改变作者Bloom。
        assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample4)); // RTX退出后恢复作者MSAA。
        assert!(app.world().get::<DeferredPrepass>(camera).is_none()); // RTX临时附件不泄漏。
        assert_eq!(
            app.world()
                .resource::<GraphicsRuntimeLeaseRegistry>()
                .active_lease_count(),
            0
        ); // 两个票据全部归还。
        if image_target {
            // F12结束租约后必须彻底归还作者控制权。
            assert!(
                app.world()
                    .get::<crate::components::AuthoredWindowPostProcessing>(camera)
                    .is_none()
            ); // 旧快照不能在后续稳定帧覆盖作者。
            app.world_mut().entity_mut(camera).remove::<Bloom>(); // 作者主动改变自己的后处理，不受窗口高Bloom设置影响。
            if authored_hdr {
                app.world_mut().entity_mut(camera).remove::<Hdr>();
            } else {
                app.world_mut().entity_mut(camera).insert(Hdr);
            } // 下一轮作者HDR基线与第一轮相反。
            app.world_mut()
                .entity_mut(camera)
                .insert(Tonemapping::AcesFitted); // 作者选择新的显示变换。
            app.update(); // 空闲帧不能由已结束租约回写旧值。
            assert_eq!(app.world().get::<Hdr>(camera).is_some(), !authored_hdr); // 作者新HDR状态保持有效。
            assert_eq!(
                app.world().get::<Tonemapping>(camera),
                Some(&Tonemapping::AcesFitted)
            ); // 空闲期不回写第一轮Reinhard。
            let second = app
                .world_mut()
                .resource_mut::<GraphicsRuntimeLeaseRegistry>()
                .acquire(requirements); // 第二轮捕获此时真正的作者状态。
            app.update(); // 正式租约重新取得HDR。
            assert!(app.world().get::<Hdr>(camera).is_some()); // 第二轮确实生效。
            app.world_mut()
                .resource_mut::<GraphicsRuntimeLeaseRegistry>()
                .release(second); // 关闭第二轮。
            app.update(); // 必须恢复新基线。
            assert_eq!(app.world().get::<Hdr>(camera).is_some(), !authored_hdr); // 不恢复第一轮旧HDR。
            assert_eq!(
                app.world().get::<Tonemapping>(camera),
                Some(&Tonemapping::AcesFitted)
            ); // 不恢复第一轮旧色调映射。
            assert!(
                app.world()
                    .get::<crate::components::AuthoredWindowPostProcessing>(camera)
                    .is_none()
            ); // 第二轮也完全释放快照。
        } // 完成F12两轮所有权交接。
    } // 完成两种作者基线。
} // 完成实际附件所有权回归。
