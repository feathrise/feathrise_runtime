//! Bevy 0.19 运行时关闭运动矢量预通道后的背景管线兼容层。

use bevy::{
    core_pipeline::prepass::{
        BackgroundMotionVectorsBindGroup, BackgroundMotionVectorsPipelineId, MotionVectorPrepass,
        NoBackgroundMotionVectors,
    }, // 同时核对真实预通道资格与 Bevy 派生的背景绘制状态。
    prelude::*, // RenderWorld 实体、查询与延迟命令类型。
};

/// 清理已经失去运动矢量附件、却仍会被预通道节点执行的背景绘制状态。
///
/// Bevy 0.19 会随主世界组件同步移除 `MotionVectorPrepass`，但不会同步移除此前由
/// `BackgroundMotionVectorsPlugin` 派生的管线 ID 和绑定组。运行时从 TAA 切到 FXAA/MSAA
/// 时，这会让无颜色附件的预通道继续绑定要求 `Rg16Float` 附件的旧管线。系统只枚举仍
/// 持有任一派生组件的少量视图，并在真实预通道缺失或作者显式退出背景写入时清理一次。
#[allow(clippy::type_complexity)] // RenderWorld 过滤器必须同时覆盖两类可能独立残留的派生组件。
pub(crate) fn cleanup_stale_background_motion_vector_state(
    mut commands: Commands, // 清理必须配合注册处紧随的 ApplyDeferred，在预通道执行前落地。
    views: Query<
        (
            Entity,                         // 当前 RenderWorld 视图实体。
            Has<MotionVectorPrepass>,       // 真实运动矢量附件是否仍会在本帧创建。
            Has<NoBackgroundMotionVectors>, // 作者是否显式禁止背景运动矢量写入。
        ),
        Or<(
            With<BackgroundMotionVectorsPipelineId>, // 上一帧可能残留的专用化管线。
            With<BackgroundMotionVectorsBindGroup>,  // 上一帧可能残留的视图绑定组。
        )>,
    >,
) {
    for (entity, has_motion_vectors, disables_background) in &views {
        if has_motion_vectors && !disables_background {
            continue; // 正常 TAA 或作者运动模糊视图继续使用 Bevy 的背景运动矢量实现。
        }
        commands.entity(entity).remove::<(
            BackgroundMotionVectorsPipelineId, // 没有匹配附件时禁止预通道节点选择旧管线。
            BackgroundMotionVectorsBindGroup,  // 同时释放不再使用的上一帧 GPU 绑定组。
        )>(); // 两个组件都不存在时查询不会命中，因此稳定帧没有重复命令。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 直接验证正式 RenderWorld 兼容系统。
    use bevy::render::render_resource::CachedRenderPipelineId; // 构造无需 GPU 的派生管线占位 ID。

    /// 建立带显式命令落地点的轻量 RenderWorld 调度替身。
    fn test_app() -> App {
        let mut app = App::new(); // 此状态机测试不创建窗口、纹理或 GPU 设备。
        app.add_systems(
            Update,
            (
                cleanup_stale_background_motion_vector_state, // 执行正式清理逻辑。
                ApplyDeferred, // 模拟插件在 PrepareViews 中保证的同帧命令落地。
            )
                .chain(),
        ); // 完成测试调度注册。
        app
    }

    #[test]
    fn removes_stale_state_but_preserves_active_motion_vector_view() {
        let mut app = test_app(); // 建立轻量世界。
        let stale_view = app
            .world_mut()
            .spawn(BackgroundMotionVectorsPipelineId(
                CachedRenderPipelineId::INVALID, // ID 数值不参与清理判定。
            ))
            .id(); // 模拟 TAA 刚关闭但 Bevy 派生状态仍在的视图。
        let active_view = app
            .world_mut()
            .spawn((
                MotionVectorPrepass, // 正常 TAA/运动模糊仍需要运动矢量附件。
                BackgroundMotionVectorsPipelineId(CachedRenderPipelineId::INVALID), // 对应管线必须保留。
            ))
            .id(); // 保存正常视图用于断言。
        let opted_out_view = app
            .world_mut()
            .spawn((
                MotionVectorPrepass,       // 附件仍存在。
                NoBackgroundMotionVectors, // 作者显式禁止背景写入。
                BackgroundMotionVectorsPipelineId(CachedRenderPipelineId::INVALID), // 模拟退出前残留。
            ))
            .id(); // 保存作者退出视图用于断言。

        app.update(); // 清理命令必须在本次调度内落地。

        assert!(
            app.world()
                .get::<BackgroundMotionVectorsPipelineId>(stale_view)
                .is_none(),
            "失去运动矢量附件后必须同帧清理旧背景管线",
        ); // 锁定用户日志中的无附件/旧管线崩溃路径。
        assert!(
            app.world()
                .get::<BackgroundMotionVectorsPipelineId>(active_view)
                .is_some(),
            "仍有运动矢量附件的正常视图不得被清理",
        ); // 避免破坏继续启用的 TAA 或作者运动模糊。
        assert!(
            app.world()
                .get::<BackgroundMotionVectorsPipelineId>(opted_out_view)
                .is_none(),
            "作者显式退出背景运动矢量时不得保留派生管线",
        ); // 同时覆盖 Bevy 公共退出标记的运行时切换。

        app.world_mut()
            .entity_mut(active_view)
            .remove::<MotionVectorPrepass>(); // 精确模拟玩家从 TAA 切换到 67% + FXAA 的提取结果。
        app.update(); // 旧派生管线必须在下一次预通道执行前消失。
        assert!(
            app.world()
                .get::<BackgroundMotionVectorsPipelineId>(active_view)
                .is_none(),
            "运行时关闭 TAA 后必须清理此前正常视图的旧背景管线",
        ); // 锁定从有效 TAA 状态切出的实际回归路径。
    }
}
