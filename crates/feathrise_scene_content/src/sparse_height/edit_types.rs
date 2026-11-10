//! 稀疏局部编辑的完整候选、采用结果与回滚契约；不包含ECS或文件副作用。
use super::author::SparseTerrainHeightChange;
use super::{SparseHeightTile, SparseHeightTileKey};
use crate::SceneId;
use std::sync::Arc;

/// 同一作者修订的局部候选；数组复制和逐格校验必须在后台完成。
#[derive(Debug)]
pub struct SparseTerrainPreparedEdit {
    pub(super) scene_id: SceneId,
    pub(super) source_revision: u64,
    pub(super) source_session: u64,
    pub(super) changes: Arc<Vec<SparseTerrainHeightChange>>,
    pub(super) writable_tiles: Vec<SparseHeightTile>,
    pub(super) live_tiles: Vec<Arc<SparseHeightTile>>,
}

/// 采用只替换有限页的所有权，旧页可供同一次作者事务失败时原样恢复。
pub struct SparseTerrainEditRollback {
    pub(super) source_revision: u64,
    pub(super) adopted_revision: u64,
    pub(super) source_session: u64,
    pub(super) previous: Vec<(SparseHeightTileKey, Option<SparseHeightTile>, bool)>,
}

/// 完整采用后，局部差分进入历史，实时页进入场景；回滚信息不长期保存。
pub struct SparseTerrainAppliedEdit {
    pub changes: Arc<Vec<SparseTerrainHeightChange>>,
    pub live_tiles: Vec<Arc<SparseHeightTile>>,
    pub rollback: SparseTerrainEditRollback,
}
