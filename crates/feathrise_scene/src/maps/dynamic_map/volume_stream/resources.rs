//! 作者与运行时共用的背压资源，以及固定大小窗口状态；不存整层块目录。
use super::types::{Identity, Pending, Work};
use bevy::{prelude::*, tasks::Task};
use feathrise_map_content::{MapId, MapPlacementRecord};
use feathrise_map_terrain::volume::sparse::paged::VolumeDisplayLod;
use std::sync::Arc;

/// 编辑器只提交当前小型作者版本；新笔与运行时窗口任务共用一份背压门。
#[derive(Resource, Default, Clone)]
pub struct TerrainVolumeStreamControl {
    pub draft: Option<(MapId, MapPlacementRecord)>,
    pub excluded: Option<String>,
    pub editing: bool,
    pub native_busy: bool,
    pub retirement_busy: bool, // 作者与运行时共同等候退役信用，不在主帧析构大作者字典。
    pub focus_override: Option<Vec3>,
    pub focus_anchor: Option<Vec3>,
    pub physics_focus: Option<Vec3>,
    /// 只由已经采用的高度显示计划更新；作者与运行时读取同一小型快照。
    pub display_lods: Arc<[VolumeDisplayLod]>,
    /// 插件finish从真实RenderDevice取得；无渲染的测试宿主不伪造显存能力。
    pub gpu_storage_limit: Option<u64>,
}

#[derive(Resource, Default)]
pub struct TerrainVolumeStreamStatus {
    pub map_id: Option<MapId>,
    pub ready: bool,
    pub detail: String,
}

#[derive(Resource, Default)]
pub(super) struct Runtime {
    pub scope: Option<(MapId, Entity)>,
    pub revision: Option<u64>,
    pub next: usize,
    pub scanning: bool,
    pub records: Vec<MapPlacementRecord>,
    pub work: Option<Work>,
    pub pending: Option<Pending>,
    pub active: Option<Identity>,
    pub observed_root: Option<Entity>,
    pub failed: Option<Identity>,
    pub release: Option<Task<()>>,
    pub material: Option<Handle<StandardMaterial>>,
}
