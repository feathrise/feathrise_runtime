//! 全局 Deferred 材质桥的离屏辅助 PBR 视图兼容租约。
//!
//! 【功能设计】材质的 `opaque_render_method` 跨相机共享。Solari 会话把世界 PBR 材质临时
//! 准备为 Deferred 后，小场景、全屏当前场景和角色预览若仍是纯 Forward 视图，就只会留下
//! 清屏色。本模块只为显式标记的 `RenderTarget::Image` 三维相机声明单采样 Depth/Deferred
//! 预通道；不传播 Solari、DLSS、HDR、运动历史、双缓冲或窗口后处理。

use bevy::{camera::RenderTarget, prelude::*}; // 离屏目标身份与有限相机查询。
use feathrise_scene_core::view::{DeferredRasterAuxiliaryCamera, PrimarySceneView}; // 明确辅助视图并排除玩法主相机。
use feathrise_settings::{GraphicsRuntimeLeaseRegistry, GraphicsRuntimeRequirements}; // 复用作者状态快照与精确回退。

use crate::resources::{
    DeferredAuxiliaryCameraLeaseState, DeferredRasterBridgeState, RaytracingRuntime,
};

/// 在材质桥安装帧取得辅助视图租约，并在回退、标记移除或相机销毁后立即释放。
#[allow(clippy::type_complexity)] // 相机资格由三项互斥标记与目标类型共同组成，必须在同一 Query 中判定。
pub(crate) fn maintain_deferred_auxiliary_camera_leases(
    bridge: Res<DeferredRasterBridgeState>, // 只有全局材质确已切到Deferred时才需要辅助视图兼容。
    runtime: Res<RaytracingRuntime>,        // 任一同帧回退请求必须阻止新增租约并释放现有所有权。
    cameras: Query<
        (Entity, &RenderTarget),
        (
            With<Camera3d>,
            With<DeferredRasterAuxiliaryCamera>,
            Without<PrimarySceneView>,
        ),
    >, // 当前三台正式消费者；未标记纹理相机和玩法窗口相机保持作者配置。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 租约真正写相机组件前仍由画质插件统一合并。
    mut state: ResMut<DeferredAuxiliaryCameraLeaseState>, // 按实体保存票据，稳定帧不重复更新。
) {
    let bridge_active = bridge.installed && runtime.pending_rollback.is_none();

    loop {
        let stale = state.leases.iter().find_map(|(entity, lease)| {
            let still_eligible = bridge_active
                && cameras
                    .get(*entity)
                    .is_ok_and(|(_, target)| matches!(target, RenderTarget::Image(_)));
            (!still_eligible).then_some((*entity, *lease))
        });
        let Some((entity, lease)) = stale else {
            break; // 稳定帧只读遍历，不取得ResMut写权限，也不刷新资源change tick。
        };
        state.leases.remove(&entity);
        leases.release(lease); // 释放帧由通用租约系统精确恢复MSAA、pass id和作者预通道。
    }
    if !bridge_active {
        return; // 关闭与回退态不扫描或写入任何相机组件。
    }

    for (entity, target) in &cameras {
        if !matches!(target, RenderTarget::Image(_)) || state.leases.contains_key(&entity) {
            continue; // 只允许真实纹理视图；稳定相机不重复取得票据。
        }
        let lease = leases.acquire(GraphicsRuntimeRequirements::deferred_raster_compatibility(
            entity,
        ));
        state.leases.insert(entity, lease); // 新建/重建辅助相机在本帧PostUpdate提取前补齐附件。
    }
}

#[cfg(test)]
mod tests;
