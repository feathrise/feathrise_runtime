//! 动态植被首载和热更共用的失败事务。

use bevy::prelude::*;

use super::resources::{DynamicVegetationPhase, DynamicVegetationState, DynamicVegetationStatus};

/// 当前修订失败时清理隐藏候选并停止重试；热更仍保留上一份完整活动根。
pub(super) fn fail_current_revision(
    commands: &mut Commands,
    state: &mut DynamicVegetationState,
    status: &mut DynamicVegetationStatus,
    detail: String,
) {
    if let Some(staging) = state.mesh_chunk_staging.take() {
        commands.entity(staging.root).try_despawn();
    }
    if let Some(candidate) = state.candidate_root.take() {
        commands.entity(candidate.root).try_despawn(); // 失败候选绝不越过后续地形提交门。
    }
    state.candidate_mesh_source = None;
    state.queued_mesh_chunks.clear(); // 失败修订不再继续提交空间块，上一完整根仍保持可见。
    state.desired_mesh_chunks.clear(); // 作者修复或资产事件创建新修订后再从当前相机重算。
    state.task = None;
    state.dirty = false;
    state.attempted_revision = Some((state.generation, state.revision));
    status.phase = DynamicVegetationPhase::Failed;
    status.detail = detail.clone();
    if state.failure_logged_revision != Some(state.revision) {
        error!(
            "动态地图植被修订失败：generation={} revision={} detail={detail}",
            state.generation, state.revision
        );
        state.failure_logged_revision = Some(state.revision);
    }
}
