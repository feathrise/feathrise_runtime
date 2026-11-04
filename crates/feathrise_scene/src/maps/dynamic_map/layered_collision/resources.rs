//! 多层共享碰撞的单飞任务、内容身份与已发布块；不拥有显示网格。
use super::types::{ChunkGeometry, ChunkKey, ChunkPlan};
use avian3d::prelude::Collider; // 后台完成形状后才交给主世界。
use bevy::{prelude::*, tasks::Task}; // 主世界资源与非阻塞任务句柄。
use feathrise_map_content::sparse_height::{SparseCraterTiles, SparseHeightTile, SparseHeightTileKey}; // 局部作者源与会话破坏。
use feathrise_map_content::{
    MapCollisionLod, MapId, SparseHeightManifestAsset, SparseHeightSpec, SparseHeightTileAsset,
}; // 当前地图与高度资产身份。
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant, // 请求冻结至完整采用的真实墙钟，不参与内容相等。
}; // 大型输入仅共享不可变所有权。 // 同一拓扑描述同时决定复用和接缝。

#[derive(Clone, Debug, PartialEq)] // 完整需求身份，禁止迟到结果跨配置或跨地图采用。
pub(super) struct Request {
    pub stitch_profiles:Arc<[feathrise_map_content::ClipmapEdgeProfile]>,
    pub stitches: Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,
    pub holes: Arc<[feathrise_map_content::MapTerrainCutout]>, // 洞口属于几何身份，不能复用仍封住入口的旧块。
    // 不含任何高度大数组。
    pub map_id: MapId,                // 地图稳定ID。
    pub root: Entity,                 // 当前地图实体代次。
    pub spec: SparseHeightSpec,       // 格点和边界身份。
    pub manifest_revision: u64,       // 正式文件事件修订。
    pub live_revision: Option<u64>,   // F12完整覆盖修订。
    pub crater_revision: Option<u64>, // 会话浅坑修订。
    pub lods: Vec<MapCollisionLod>,   // 保存后的配置快照。
    pub anchor: Vec2,                 // 候选冻结的玩家圆心，绝不采用相机焦点。
} // 每个异步阶段均携带此身份。

pub(super) struct LayoutChunk {
    // 后台生成的零高度拓扑及精确采样需求。
    pub plan: ChunkPlan,                      // 接缝标识参与缓存判断。
    pub geometry: ChunkGeometry,              // 只在后台填高度和构造Collider。
    pub tiles: BTreeSet<SparseHeightTileKey>, // 仅当前块顶点真正需要的基础页。
} // 不枚举块范围内没有被采样的整图格点。

pub(super) struct Layout {
    // 一次完整窗口，全部成功后方可采用。
    pub planning_cache: Option<Arc<super::planning::PlanCache>>, // 跨次径向规划快照随完整采用布局发布，串行参考夹具不持有缓存。
    pub chunks: BTreeMap<ChunkKey, Arc<LayoutChunk>>, // 最大512块；未变化拓扑和页依赖共享，不深拷贝。
    pub tiles: BTreeSet<SparseHeightTileKey>,         // 去重后的源页需求。
    pub supported_polygon: Vec<Vec2>,                 // 旧凸包合同兼容字段；新完整格路径为空。
    pub supported_rows: Option<Arc<feathrise_scene_core::residency::TerrainSupportRows>>, // 完整粗格行覆盖。
    pub elapsed_ms: f32,                              // 后台规划与布局合计墙钟。
    pub rebuilt_blocks: usize,                        // 本轮真正生成网格和页依赖的块数。
    pub reused_blocks: usize,                         // 规划身份相同直接共享的块数。
} // 只保留一个候选布局。

#[derive(Clone, PartialEq, Eq)] // 内容寻址比较保证未变块不重建。
pub(super) struct SourceStamp {
    // 只记录本块采样触及页的身份。
    pub sources: Vec<(SparseHeightTileKey, String, usize, usize)>, // 正式摘要、实时Arc身份、浅坑Arc身份。
} // Arc身份仅在同一进程且源快照仍持有时比较。

pub(super) struct Resident {
    // 当前真正参与物理的形状。
    pub entity: Entity,     // 原位换形状避免重建无关刚体。
    pub plan: ChunkPlan,    // 对比接缝变化。
    pub stamp: SourceStamp, // 对比局部高度变化。
} // 旧集合保留直到候选完整。

pub(super) struct Cooked {
    // 后台返回已完成的替换集合。
    pub shapes: Vec<(ChunkPlan, SourceStamp, Collider)>, // 仅包含缺失或变脏的块。
    pub plans: BTreeMap<ChunkKey, ChunkPlan>,            // 包含保留块的完整目标集合。
    pub sources: BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>, // 保持实时Arc身份直到下一次采用。
    pub craters: SparseCraterTiles, // 同理保留浅坑源快照，防止指针复用。
    pub bounds: Vec<(Vec2, Vec2)>,  // 活动门读取完整已采用覆盖。
    pub supported_polygon: Vec<Vec2>, // 旧凸包合同兼容字段；新完整格路径为空。
    pub supported_rows: Option<Arc<feathrise_scene_core::residency::TerrainSupportRows>>, // 与候选形状同版的完整格覆盖。
    pub elapsed_ms: f32,             // 本次后台实际构造耗时。
    pub layout: Option<Arc<Layout>>, // 仅完整采用后进入运行时缓存；测试兼容夹具允许无缓存。
} // ECS写入留给主线程。

pub(super) enum Completion {
    // 单飞阶段机。
    Layout(Result<Layout, String>), // 纯规划与拓扑生成。
    Cooked(Result<Cooked, String>), // 纯采样与物理形状构建。
} // 同时至多一个CPU任务。

pub(super) struct TaskResult {
    // 完成值显式绑定发起票据。
    pub ticket: u64,            // 换图或新需求递增。
    pub request: Arc<Request>, // 完成值直接携带地图、根、源修订、配置及窗口身份，不只依赖票据间接推断。
    pub completion: Completion, // 任务失败也回收并发槽。
} // 不丢弃运行中的CPU句柄伪装取消。

pub(super) struct Lease {
    // 受检内容寻址资产强引用。
    pub map_id: MapId, // 跨图未完成IO保留背压，绝不能误认为新图同坐标源页。
    pub hash: String,  // 只在内容变化时重新加载。
    pub handle: Handle<SparseHeightTileAsset>, // IO由AssetServer负责。
} // 候选完成后淘汰不再需要的页。

#[derive(Default)] // 固定三个所有权槽，不维护历史数组或异步任务链。
pub(super) struct RetiredPlanningPayload {
    pub applied: Option<Arc<Layout>>, // 被替换或换图退出的唯一已采用布局。
    pub candidate: Option<Layout>,    // 等待IO时被换代的唯一候选布局。
    pub completion: Option<Completion>, // 单飞任务唯一迟到/拒收完成值。
} // 退役未排空时禁止新增候选，因此各槽不会累计多个版本。

#[derive(Resource, Default)] // 插件幂等初始化。
pub(crate) struct LayeredCollisionRuntime {
    // 独立于历史双层和显示同网格模式。
    pub(super) map_id: Option<MapId>, // 当前地图。
    pub(super) root: Option<Entity>,  // 当前根。
    pub(super) path: Option<String>,  // 当前稀疏清单路径。
    pub(super) manifest: Option<Handle<SparseHeightManifestAsset>>, // 强引用维持正式源。
    pub(super) manifest_revision: u64, // 不因相机移动换代。
    pub(super) desired: Option<Arc<Request>>, // 在途冻结需求；同内容普通走动不改变其发布身份。
    pub(super) last_player_center: Option<Vec2>, // 唯一合并的最新玩家位置；当前候选完成后下一帧追赶，不积压位置队列。
    pub(super) applied: Option<Arc<Request>>,    // 当前完整采用版本。
    pub(super) applied_layout: Option<Arc<Layout>>, // 只保留已采用拓扑，候选失败不污染下一轮缓存。
    pub(super) retire_task: Option<Task<()>>,    // 至多一个后台析构任务，实际完成才归还槽位。
    pub(super) retired: RetiredPlanningPayload, // 至多一个固定待释放集合；换图重复调用仅移动尚未取走的所有权。
    pub(super) request_started: Option<Instant>, // 当前票据冻结时刻，跨后台与IO阶段保持不变。
    pub(super) ticket: u64,                     // 所有阶段共用取消身份。
    pub(super) task: Option<Task<TaskResult>>,  // 被换代任务仍占槽直到实际完成。
    pub(super) layout: Option<Layout>,          // 页准备中的唯一候选。
    pub(super) resident: BTreeMap<ChunkKey, Resident>, // 最大512个有效块。
    pub(super) leases: BTreeMap<SparseHeightTileKey, Lease>, // 候选有界页租约。
    pub(super) retained_sources: BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>, // 旧版仍有效的源。
    pub(super) retained_craters: SparseCraterTiles, // 防止源指针在比较前失效。
    pub(super) failed_ticket: Option<u64>,          // 同一坏输入不忙等重试。
} // 显示和作者文件不会被此资源修改。

// 在途任务真正完成后才释放实例并发席位。
impl LayeredCollisionRuntime {
    pub(in crate::maps::dynamic_map) fn background_busy(&self, server: &AssetServer) -> bool { self.task.is_some() || self.retire_task.is_some()
        || self.leases.values().any(|lease| matches!(server.load_state(lease.handle.id()), bevy::asset::LoadState::NotLoaded | bevy::asset::LoadState::Loading)) }
    pub(in crate::maps::dynamic_map) fn background_finished(&self, server: &AssetServer) -> bool { self.task.as_ref().is_none_or(|task| task.is_finished()) && self.retire_task.as_ref().is_none_or(|task| task.is_finished())
        && self.leases.values().all(|lease| !matches!(server.load_state(lease.handle.id()), bevy::asset::LoadState::NotLoaded | bevy::asset::LoadState::Loading)) }
}
