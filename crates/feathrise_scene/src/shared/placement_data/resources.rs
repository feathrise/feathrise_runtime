//! 显式场景实例热更新状态。

use std::collections::{HashMap, HashSet, VecDeque, hash_set, vec_deque}; // Ready信用、定向结算索引、规划唯一性及拥有型退役游标。
use std::sync::Arc; // 完整建筑净空数组在候选、活动资源和植被后台任务之间只浅克隆。

use bevy::asset::AssetId; // 规划身份冻结清单、摆放JSON与去重Scene资产ID。
use bevy::prelude::*; // Entity、Transform、Rect与Resource。
use bevy::world_serialization::{InstanceId, WorldAsset}; // 候选场景强句柄与换代阻断票据都只保存轻量身份。
use feathrise_scene_content::{
    BuildingWorldQuad, SceneId, SceneManifestAsset, ScenePlacementRecord, ScenePlacementWaterSurface,
    ScenePlacementsAsset,
}; // 增量规划冻结输入资产身份、单记录快照及其水体退役游标。
use feathrise_scene_core::state::ScenePhase; // 识别切换场景并清理旧实例。

use super::components::PlacementInstanceGate; // 同批placement共享O(1)作者活动门。

/// 当前场景显式实例数据阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScenePlacementRuntimePhase {
    /// 目录、清单或摆放文件仍在加载。
    #[default]
    Waiting,
    /// 场景依赖已加载，正在隐藏分帧生成候选实例树。
    Building,
    /// 完整文件已经校验并生成。
    Ready,
    /// 本次数据无效；继续保留上一份完整实例。
    Failed,
}

/// F3和F12可读取的显式实例热更新证据。
#[derive(Resource, Clone, Debug, Default)]
pub struct ScenePlacementRuntimeStatus {
    pub phase: ScenePlacementRuntimePhase,
    pub scene_id: Option<SceneId>,
    /// 最近一次已经完整原子采用的公开修订；失败尝试不会推进它。
    pub revision: u64,
    /// 每次正式摆放候选开始构建或确定失败时推进，用于区分保存发布后的新失败和旧失败。
    pub attempt_revision: u64,
    /// 当前运行时发生真实有界推进时单调增加；稳定轮询和诊断文字刷新不得续命。
    ///
    /// 保存交接以本值实现“连续三十秒无进展”超时，因此候选规划、生成游标、
    /// World/水体联合门和最终复验游标都必须只在确实前进时调用`advance_progress`。
    pub progress_revision: u64,
    pub placement_count: usize,
    pub detail: String,
}

impl ScenePlacementRuntimeStatus {
    /// 发布一次真实运行时推进；饱和后保持单调而不回绕成旧证据。
    pub(crate) fn advance_progress(&mut self) {
        self.progress_revision = self.progress_revision.saturating_add(1);
    }
}

/// World/整网格水联合门最多缓存的去重候选根数；覆盖一张合法场景8192项placement上限。
pub(super) const MAX_PLACEMENT_SETTLEMENT_ROOTS: usize = 8_192;

/// 生命周期observer提交一个定向候选根时的确定结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlacementSettlementEnqueueResult {
    Queued,    // 新根进入FIFO，等待固定预算结算。
    Coalesced, // 同根多次World/水状态变化合并为一次最终状态读取。
    Full,      // 队列达到硬门，调用方必须拒绝整份候选修订。
}

/// 一个候选根在可O(1)取消的双向FIFO中的邻接身份。
#[derive(Clone, Copy, Debug)]
struct PlacementSettlementQueueLinks {
    previous: Option<Entity>, // None表示当前根是队首。
    next: Option<Entity>,     // None表示当前根是队尾。
}

/// 生命周期observer只写入发生变化的根；稳定帧不再扫描全部候选原型。
#[derive(Resource, Default)]
pub(super) struct PlacementSettlementQueue {
    head: Option<Entity>, // 下一项需要读取最终World/水联合门状态的根。
    tail: Option<Entity>, // 新定向事件以O(1)追加到队尾。
    links: HashMap<Entity, PlacementSettlementQueueLinks>, // 反向索引同时提供去重和根移除取消。
}

impl PlacementSettlementQueue {
    /// 把根加入固定容量FIFO；同帧World ready与水状态写入只保留一个席位。
    pub(super) fn enqueue(&mut self, entity: Entity) -> PlacementSettlementEnqueueResult {
        if self.links.contains_key(&entity) {
            return PlacementSettlementEnqueueResult::Coalesced; // 处理时读取组件最终值，不保存中间状态。
        }
        if self.links.len() >= MAX_PLACEMENT_SETTLEMENT_ROOTS {
            return PlacementSettlementEnqueueResult::Full; // 不丢弃旧根，也不突破固定内存上限。
        }
        let previous = self.tail; // 当前队尾成为新项的前驱。
        self.links.insert(
            entity,
            PlacementSettlementQueueLinks {
                previous,   // 空队列的新项没有前驱。
                next: None, // 新项固定成为队尾。
            },
        ); // 完成唯一邻接记录登记。
        if let Some(previous) = previous {
            self.links
                .get_mut(&previous)
                .expect("结算队尾必须仍在反向索引中")
                .next = Some(entity); // 原队尾链接到新项。
        } else {
            self.head = Some(entity); // 第一项同时成为队首。
        }
        self.tail = Some(entity); // 无论是否为空，新项都是唯一队尾。
        PlacementSettlementEnqueueResult::Queued // 返回真实入队供调用方推进诊断。
    }

    /// 弹出一个仍有效的定向根，稳定空帧只执行一次Option检查。
    pub(super) fn pop_front(&mut self) -> Option<Entity> {
        let entity = self.head?; // 空队列不访问任何placement组件。
        let removed = self.cancel(entity); // 统一走双向摘链保持不变量。
        debug_assert!(removed, "结算队首必须存在于反向索引"); // 调试构建锁定队列一致性。
        Some(entity) // 返回本帧预算中的一个精确根。
    }

    /// 根移除、退役或已消费时O(1)撤销其待结算席位。
    pub(super) fn cancel(&mut self, entity: Entity) -> bool {
        let Some(links) = self.links.remove(&entity) else {
            return false; // 不在队列的根保持幂等。
        };
        if let Some(previous) = links.previous {
            self.links
                .get_mut(&previous)
                .expect("有效结算前驱必须仍在索引中")
                .next = links.next; // 前驱跳过当前根。
        } else {
            self.head = links.next; // 删除队首时提升后继。
        }
        if let Some(next) = links.next {
            self.links
                .get_mut(&next)
                .expect("有效结算后继必须仍在索引中")
                .previous = links.previous; // 后继跳过当前根。
        } else {
            self.tail = links.previous; // 删除队尾时降低前驱。
        }
        true // 报告确实撤销了一个有效席位。
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.links.len() // 反向索引数量就是当前有效根数。
    }
}

/// `WorldAssetRoot`替换到新`WorldInstanceReady`之间阻断旧实例迟到事件。
#[derive(Resource, Default)]
pub(super) struct PlacementWorldAssetInvalidations {
    blocked_instances: HashMap<Entity, InstanceId>, // 每个正在换代的根只保留被废弃的旧票据。
}

impl PlacementWorldAssetInvalidations {
    /// 记录资产根刚刚废弃的旧World实例身份。
    pub(super) fn block(&mut self, entity: Entity, instance_id: InstanceId) {
        self.blocked_instances.insert(entity, instance_id); // 同根连续替换只保留最新旧票据。
    }

    /// 只有不同于被废弃票据的新Ready才能解除换代间隙。
    pub(super) fn accept_ready(&mut self, entity: Entity, instance_id: InstanceId) -> bool {
        let Some(blocked) = self.blocked_instances.get(&entity).copied() else {
            return true; // 未发生资产根替换时，GLB同实例热重载Ready仍然合法。
        };
        if blocked == instance_id {
            return false; // 旧实例迟到Ready不得重新授信或重建水材质。
        }
        self.blocked_instances.remove(&entity); // 新InstanceId首次Ready结束换代间隙。
        true
    }

    /// 根真实移除后不再等待任何新实例票据。
    pub(super) fn clear(&mut self, entity: Entity) {
        self.blocked_instances.remove(&entity); // O(1)释放轻量换代身份。
    }
}

/// 请求当前场景用现有正式`ScenePlacementsAsset`重新建立一次隐藏候选。
///
/// 【功能设计】保存交接失败后的原样重试不能为了制造`Modified`而重写未变化资产；
/// 编辑器改为发送本消息，运行时仍走同一完整校验、隐藏候选和原子替换链路。
#[derive(Message, Clone, Debug)]
pub struct ScenePlacementRebuildRequest {
    pub scene_id: SceneId,
}

/// 完整校验后持有的一项场景实例计划。
#[derive(Debug)]
pub(crate) struct PendingPlacementPrimitive {
    pub(crate) mesh: Handle<Mesh>, // 精确`MeshN/PrimitiveN`强句柄随计划等待完整依赖。
    pub(crate) material: Handle<StandardMaterial>, // 原关联`MaterialN/std`或默认材质强句柄。
}

/// 完整校验后持有的一项场景或单primitive实例计划。
#[derive(Debug)]
pub(crate) struct PendingPlacementPlan {
    pub(crate) record: ScenePlacementRecord, // 强句柄等待期保留文件内目标、原始变换与赋水数组的同一完整JSON快照。
    pub(crate) scene: Handle<WorldAsset>,  // Scene0继续作为同源GLTF解析与完整场景依赖的强句柄。
    pub(crate) primitive: Option<PendingPlacementPrimitive>, // Some时候选直接生成唯一网格而不实例化整棵Scene0。
    pub(crate) transform: Transform, // 已校验记录一次转换为Bevy运行时变换，不在生成帧重复计算。
}

/// 一次不可拆分的显式实例修订。
#[derive(Debug)]
pub(crate) struct PendingPlacementRevision {
    pub(crate) terrain_patch_count: usize,
    pub(crate) terrain_holes: Arc<[feathrise_scene_content::SceneTerrainCutout]>, // 至多32个完整世界洞口，只在全记录通过后发布。
    pub(crate) generation: u32,
    pub(crate) revision: u32,
    pub(crate) scene_id: SceneId,
    pub(crate) building_preset_revision: u64, // 净空四边形必须与同一建筑目录快照绑定。
    pub(crate) building_clearances: Arc<[BuildingWorldQuad]>, // 全部记录完成后一次冻结，不暴露规划前缀。
    pub(crate) building_clearance_floor_heights: Arc<[f32]>, // 与净空同序的地基层，洞内外共享XZ时仍可区分。
    pub(crate) plans: Vec<PendingPlacementPlan>,
}

/// 正在校验一条含整网格水体的摆放记录。
///
/// 【功能设计】整份JSON不在启动帧深拷贝；只冻结当前一条记录，
/// 并用水体游标在最多六十四项的独立帧预算下完成稳定身份校验。
#[derive(Debug)]
pub(crate) struct PlacementRecordPlanning {
    pub(crate) source_pose: feathrise_scene_content::TerrainObjectTransform,
    pub(crate) record: ScenePlacementRecord, // 基础字段已冻结，水体Vec只追加已通过当帧预算校验的前缀。
    pub(crate) transform: Transform,       // 基础字段校验后只转换一次运行时变换。
    pub(crate) total_water_surfaces: usize, // 启动记录时冻结的源绑定数，跨帧变化立即失败。
    pub(crate) next_water_surface: usize,  // 下一个待校验水体绑定索引。
    pub(crate) water_identities: HashSet<(Vec<String>, String)>, // 同根节点路径与网格名组合必须唯一。
}

/// 一次正式摆放文件的有界增量规划事务。
#[derive(Debug)]
pub(crate) struct PlacementPlanning {
    /// 地形域只接管旧图的三维地形记录，场景容器只处理普通实例，防止迁移后重复生成。
    pub(crate) terrain_filter: TerrainPlacementFilter,
    pub(crate) terrain_patch_count: usize,
    pub(crate) terrain_holes: Vec<feathrise_scene_content::SceneTerrainCutout>,
    pub(crate) generation: u32,       // 切换场景代次与后续候选根严格对齐。
    pub(crate) revision: u32,         // 文件尝试修订防止迟到规划被采用。
    pub(crate) scene_id: SceneId,         // 同一DynamicSceneRoot内换SceneId也会取消本事务。
    pub(crate) catalog_revision: u64, // 目录句柄表换代后不再继续读取旧输入。
    pub(crate) building_preset_revision: u64, // 建筑净空参数换代后整份规划必须重启。
    pub(crate) manifest_id: AssetId<SceneManifestAsset>, // 冻结世界范围的清单资产身份。
    pub(crate) document_id: AssetId<ScenePlacementsAsset>, // 跨帧继续读取的摆放JSON资产身份。
    pub(crate) world_bounds: Rect,    // 轻量范围在开始时完整校验并冻结。
    pub(crate) total_records: usize,  // 与启动时文档长度一致，变化时失败关闭。
    pub(crate) next_record: usize,    // 下一条未启动校验的记录游标。
    pub(crate) total_water_surfaces: usize, // 逐记录累计全图二百五十六项硬门。
    pub(crate) ids: HashSet<String>,  // 增量拒绝重复placement ID，不再单帧建全表。
    pub(crate) current_record: Option<PlacementRecordPlanning>, // 最多保留一条未完成的含水记录。
    pub(crate) plans: Vec<PendingPlacementPlan>, // 已完整校验的前缀，完成时O(1)移入候选修订。
    pub(crate) building_clearances: Vec<BuildingWorldQuad>, // 每条建筑记录完成时追加一个固定大小世界四边形。
    pub(crate) building_clearance_floor_heights: Vec<f32>,  // 固定小预算随该条记录一起追加。
    pub(crate) scene_ids: HashSet<AssetId<WorldAsset>>,     // 多个摆放复用同一GLB时仅跟踪一份依赖。
    pub(crate) queued_scene_ids: HashSet<AssetId<WorldAsset>>, // 与首轮队列同步增量构建，收尾帧不复制全表。
    pub(crate) scene_checks: VecDeque<AssetId<WorldAsset>>,    // 规划期同步建立首轮有界检查队列。
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum TerrainPlacementFilter {
    #[default]
    All,
    Container,
    Terrain(feathrise_scene_content::TerrainId),
}

impl TerrainPlacementFilter {
    pub(crate) fn accepts(&self, record: &feathrise_scene_content::ScenePlacementRecord) -> bool {
        match self {
            Self::All => true,
            Self::Container => record.terrain_patch.is_none(),
            Self::Terrain(id) => record.terrain_patch.as_ref().is_some_and(|patch| patch.terrain_id.as_ref()
                .map_or(*id == feathrise_scene_content::TerrainId::legacy(), |owner| owner == id)),
        }
    }
}

/// 完整规划后的Scene依赖就绪跟踪器。
#[derive(Debug)]
pub(crate) struct PlacementSceneLoadTracker {
    pub(crate) generation: u32, // 只服务同一候选代次。
    pub(crate) revision: u32,   // 只服务同一文件修订。
    /// 规划阶段的placement稳定ID集合继续随候选持有，结束时交给固定预算退役器逐项释放。
    pub(crate) placement_ids: HashSet<String>,
    pub(crate) scene_ids: HashSet<AssetId<WorldAsset>>, // 资产事件归属使用O(1)成员查询。
    pub(crate) ready_scene_ids: HashSet<AssetId<WorldAsset>>, // 只记录已观测到完整依赖Ready的去重ID。
    pub(crate) queued_scene_ids: HashSet<AssetId<WorldAsset>>, // 防止事件风暴重复堆积同一ID。
    pub(crate) scene_checks: VecDeque<AssetId<WorldAsset>>, // 每帧只弹出启动帧队列长度内的固定数量。
}

/// 中途取消规划的拥有型逐项释放游标。
#[derive(Debug)]
pub(crate) struct PlacementPlanningRetirement {
    pub(crate) current_record: Option<PlacementPlanRetirementCursor>, // 跨帧未完成的单记录快照也按水绑定/路径预算释放。
    pub(crate) water_identities:
        Option<std::iter::Peekable<hash_set::IntoIter<(Vec<String>, String)>>>, // 当前单记录的节点路径/网格去重身份。
    pub(crate) current_water_identity: Option<PlacementWaterIdentityRetirement>, // HashSet取出的单项继续逐路径段释放。
    pub(crate) plans: std::vec::IntoIter<PendingPlacementPlan>, // 已完整校验的强句柄计划前缀。
    pub(crate) current_plan: Option<PlacementPlanRetirementCursor>, // Vec取出的计划不能连同嵌套水路径整项析构。
    pub(crate) placement_ids: std::iter::Peekable<hash_set::IntoIter<String>>, // 已见placement稳定ID。
    pub(crate) scene_ids: hash_set::IntoIter<AssetId<WorldAsset>>, // 已见唯一Scene资产ID。
    pub(crate) queued_scene_ids: hash_set::IntoIter<AssetId<WorldAsset>>, // 已进入首轮检查队列的Scene ID。
    pub(crate) scene_checks: vec_deque::IntoIter<AssetId<WorldAsset>>,    // 首轮Scene检查队列。
}

/// 候选取消或采用后的拥有型逐项释放游标。
#[derive(Debug)]
pub(crate) struct PlacementCandidatePlanRetirement {
    pub(crate) plans: std::vec::IntoIter<PendingPlacementPlan>, // 完整候选的强句柄计划。
    pub(crate) current_plan: Option<PlacementPlanRetirementCursor>, // 当前计划的水绑定和路径段跨帧析构游标。
    pub(crate) placement_ids: std::iter::Peekable<hash_set::IntoIter<String>>, // 规划唯一性集合随候选保持到终止边界。
    pub(crate) scene_ids: hash_set::IntoIter<AssetId<WorldAsset>>,             // 资产事件归属全集。
    pub(crate) ready_scene_ids: hash_set::IntoIter<AssetId<WorldAsset>>, // 已Ready的Scene信用。
    pub(crate) queued_scene_ids: hash_set::IntoIter<AssetId<WorldAsset>>, // 去重排队信用。
    pub(crate) scene_checks: vec_deque::IntoIter<AssetId<WorldAsset>>,   // Waiting检查队列。
}

/// 一个强句柄计划或规划中单记录的嵌套析构游标。
#[derive(Debug)]
pub(crate) struct PlacementPlanRetirementCursor {
    pub(crate) id: Option<String>, // placement稳定ID按字符串字节预算释放。
    pub(crate) asset_path: Option<String>, // GLB路径按字符串字节预算释放。
    pub(crate) scene: Option<Handle<WorldAsset>>, // 强句柄只在全部嵌套水绑定释放后作为顶层计划项释放。
    pub(crate) water_surfaces: std::vec::IntoIter<ScenePlacementWaterSurface>, // 每项继续拆成路径段游标。
    pub(crate) current_water_surface: Option<PlacementWaterIdentityRetirement>, // 当前水绑定不允许一次析构完整节点路径。
}

/// 一个水绑定或去重身份的路径段/网格名析构游标。
#[derive(Debug)]
pub(crate) struct PlacementWaterIdentityRetirement {
    pub(crate) node_path: std::vec::IntoIter<String>, // 节点路径每段同时受路径段和字符串字节预算。
    pub(crate) mesh_name: Option<String>, // 网格名受字符串字节预算，释放后才算完成一个水绑定。
}

/// 一代规划或完整候选的不可见大容器退役所有权。
///
/// 【功能设计】取消、换代和原子采用只把这些所有权O(1)移入本状态；独立系统随后
/// 按固定预算释放计划、稳定ID和Scene依赖身份，避免在交互帧同步析构八千项。
#[derive(Debug)]
pub(crate) struct PlacementPlanRetirement {
    pub(crate) planning: Option<PlacementPlanningRetirement>, // 中途规划各大容器转换为不回扫旧桶的拥有型游标。
    pub(crate) candidate: Option<PlacementCandidatePlanRetirement>, // 完整计划与Scene集合在同一候选终止边界退役。
}

/// 已建立但仍保持隐藏的候选实体树。
#[derive(Debug, Default)]
pub(crate) struct PlacementReuseState {
    pub(crate) cursor: usize,
    // 只持有标量hash和Entity，释放索引不逐条析构作者字符串；命中后仍精确核对ID。
    pub(crate) index: HashMap<u64, Option<Entity>>,
    pub(crate) carry: Option<(Entity, Entity)>, // (Identity容器, 旧活动父根)，取消后有界归还。
}

#[derive(Debug)]
pub(crate) struct PlacementStaging {
    pub(crate) reuse: PlacementReuseState,
    pub(crate) terrain_roots: Vec<Entity>, // 至多32个三维地形根，采用帧不扫描全图实例。
    pub(crate) primitive_collision_roots: Vec<Entity>, // 普通 GLB 网格共用后台碰撞，但无需参与地形洞口派生。
    pub(crate) root: Entity,
    /// 候选根列表按固定预算复验；采用后原样移动给活动批次，不在采用帧复制。
    pub(crate) roots: Vec<Entity>,
    pub(crate) gate: PlacementInstanceGate, // 全部根共享，候选期始终false。
    pub(crate) generation: u32,
    pub(crate) revision: u32,
    pub(crate) building_preset_revision: u64, // 原子采用时连同净空来源目录一起发布。
    pub(crate) building_clearances: Arc<[BuildingWorldQuad]>, // 候选取消或采用都不复制完整数组。
    pub(crate) building_clearance_floor_heights: Arc<[f32]>, // 与候选净空在同一采用点发布。
    pub(crate) next_plan: usize,
    /// 仅收录当前代次/修订且最新一次World实例化后整网格水已Ready的根。
    pub(crate) ready_roots: HashSet<Entity>, // 容量被全图8192 placement硬门封顶，重复事件不增长。
    /// 相关Scene事件先按AssetId全局阻止采用，根级旧信用随后由共享索引按128项/帧撤销。
    pub(crate) blocked_scene_assets: HashMap<AssetId<WorldAsset>, u64>,
    pub(crate) total_instances: usize,
    /// Commands创建的容器必须至少在下一帧被真实Query观察一次，零placement不能用预留Entity冒充已生成。
    pub(crate) container_observed: bool,
    /// 任一根的新World实例或水状态换代都会推进，令在途完整复验从头开始。
    pub(crate) eligibility_epoch: u64,
    pub(crate) verification_epoch: u64,
    pub(crate) verification_cursor: usize,
}

/// 已原子采用的一整批显式实例；根表继续服务有界标记清理和退役静默化。
#[derive(Debug)]
pub(crate) struct PlacementActiveBatch {
    pub(crate) building_preset_revision: u64,
    pub(crate) terrain_roots: Vec<Entity>, // 至多32项，旧物理在采用或离图边界立即撤销。
    pub(crate) primitive_collision_roots: Vec<Entity>,
    pub(crate) root: Entity,
    pub(crate) roots: Vec<Entity>,
    pub(crate) gate: PlacementInstanceGate,
    pub(crate) marker_cleanup_cursor: usize,
}

/// 一棵退役树的非递归深度优先回收游标。
#[derive(Debug)]
pub(crate) struct PlacementTreeCleanup {
    pub(crate) restore_carry: Option<(Entity, Entity)>, // 失败候选不得拥有复用根；静默阶段把它们归还旧活动容器。
    pub(crate) root: Entity,
    /// 先分帧移除这些直接根的`WorldAssetRoot`，阻止回收期间继续热重载长出后代。
    pub(crate) roots: Vec<Entity>,
    pub(crate) quiesce_cursor: usize,
    /// 直接根全部静默后，从容器开始逐边断开`ChildOf`并只销毁叶节点。
    pub(crate) stack: Vec<Entity>,
}

impl PlacementTreeCleanup {
    pub(crate) fn new(root: Entity, roots: Vec<Entity>) -> Self {
        Self {
            restore_carry: None,
            root,
            roots,
            quiesce_cursor: 0,
            stack: Vec::new(),
        }
    }
}

/// 唯一写侧保存的有限事务状态。
#[derive(Resource, Debug)]
pub(crate) struct ScenePlacementRuntimeState {
    pub(crate) observed_document: Option<AssetId<ScenePlacementsAsset>>, // 暂停恢复不把相同预览句柄误当成新内容。
    pub(crate) terrain_mesh_generation: Option<[u32; 2]>,
    pub(crate) terrain_patch_active: bool,
    pub(crate) terrain_holes_applied: Arc<[feathrise_scene_content::SceneTerrainCutout]>, // 地形物理发布后才放行同一摆放候选。
    pub(crate) dirty: bool,
    pub(crate) active_scene: Option<ScenePhase>,
    /// `DynamicSceneRoot`不换State也能切换SceneId，必须作为候选身份的一部分观察。
    pub(crate) active_scene_id: Option<SceneId>,
    pub(crate) catalog_revision: u64,
    pub(crate) building_preset_revision: u64, // 建筑目录成功换代会触发同一有界 placement 重建链。
    pub(crate) generation: u32,
    pub(crate) revision: u32,
    /// 记录与整网格水体正在按固定预算生成强Scene0句柄计划。
    pub(crate) planning: Option<PlacementPlanning>,
    pub(crate) pending: Option<PendingPlacementRevision>,
    /// 已规划完整的去重Scene依赖以队列而非全表`any/all`轮询。
    pub(crate) scene_load: Option<PlacementSceneLoadTracker>,
    pub(crate) staging: Option<PlacementStaging>,
    pub(crate) active: Option<PlacementActiveBatch>,
    /// 背压保证任一时刻最多只有一代大计划退役；新规划必须等其逐项释放完毕。
    pub(crate) retired_plan: Option<PlacementPlanRetirement>,
    /// 新构建在队列清空前背压，因此活动、候选和既有退役任务合计始终是固定小数量。
    pub(crate) retired_trees: VecDeque<PlacementTreeCleanup>,
    /// 失败后只在匹配场景资产事件出现时重新检查，稳定帧不轮询全部句柄。
    pub(crate) asset_retry_requested: bool,
    pub(crate) failure_logged_revision: Option<u32>,
    /// 共享WorldAsset读者存在未消费积压时，全局拒绝候选采用；未读Modified可能已在本帧SpawnScene重生。
    pub(crate) world_asset_events_caught_up: bool,
}

impl Default for ScenePlacementRuntimeState {
    fn default() -> Self {
        Self {
            observed_document: None,
            terrain_mesh_generation: None,
            terrain_patch_active: false,
            terrain_holes_applied: Arc::default(),
            dirty: true,
            active_scene: None,
            active_scene_id: None,
            catalog_revision: 0,
            building_preset_revision: 0,
            generation: 0,
            revision: 0,
            planning: None,
            pending: None,
            scene_load: None,
            staging: None,
            active: None,
            retired_plan: None,
            retired_trees: VecDeque::new(),
            asset_retry_requested: false,
            failure_logged_revision: None,
            world_asset_events_caught_up: true,
        }
    }
}
