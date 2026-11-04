//! 普通文字引导一直保持可用；RenderWorld按本帧实际分流结果发布固定GPU暂停信号。
use bevy::prelude::*; // 仅按相机和固定资源更新。
use feathrise_billboard_text::{BILLBOARD_GUIDE_GATE, BillboardTemporalGuideSuppressed}; // 通用引导契约。
pub(super) fn sync_suppression(
    mut commands: Commands,
    cameras: Query<Entity, With<BillboardTemporalGuideSuppressed>>,
) {
    // 清理旧实现提前按请求隐藏的相机标记。
    for entity in &cameras {
        commands
            .entity(entity)
            .remove::<BillboardTemporalGuideSuppressed>();
    } // 普通/回退帧始终可获得引导，GPU信号决定实际分流帧是否写入。
} // 相机状态归还结束。
pub(super) fn publish_gate(
    frame: Res<bevy::diagnostic::FrameCount>,
    gate: Res<super::resources::WorldUiFrameGate>,
    buffers: Res<bevy::render::render_asset::RenderAssets<bevy::render::storage::GpuShaderBuffer>>,
    queue: Res<bevy::render::renderer::RenderQueue>,
) {
    // 在Queue分流完成后、所有GPU pass之前更新。
    if let Some(buffer) = buffers.get(BILLBOARD_GUIDE_GATE.id()) {
        // 分流预检保证需要暂停时缓冲存在。
        let value = if gate.frame == frame.0 && gate.split {
            1.0_f32
        } else {
            0.0_f32
        }; // 未就绪、超预算、关闭均恢复正常引导。
        queue.write_buffer(&buffer.buffer, 0, &value.to_ne_bytes()); // 仅4字节，单队列提交前更新，不等待GPU。
    } // 缺失缓冲时不能成功分流，也无可绘制引导材质。
} // 实际资格发布结束。
