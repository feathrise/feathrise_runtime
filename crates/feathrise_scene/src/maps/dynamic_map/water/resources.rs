//! 动态地图绘制水域的阶段、诊断和有界后台任务状态。

use std::collections::{BTreeMap, VecDeque}; // 水域句柄确定排序，GPU候选逐帧弹出一张完成Mesh。
use std::sync::Arc; // Clipmap作者草稿共享R8，不在主线程复制4096²画布。

use bevy::prelude::*; // Entity、Handle、Mesh与Resource。
use bevy::tasks::Task; // 同一地图最多一个全水域建网格任务。
use feathrise_map_content::{MapId, MapWaterAsset, MapWaterMaskAsset}; // R8作者资产和稳定地图身份进入任务状态。

use crate::shared::large_map::{
    BuiltWaterSurface, PaintedWaterSurfaceSpawn, StagedWaterSurface, WaterGpuUploadTicket,
}; // 与正式大地图共享后台完成值、不可见GPU候选和正式资产所有权。

/// 后台任务返回地图、输入代次和全部层的原子结果。
pub(super) type DynamicWaterTaskResult = (
    MapId,                                  // 地图身份阻止切图后的迟到结果进入新场景。
    u64, // 输入代次覆盖水JSON、全部遮罩、清单参数与内存高度内容。
    u64, // GLB派生地形代次防止水面采用旧高度快照。
    Result<Vec<BuiltWaterSurface>, String>, // 静水、河流或瀑布任一失败时整批拒绝采用。
);

/// 动态地图水域运行时阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicWaterPhase {
    /// 当前目录未声明水域文件。
    #[default]
    Disabled,
    /// JSON、无Mip R8作者源或GLB派生内存高度仍在异步准备。
    Loading,
    /// 已取得完整R8快照，后台正在构建水面网格。
    Building,
    /// 全部非空水域层已原子采用。
    Ready,
    /// 首次加载无水面或热更保留上一有效水面，并公开具体失败项。
    Failed,
}

/// F3与F12可读取的水域热加载证据。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicWaterStatus {
    /// 当前阶段。
    pub phase: DynamicWaterPhase,
    /// 当前目标地图。
    pub map_id: Option<MapId>,
    /// 最近一次完整成功采用修订。
    pub revision: u64,
    /// 当前地图全部JSON、R8与地形派生变化合并后的期望输入代次；Disabled时重置为零。
    pub desired_generation: u64,
    /// 最近一次原子安装的输入代次；仅Ready且等于期望代次时证明当前输入全部落稳。
    pub applied_generation: u64,
    /// Clipmap当前水材质绑定的地图/高度/视图修订；视图移动只更新此值，不推进内容代次。
    pub height_binding_revision: [u64; 3],
    /// 当前输入来自哪个F12内存草稿修订；None表示正式磁盘JSON/R8/KTX2。
    pub input_draft_revision: Option<u64>,
    /// 当前已安装的非空水体数量；兼容字段名同时统计静水、河流与瀑布。
    pub layer_count: usize,
    /// 当前已安装水面三角形总数。
    pub triangle_count: usize,
    /// 具体等待项、成功预算或失败原因。
    pub detail: String,
}

/// F12完整Clipmap水网快照；定义、CPU R8和全部GPU图片只能整批发布。
#[derive(Clone)]
pub struct DynamicClipmapWaterDraftSnapshot {
    pub source_revision: u64,
    pub definition: MapWaterAsset,
    pub sources: BTreeMap<String, Arc<[u8]>>,
    pub masks: BTreeMap<String, Handle<Image>>,
}

/// 编辑器关闭后仍由正式Clipmap水运行时消费最后一份同图草稿，直到切图或草稿卸载。
#[derive(Resource, Default, Clone)]
pub struct DynamicClipmapWaterDraft {
    revision: u64,
    snapshot: Option<std::sync::Arc<DynamicClipmapWaterDraftSnapshot>>,
    root_revision: u64,
    root_snapshot: Option<std::sync::Arc<DynamicClipmapWaterDraftSnapshot>>, // 地图独立静水单独停放，不能借当前地形画布的作者信用。
}

impl DynamicClipmapWaterDraft {
    pub fn revision_for_map(&self, map_id: &MapId) -> Option<u64> {
        self.for_map(map_id).map(|_| self.revision)
    }

    pub fn for_map(&self, map_id: &MapId) -> Option<&DynamicClipmapWaterDraftSnapshot> {
        self.snapshot
            .as_deref()
            .filter(|snapshot| snapshot.definition.map_id == *map_id)
    }

    pub fn publish(&mut self, snapshot: DynamicClipmapWaterDraftSnapshot) {
        self.snapshot = Some(std::sync::Arc::new(snapshot));
        self.revision = self.revision.wrapping_add(1).max(1);
    }

    pub fn clear(&mut self) {
        if self.snapshot.take().is_some() {
            self.revision = self.revision.wrapping_add(1).max(1);
        }
    }

    /// 根水网与当前选中的地形水网有独立发布槽，切换作者对象不改变其水面。
    pub fn root_for_map(&self, map_id: &MapId) -> Option<&DynamicClipmapWaterDraftSnapshot> {
        self.root_snapshot.as_deref().filter(|snapshot| snapshot.definition.map_id == *map_id)
    }

    pub fn root_revision_for_map(&self, map_id: &MapId) -> Option<u64> {
        self.root_for_map(map_id).map(|_| self.root_revision)
    }

    pub fn publish_root(&mut self, snapshot: DynamicClipmapWaterDraftSnapshot) {
        self.root_snapshot = Some(std::sync::Arc::new(snapshot));
        self.root_revision = self.root_revision.wrapping_add(1).max(1);
    }

    /// 正式保存交接只撤回根覆盖，仍在编辑的地形水层继续读取自己的作者版本。
    pub fn clear_root(&mut self) {
        if self.root_snapshot.take().is_some() {
            self.root_revision = self.root_revision.wrapping_add(1).max(1);
        }
    }

    /// 已完整发布的定义与CPU像素共享给后台消费者，帧内不深拷贝水网或R8数组。
    pub fn shared_for_map(&self, map_id: &MapId) -> Option<std::sync::Arc<DynamicClipmapWaterDraftSnapshot>> {
        self.snapshot.as_ref().filter(|snapshot| snapshot.definition.map_id == *map_id).cloned()
    }

    pub fn shared_root_for_map(&self, map_id: &MapId) -> Option<std::sync::Arc<DynamicClipmapWaterDraftSnapshot>> {
        self.root_snapshot.as_ref().filter(|snapshot| snapshot.definition.map_id == *map_id).cloned()
    }

    /// 作者焦点切换只交换所属地形，地图独立静水始终留在全局根槽。
    pub(crate) fn swap_terrain_channel(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.revision, &mut other.revision);
        std::mem::swap(&mut self.snapshot, &mut other.snapshot);
    }

    pub(crate) fn terrain_channel_clone(&self) -> Self {
        Self { revision: self.revision, snapshot: self.snapshot.clone(), ..default() }
    }
}

/// 保存发布正式JSON/R8前请求一个不可与既有dirty批次合并的新水域事务代次。
#[derive(Message, Clone, Debug)]
pub struct DynamicWaterRebuildRequest {
    /// 只有请求地图仍是当前玩法地图时才消费，切图后的迟到请求自然失效。
    pub map_id: MapId,
}

/// 独立画布保存只推进自己的一次正式水事务，不使同图其他未修改水域重新构建。
#[derive(Message, Clone, Debug)]
pub struct TerrainWaterRebuildRequest {
    pub map_id: MapId,
    pub terrain_id: feathrise_map_content::TerrainId,
}

/// 一个当前已安装静水、河流或瀑布水面的资产所有权。
pub(super) struct InstalledDynamicWaterLayer {
    pub(super) spawn: PaintedWaterSurfaceSpawn,
}

/// CPU整批成功后逐帧登记的不可见候选；完整GPU回执前旧正式水面继续显示。
pub(super) struct PendingDynamicWaterBatch {
    pub(super) map_id: MapId,
    pub(super) generation: u64,
    pub(super) terrain_generation: u64,
    pub(super) remaining: VecDeque<BuiltWaterSurface>,
    pub(super) staged: Vec<StagedWaterSurface>,
    pub(super) warmup_entities: Vec<Entity>,
    pub(super) triangle_count: usize,
    pub(super) ticket: Option<WaterGpuUploadTicket>,
    pub(super) ready_to_commit: bool, // CPU、GPU和透明管线均就绪后等待地形共同提交令牌。
}

/// 动态水域唯一写侧；所有整图工作都通过一个single-flight任务背压。
#[derive(Resource, Default)]
pub(crate) struct DynamicWaterState {
    /// 当前目标地图；改变时旧图水面立即清理，迟到任务按此拒收。
    pub(super) desired_map_id: Option<MapId>,
    /// 当前已安装水面所属地图。
    pub(super) installed_map_id: Option<MapId>,
    /// 上次观察到的目录修订。
    pub(super) catalog_revision: u64,
    /// 上次观察到的全局水体预设修订；变化会重建全部静水描述和GPU材质。
    pub(super) preset_revision: u64,
    /// 每次地图、JSON、R8作者源、清单或GLB派生高度变化递增。
    pub(super) generation: u64,
    /// 最新输入尚未提交后台构建。
    pub(super) dirty: bool,
    /// 当前层ID到无Mip R8作者源强句柄。
    pub(super) mask_handles: BTreeMap<String, Handle<MapWaterMaskAsset>>,
    pub(super) mask_asset_ids: std::collections::HashSet<AssetId<MapWaterMaskAsset>>, // 事件帧O(1)身份反查，不扫描全部水层。
    /// 当前层ID到作者源路径，用于判断JSON是否改变CPU遮罩身份。
    pub(super) mask_paths: BTreeMap<String, String>,
    /// 当前采用的GLB派生地形代次；新高度完成后自动作废水面旧批次。
    pub(super) terrain_generation: Option<u64>,
    /// 同一时间最多一个地图级完整建网格任务。
    pub(super) task: Option<Task<DynamicWaterTaskResult>>,
    /// 后台完整结果按共享帧预算登记，GPU就绪前不生成任何互动实体。
    pub(super) pending: Option<PendingDynamicWaterBatch>,
    /// 上一完整有效结果；热更失败时继续显示。
    pub(super) installed: Vec<InstalledDynamicWaterLayer>,
}

/// 宿主只把并发席位归还给真正结清后台工作的实例。
impl DynamicWaterState {
    pub(in crate::maps::dynamic_map) fn background_busy(&self) -> bool {
        self.task.is_some()
    }
    pub(in crate::maps::dynamic_map) fn background_finished(&self) -> bool {
        self.task.as_ref().is_none_or(|task| task.is_finished())
    }
}

impl DynamicWaterState {
    /// 全局地形提交门查询当前水域是否已为指定GLB候选准备完整隐藏批次。
    pub(crate) fn candidate_ready_for_terrain(
        &self,
        map_id: &MapId,
        terrain_generation: u64,
        required: bool,
    ) -> bool {
        if !required {
            return true; // 未声明水域的地图没有待切换水体。
        }
        self.pending.as_ref().is_some_and(|pending| {
            pending.map_id == *map_id
                && pending.terrain_generation == terrain_generation
                && pending.ready_to_commit
        }) // 只有完整GPU候选可参与四域同帧提交。
    }

    /// 合并同帧多个资产事件为一个新代次，不启动并发整图任务。
    pub(super) fn mark_dirty(&mut self, status: &mut DynamicWaterStatus) {
        if !self.dirty {
            self.generation = self.generation.wrapping_add(1).max(1); // 零保留给Disabled公开语义。
        }
        self.dirty = true;
        status.desired_generation = self.generation; // 保存交接立即看见尚未采用的新输入代次。
        status.phase = DynamicWaterPhase::Loading; // 新成功输入先退出旧Ready/Failed，避免恢复事件被误判成当前代次失败。
    }

    /// 保存交接专用代次边界；即使已有输入dirty或后台任务在途，也必须明确推进一次。
    pub(super) fn begin_rebuild_transaction(&mut self, status: &mut DynamicWaterStatus) {
        self.generation = self.generation.wrapping_add(1).max(1); // 请求之后发布的全部资产事件合并到这个新事务或其后继代次。
        self.dirty = true; // 在途旧任务保留句柄，完成时按新generation拒收后再构建当前输入。
        status.desired_generation = self.generation;
        status.phase = DynamicWaterPhase::Loading;
    }

    /// 确认资产失败时立即作废在途旧快照，但等待后续修复事件再启动新任务。
    pub(super) fn invalidate_failed_generation(&mut self, status: &mut DynamicWaterStatus) {
        self.generation = self.generation.wrapping_add(1).max(1); // 不能丢弃非协作任务句柄，零保留Disabled。
        self.dirty = false; // 失败输入不立即重试，避免稳定帧反复日志和构建。
        status.desired_generation = self.generation; // 失败代次仍是当前权威目标，不能伪装成上一批已落稳。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 锁定运行时输入代次对公开交接证据的推进规则。

    #[test]
    fn dirty_and_failure_generations_never_advance_applied_evidence() {
        let mut state = DynamicWaterState::default(); // 从未加载水域的零代次状态。
        let mut status = DynamicWaterStatus::default(); // 公开证据同样从零开始。

        state.mark_dirty(&mut status); // 首批输入成为当前期望代次。
        assert_eq!(status.desired_generation, state.generation); // 期望证据必须同步公开。
        assert_eq!(status.applied_generation, 0); // 尚未安装不能提前宣称完成。

        status.applied_generation = state.generation; // 模拟首批已经原子安装。
        state.dirty = false; // 下一事件应建立新代次而不是与旧脏标记合并。
        state.mark_dirty(&mut status); // JSON、R8或GLB地形生成任一变化都作废落稳等式。
        assert_ne!(status.desired_generation, status.applied_generation); // 新输入等待期间交接门保持关闭。

        let applied = status.applied_generation; // 热更失败仍可保留上一完整可见批次。
        state.invalidate_failed_generation(&mut status); // 当前失败进一步作废旧在途完成值。
        status.phase = DynamicWaterPhase::Failed; // 模拟资产事件系统发布本代次明确失败。
        assert_eq!(status.applied_generation, applied); // 失败不得伪造一次成功安装。
        assert_eq!(status.desired_generation, state.generation); // 当前失败身份仍完整公开。

        state.mark_dirty(&mut status); // 后续修复事件必须先退出旧失败阶段再等待最新整批构建。
        assert_eq!(status.phase, DynamicWaterPhase::Loading); // 保存交接不能把恢复中的新代次误判成已失败。

        let dirty_generation = state.generation;
        state.begin_rebuild_transaction(&mut status); // 即使dirty仍为true，保存事务也必须取得严格的新边界。
        assert!(state.generation > dirty_generation);
        assert_eq!(status.desired_generation, state.generation);
        assert!(state.dirty);
    }

    #[test]
    fn undeclared_water_does_not_block_a_terrain_commit() {
        let map_id = MapId::parse("MAP-water-optional").expect("测试MapId必须合法");
        let state = DynamicWaterState::default();
        assert!(state.candidate_ready_for_terrain(&map_id, 7, false));
        assert!(!state.candidate_ready_for_terrain(&map_id, 7, true));
    }
}
