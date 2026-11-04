//! 单根整网格水增量预检的固定上限游标、请求修订与 single-flight 所有权。

use std::collections::{HashMap, HashSet}; // 身份索引消除 descendants×surfaces 比较，修订表只覆盖等待或活动根。

use bevy::prelude::*; // Entity、Handle、Transform、Resource 与水材质资产句柄。
use bevy::world_serialization::InstanceId; // 最终提交必须仍属于启动时同一个World实例。
use feathrise_map_content::MapPlacementWaterSurface; // 增量快照保留完整作者值。

use super::super::super::DynamicWaterMeshOverrideQueue;
use super::super::super::{
    MeshWaterEnqueueResult, PreparedPrimitive, PrimitiveCandidate,
}; // 复用功能域硬上限、提交完成值和既有去重FIFO结果。
use super::super::height_offset::TransformChainCursor; // 高度与几何世界米制父链都使用同一分帧游标。
use super::super::uv_validation::IncrementalUvValidation; // POSITION/UV0与三角证据分别按固定预算推进。 // 请求修订与等待FIFO在同一入口保持去重。

/// 作者路径与Blender Mesh名组成的最终稳定身份。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct SurfaceIdentity {
    pub(super) node_path: Vec<String>, // 路径已经通过公共深度与字符预算校验。
    pub(super) mesh_name: String,      // Mesh datablock名称和路径一起避免共享网格误接管。
} // 结束最终稳定身份。

impl From<&MapPlacementWaterSurface> for SurfaceIdentity {
    fn from(surface: &MapPlacementWaterSurface) -> Self {
        Self {
            node_path: surface.node_path.clone(), // 每帧只复制固定数量已验证路径。
            mesh_name: surface.mesh_name.clone(), // 身份HashMap拥有值，不借用可变根组件。
        }
    }
}

/// DFS中一个父节点的下一待访问子项。
pub(super) struct HierarchyFrame {
    pub(super) parent: Entity,    // 当前Children查询的父节点。
    pub(super) next_child: usize, // 跨帧保持精确子项游标，不重复扫描前缀。
} // 结束层级DFS帧。

/// 相关primitive沿父链分帧构造作者路径时的完成前状态。
pub(super) struct PendingCandidatePath {
    pub(super) candidate: PrimitiveCandidate, // 除node_path外已捕获原始恢复快照。
    pub(super) current: Entity,               // 下一次应读取Name/ChildOf的对象或祖先。
    pub(super) reversed: Vec<String>,         // 从对象向根收集，完成后一次反转。
    pub(super) path_chars: usize,             // 路径总字符数按公共硬上限累计。
} // 结束候选路径游标。

/// 一个根的增量DFS和候选路径构建状态。
pub(super) struct CandidateTraversal {
    pub(super) frames: Vec<HierarchyFrame>, // 只随当前DFS深度增长，不物化全部后代。
    pub(super) descendants_seen: usize,     // 单根后代总额硬门跨帧累计。
    pub(super) pending_path: Option<PendingCandidatePath>, // 同时最多构造一个相关primitive路径。
} // 结束候选层级遍历状态。

impl CandidateTraversal {
    pub(super) fn new(root: Entity) -> Self {
        Self {
            frames: vec![HierarchyFrame {
                parent: root,  // 根帧通过Children下标逐项进入World实例层级。
                next_child: 0, // 首帧从第一个直接子节点开始。
            }],
            descendants_seen: 0, // 尚未消费任何后代额度。
            pending_path: None,  // 尚未命中需要构造路径的primitive。
        }
    }
} // 结束DFS状态构造。

/// 完成表面高度换算后等待逐primitive几何预检的小型输入。
#[derive(Clone)]
pub(super) struct PrimitiveGeometryInput {
    pub(super) candidate: PrimitiveCandidate, // 原Mesh、材质、位姿和标记恢复快照。
    pub(super) surface_index: usize,          // 流速、类别与诊断名称留在作者快照表中。
    pub(super) local_translation_offset: Vec3, // 已验证世界Y偏移对应的父坐标位移。
} // 结束几何预检输入。

/// 单primitive几何验证的阶段。
pub(super) enum GeometryPhase {
    ResolveWorldTransform, // 逐层组合Mesh局部到世界米制线性变换。
    ValidateUv,            // POSITION/UV0有限值与均匀三角证据分别分帧扫描。
} // 结束几何阶段。

/// 当前唯一正在推进的primitive几何游标。
pub(super) struct PrimitiveGeometryCursor {
    pub(super) input: PrimitiveGeometryInput, // 完整成功前只保留只读候选值。
    pub(super) phase: GeometryPhase,          // 明确区分层级预算和几何预算。
    pub(super) transform_chain: TransformChainCursor, // 世界线性变换逐层计算。
    pub(super) uv_validation: Option<IncrementalUvValidation>, // 变换完成后才创建属性游标。
} // 结束单primitive几何游标。

/// 几何通过但尚未登记动态材质的提交前完成值。
pub(super) struct ValidatedPrimitive {
    pub(super) candidate: PrimitiveCandidate, // 最终提交继续使用同一原始快照。
    pub(super) base_material: Handle<StandardMaterial>, // 材质阶段按预算克隆作者PBR资产。
    pub(super) flow_speed: f32,               // 作者米每秒流速。
    pub(super) flow_style_encoding: Vec2,     // 整网格普通水或瀑布Shader哨兵。
    pub(super) local_translation_offset: Vec3, // 已验证世界Y偏移。
} // 结束材质准备输入。

/// 单根工作当前阶段；除Commit外都不得修改旧primitive材质或位姿。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::shared::large_map::water::mesh_overrides::runtime) enum MeshWaterWorkStage {
    Snapshot,  // 每帧验证并复制固定数量作者覆盖。
    Traverse,  // 按层级与路径操作预算扫描World实例。
    Finalize,  // 逐覆盖核对匹配并分帧解析高度父链。
    Geometry,  // 逐primitive分帧扫描父链、顶点和抽样三角。
    Materials, // 每帧登记固定数量不可见独占动态材质。
    Discard,   // 失败或换代后先按材质预算清理临时资产。
    Commit,    // 身份再次匹配后才允许一次性原子切换ECS。
} // 结束单根阶段枚举。

/// 全局唯一活动根的完整增量工作集。
pub(in crate::shared::large_map::water::mesh_overrides::runtime) struct MeshWaterRootWork {
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) root: Entity, // 当前single-flight根。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) instance_id:
        Option<InstanceId>, // None仅供不依赖WorldSpawner的纯状态测试。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) ready_epoch: Option<u64>, // None仅供纯状态测试；正式工作必须绑定资产epoch。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) revision: u64, // Changed或World换代会推进请求修订并使本工作过期。
    pub(super) expected_surface_count: usize, // 启动时O(1)捕获，逐项快照完成前持续核对长度。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) had_applied_override: bool, // 失败日志区分Blender托底与旧成功水面托底。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) stage: MeshWaterWorkStage, // 当前每帧只推进一个明确阶段。
    pub(super) snapshot_cursor: usize, // 下一项待验证作者覆盖。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) surfaces:
        Vec<MapPlacementWaterSurface>, // 已逐项验证的拥有快照。
    pub(super) identities: HashMap<SurfaceIdentity, usize>, // 精确路径身份O(1)定位覆盖索引。
    pub(super) mesh_names: HashSet<String>, // 后代primitive先按Mesh名O(1)快筛再构造路径。
    pub(super) matches: Vec<Vec<PrimitiveCandidate>>, // 每项最多十六个primitive且全根最多六十四个。
    pub(super) matched_primitives: usize, // O(1)累计匹配总数，避免每个后代重新扫描全部覆盖桶。
    pub(super) traversal: CandidateTraversal, // 不保存全部后代，只保留DFS深度游标。
    pub(super) finalize_surface: usize, // 下一项待匹配/高度换算覆盖。
    pub(super) finalize_object_node: Option<Entity>, // 当前覆盖唯一Blender对象父节点。
    pub(super) finalize_chain: Option<TransformChainCursor>, // 当前覆盖高度父链分帧游标。
    pub(super) selected_entities: HashSet<Entity>, // 防御内存热改导致primitive重复归属。
    pub(super) geometry_inputs: Vec<PrimitiveGeometryInput>, // 匹配完成后的最多六十四项小型输入。
    pub(super) geometry_cursor: usize, // 下一primitive索引。
    pub(super) active_geometry: Option<PrimitiveGeometryCursor>, // 当前唯一几何游标。
    pub(super) inspected_vertices: usize, // 全根顶点硬上限累计。
    pub(super) sampled_triangles: usize, // 全根三角证据硬上限累计。
    pub(super) validated: Vec<ValidatedPrimitive>, // 全量几何成功后进入材质阶段。
    pub(super) material_cursor: usize, // 下一项待登记动态材质。
    pub(super) material_time: Option<f32>, // 全根材质共享同一首帧时间基线。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) prepared:
        Vec<PreparedPrimitive>, // 不可见临时材质与最终原子提交值。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) failure_detail: Option<String>, // Some表示清理完成后发布Failed；None表示静默换代。
} // 结束单根增量工作集。

impl MeshWaterRootWork {
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn new(
        root: Entity,
        instance_id: Option<InstanceId>,
        ready_epoch: Option<u64>,
        revision: u64,
        expected_surface_count: usize,
        had_applied_override: bool,
    ) -> Self {
        Self {
            root,
            instance_id,
            ready_epoch,
            revision,
            expected_surface_count,
            had_applied_override,
            stage: MeshWaterWorkStage::Snapshot,
            snapshot_cursor: 0,
            surfaces: Vec::with_capacity(expected_surface_count.min(64)),
            identities: HashMap::with_capacity(expected_surface_count.min(64)),
            mesh_names: HashSet::with_capacity(expected_surface_count.min(64)),
            matches: Vec::with_capacity(expected_surface_count.min(64)),
            matched_primitives: 0,
            traversal: CandidateTraversal::new(root),
            finalize_surface: 0,
            finalize_object_node: None,
            finalize_chain: None,
            selected_entities: HashSet::with_capacity(64),
            geometry_inputs: Vec::with_capacity(64),
            geometry_cursor: 0,
            active_geometry: None,
            inspected_vertices: 0,
            sampled_triangles: 0,
            validated: Vec::with_capacity(64),
            material_cursor: 0,
            material_time: None,
            prepared: Vec::with_capacity(64),
            failure_detail: None,
        }
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn begin_failure(
        &mut self,
        detail: String,
    ) {
        self.failure_detail = Some(detail); // 失败详情只在临时材质清理完成且身份仍最新时发布。
        self.stage = MeshWaterWorkStage::Discard; // 失败路径永远不能越过Commit触碰旧画面。
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn begin_cancellation(
        &mut self,
    ) {
        self.failure_detail = None; // 换代不是用户可见失败，最新请求会重新排队。
        self.stage = MeshWaterWorkStage::Discard; // 已登记临时材质仍需分帧精确释放。
    }
} // 结束单根工作操作。

/// 等待FIFO之外只允许一个活动根，并用轻量修订取消同根旧游标。
#[derive(Clone, Copy)]
struct MeshWaterRequestIdentity {
    revision: u64,           // 每次覆盖Changed或实例ready都生成新的单调票据。
    instance_id: InstanceId, // 请求只允许提交到生成该票据的World实例。
    ready_epoch: u64,        // 原地热重载复用InstanceId时仍以资产epoch失效旧工作。
} // 结束等待或活动请求身份。

/// 等待FIFO之外只允许一个活动根，并用轻量修订取消同根旧游标。
#[derive(Resource, Default)]
pub(in crate::shared::large_map::water) struct DynamicWaterMeshOverrideWorkState {
    next_revision: u64, // 全局单调票据不依赖作者浮点或字符串比较。
    revisions: HashMap<Entity, MeshWaterRequestIdentity>, // 只覆盖等待或活动根，容量与公开队列硬上限一致。
    blocked_world_instances: HashMap<Entity, InstanceId>, // WorldAssetRoot替换后阻断旧票据迟到Ready，真实Remove立即清理。
    active: Option<MeshWaterRootWork>,                    // single-flight保证全局每帧只推进一个根。
} // 结束整网格水增量运行时资源。

impl DynamicWaterMeshOverrideWorkState {
    fn next_revision(&mut self) -> u64 {
        self.next_revision = self.next_revision.wrapping_add(1).max(1); // 零保留为未请求诊断值。
        self.next_revision
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn request(
        &mut self,
        queue: &mut DynamicWaterMeshOverrideQueue,
        root: Entity,
        instance_id: InstanceId,
        ready_epoch: u64,
    ) -> MeshWaterEnqueueResult {
        let already_tracked = self.revisions.contains_key(&root); // 活动或已排队同根不消耗新容量。
        if !already_tracked && self.revisions.try_reserve(1).is_err() {
            return MeshWaterEnqueueResult::Full; // 等待加活动根总数始终不超过固定上限。
        }
        let revision = self.next_revision(); // 每次Changed或ready都使旧工作身份失效。
        self.revisions.insert(
            root,
            MeshWaterRequestIdentity {
                revision,
                instance_id,
                ready_epoch,
            },
        ); // 最新票据与World实例共同构成最终提交的O(1)权威身份。
        if self.active.as_ref().is_some_and(|work| work.root == root) {
            MeshWaterEnqueueResult::Coalesced // 活动根不重复进FIFO，处理系统会发现修订变化后重启。
        } else {
            let result = queue.enqueue(root); // 等待同根继续由既有双向FIFO O(1)去重。
            if result == MeshWaterEnqueueResult::Full {
                self.revisions.remove(&root); // 队列异常满时不遗留无法被消费的修订。
            }
            result
        }
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn take_next(
        &mut self,
        queue: &mut DynamicWaterMeshOverrideQueue,
    ) -> Option<(Entity, u64, InstanceId, u64)> {
        let root = queue.pop_front()?; // 稳定空帧只执行一次Option检查。
        self.revisions.get(&root).map(|request| {
            (
                root,
                request.revision,
                request.instance_id,
                request.ready_epoch,
            )
        }) // 已取消根没有工作身份，票据绑定请求产生时的实例与资产epoch。
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn current_revision(
        &self,
        root: Entity,
    ) -> Option<u64> {
        self.revisions.get(&root).map(|request| request.revision) // O(1)核对Changed或World换代身份。
    }

    #[cfg(test)]
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn current_instance_id(
        &self,
        root: Entity,
    ) -> Option<InstanceId> {
        self.revisions.get(&root).map(|request| request.instance_id) // Changed失效系统不会误取消同帧ready observer已登记的新实例请求。
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn is_active_root(
        &self,
        root: Entity,
    ) -> bool {
        self.active.as_ref().is_some_and(|work| work.root == root) // 空覆盖快路径不能越过活动临时材质清理。
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn take_active(
        &mut self,
    ) -> Option<MeshWaterRootWork> {
        self.active.take() // 每帧临时拥有工作以避免资源字段与查询借用交叠。
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn set_active(
        &mut self,
        work: MeshWaterRootWork,
    ) {
        debug_assert!(self.active.is_none(), "整网格水同时只能有一个活动根");
        self.active = Some(work); // 未完成游标准确留到下一帧。
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn finish_revision(
        &mut self,
        root: Entity,
        revision: u64,
    ) {
        if self.current_revision(root) == Some(revision) {
            self.revisions.remove(&root); // 只有当前请求终止才释放容量，较新Changed必须保留。
        }
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn requeue_if_newer(
        &mut self,
        queue: &mut DynamicWaterMeshOverrideQueue,
        root: Entity,
        finished_revision: u64,
    ) {
        if self
            .current_revision(root)
            .is_some_and(|value| value != finished_revision)
        {
            let result = queue.enqueue(root); // 取消清理完成后把同根最新请求放回FIFO，避免活动与等待重复。
            debug_assert_ne!(result, MeshWaterEnqueueResult::Full); // 活动根原本已占固定容量，转回等待不可能扩容。
        } else {
            self.finish_revision(root, finished_revision); // 没有更新请求时静默结束旧工作。
        }
    }

    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn cancel_root(
        &mut self,
        queue: &mut DynamicWaterMeshOverrideQueue,
        root: Entity,
    ) {
        let _ = queue.cancel(root); // 等待根立即O(1)摘链。
        self.revisions.remove(&root); // 活动根下一处理帧据此进入分帧取消清理。
    }

    /// 资产根刚被替换时撤销旧工作，并冻结当前InstanceId直到新票据Ready。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn block_world_instance(
        &mut self,
        queue: &mut DynamicWaterMeshOverrideQueue,
        root: Entity,
        instance_id: InstanceId,
    ) {
        self.cancel_root(queue, root); // 旧实例等待项立即摘链，活动游标下一帧进入有界Discard。
        self.blocked_world_instances.insert(root, instance_id); // 同根连续替换只保留最近被废弃票据。
    }

    /// Ready或覆盖Insert只在当前票据不是刚被废弃的旧实例时继续。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn accept_world_instance(
        &mut self,
        root: Entity,
        instance_id: InstanceId,
    ) -> bool {
        let Some(blocked) = self.blocked_world_instances.get(&root).copied() else {
            return true; // 普通首次加载和同InstanceId资产热重载没有根替换间隙。
        };
        if blocked == instance_id {
            return false; // 旧Ready命令即使与根替换同帧迟到，也不能重新登记请求。
        }
        self.blocked_world_instances.remove(&root); // 新InstanceId首次可用后结束换代阻断。
        true
    }

    /// 根真实移除或覆盖功能撤销时释放全部轻量身份。
    pub(in crate::shared::large_map::water::mesh_overrides::runtime) fn forget_root(
        &mut self,
        queue: &mut DynamicWaterMeshOverrideQueue,
        root: Entity,
    ) {
        self.cancel_root(queue, root); // 等待/活动请求先按既有语义失效。
        self.blocked_world_instances.remove(&root); // 此根不会再产生合法新Ready，阻断票据可立即释放。
    }
} // 结束single-flight请求状态操作。

#[cfg(test)]
mod tests {
    use bevy::world_serialization::{WorldAsset, WorldInstanceSpawner};

    use super::*;

    fn instance_id(spawner: &mut WorldInstanceSpawner) -> InstanceId {
        spawner.spawn(Handle::<WorldAsset>::default()) // 只生成公开唯一票据，不需要加载资产或实例化World。
    }

    #[test]
    fn changed_request_invalidates_active_cursor_without_duplicating_fifo_root() {
        let root = Entity::from_raw_u32(71).expect("测试根实体必须合法");
        let mut spawner = WorldInstanceSpawner::default();
        let instance = instance_id(&mut spawner);
        let mut queue = DynamicWaterMeshOverrideQueue::default();
        let mut state = DynamicWaterMeshOverrideWorkState::default();
        assert_eq!(
            state.request(&mut queue, root, instance, 7),
            MeshWaterEnqueueResult::Queued
        );
        let (_, first_revision, first_instance, first_epoch) =
            state.take_next(&mut queue).expect("首次请求必须可弹出");
        assert_eq!(first_instance, instance); // FIFO弹出值必须冻结请求产生时的World实例票据。
        assert_eq!(first_epoch, 7);
        state.set_active(MeshWaterRootWork::new(
            root,
            Some(instance),
            Some(first_epoch),
            first_revision,
            1,
            false,
        ));

        assert_eq!(
            state.request(&mut queue, root, instance, 8),
            MeshWaterEnqueueResult::Coalesced
        ); // 活动根覆盖Insert只推进权威修订，不重复进入FIFO。
        assert_eq!(queue.len(), 0);
        assert_ne!(state.current_revision(root), Some(first_revision)); // 旧游标下一帧必定身份失配并转入Discard。
        assert_eq!(state.current_instance_id(root), Some(instance)); // 同实例作者热改仍绑定当前World票据。
    }

    #[test]
    fn stale_ready_is_blocked_until_a_different_world_instance_arrives() {
        let root = Entity::from_raw_u32(72).expect("测试根实体必须合法");
        let mut spawner = WorldInstanceSpawner::default();
        let old_instance = instance_id(&mut spawner);
        let new_instance = instance_id(&mut spawner);
        let mut queue = DynamicWaterMeshOverrideQueue::default();
        let mut state = DynamicWaterMeshOverrideWorkState::default();
        assert_eq!(
            state.request(&mut queue, root, old_instance, 4),
            MeshWaterEnqueueResult::Queued
        );
        state.block_world_instance(&mut queue, root, old_instance); // WorldAssetRoot Discard同步撤销旧请求并冻结旧票据。
        assert!(!state.accept_world_instance(root, old_instance)); // 同帧迟到的旧Ready不能重新占用FIFO。
        assert_eq!(queue.len(), 0); // 被废弃请求已经O(1)摘链。
        assert!(state.accept_world_instance(root, new_instance)); // 只有不同的新InstanceId结束换代阻断。
        assert_eq!(
            state.request(&mut queue, root, new_instance, 5),
            MeshWaterEnqueueResult::Queued
        ); // 随后的Ready observer登记新实例。
        assert_eq!(state.current_instance_id(root), Some(new_instance));
        assert_eq!(queue.pop_front(), Some(root));
    }

    #[test]
    fn failure_state_can_only_discard_and_never_reach_commit() {
        let root = Entity::from_raw_u32(73).expect("测试根实体必须合法");
        let mut work = MeshWaterRootWork::new(root, None, None, 1, 1, true);
        work.begin_failure("第二个材质资产缺失".to_owned());
        assert_eq!(work.stage, MeshWaterWorkStage::Discard); // 失败路径唯一后继是清理，不允许半成品进入Commit。
        assert_eq!(work.failure_detail.as_deref(), Some("第二个材质资产缺失"));
        assert!(work.prepared.is_empty()); // 没有提交阶段调用时旧primitive材质与位姿完全无写入机会。
    }
}
