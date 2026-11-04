//! 显式实例批次的身份撤销、背压排队与固定预算非递归树回收。

#[cfg(test)]
#[path = "retirement/terrain_tests.rs"]
mod terrain_tests;
mod tree_cleanup; // 实体树exclusive回收与规划数据析构使用独立固定预算。

pub(crate) use tree_cleanup::cleanup_retired_placement_trees; // 插件层保持原系统路径不变。

use feathrise_scene_core::residency::{InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use bevy::prelude::*; // 批次显隐、场景根注销、父子关系断边与叶实体回收。

use super::super::resources::{
    MapPlacementRuntimeState, PendingPlacementPlan, PendingPlacementRevision,
    PlacementCandidatePlanRetirement, PlacementPlanRetirement, PlacementPlanRetirementCursor,
    PlacementPlanning, PlacementPlanningRetirement, PlacementRecordPlanning,
    PlacementSceneLoadTracker, PlacementTreeCleanup, PlacementWaterIdentityRetirement,
}; // 活动/候选所有权、规划大容器与退役树精确游标。

pub(super) const PLACEMENT_PLAN_RETIRE_BUDGET_PER_FRAME: usize = 128; // 空水计划、稳定ID或Scene队列项最多释放一百二十八个。
pub(super) const PLACEMENT_PLAN_RETIRE_WATER_BUDGET_PER_FRAME: usize = 64; // 水绑定及其重复身份独立限制为六十四项。
pub(super) const PLACEMENT_PLAN_RETIRE_PATH_BUDGET_PER_FRAME: usize = 512; // 节点路径字符串最多释放五百一十二段。
pub(super) const PLACEMENT_PLAN_RETIRE_STRING_BYTES_PER_FRAME: usize = 32 * 1024; // ID、资产路径、网格名和路径段合计三十二KiB。
pub(super) const PLACEMENT_TREE_CLEANUP_BUDGET_PER_FRAME: usize = 128; // 退役树每帧只处理固定数量静默、断边或叶销毁操作。
const MAX_RETIRED_PLACEMENT_TREES: usize = 3; // 既有退役树加活动/候选换图最多形成三个清理所有权。

/// 单帧退役真实工作量；测试用它证明四组独立预算都没有被嵌套析构绕过。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PlacementPlanRetireAdvance {
    pub(super) top_level_items: usize, // 完成的计划壳、稳定ID和Scene ID/队列项。
    pub(super) water_surfaces: usize,  // 完成的水绑定或规划重复身份。
    pub(super) path_segments: usize,   // 逐个释放的节点路径字符串。
    pub(super) string_bytes: usize,    // 本帧实际释放的作者字符串字节。
}

/// 一帧四组独立剩余额度；合法单个超大字符串只允许在全新帧独占一次。
struct PlacementPlanRetireBudget {
    top_level_items: usize,              // 普通计划和集合条目额度。
    water_surfaces: usize,               // 水绑定/身份额度。
    path_segments: usize,                // 节点路径段额度。
    string_bytes: usize,                 // 所有作者字符串累计字节额度。
    released_any: bool,                  // 防止多个超预算单项在同帧连续放行。
    advance: PlacementPlanRetireAdvance, // 对外只发布真实释放计数。
}

impl Default for PlacementPlanRetireBudget {
    fn default() -> Self {
        Self {
            top_level_items: PLACEMENT_PLAN_RETIRE_BUDGET_PER_FRAME,
            water_surfaces: PLACEMENT_PLAN_RETIRE_WATER_BUDGET_PER_FRAME,
            path_segments: PLACEMENT_PLAN_RETIRE_PATH_BUDGET_PER_FRAME,
            string_bytes: PLACEMENT_PLAN_RETIRE_STRING_BYTES_PER_FRAME,
            released_any: false,
            advance: PlacementPlanRetireAdvance::default(),
        }
    }
}

impl PlacementPlanRetireBudget {
    /// 在真正移动字符串前预扣字节；单个合法大项可以独占空白帧而不会永久阻塞。
    fn charge_string(&mut self, bytes: usize) -> bool {
        if bytes <= self.string_bytes {
            self.string_bytes -= bytes; // 普通字符串从三十二KiB额度精确扣减。
        } else if !self.released_any {
            self.string_bytes = 0; // 只让当前单项独占本帧，其余工作将在下一帧继续。
        } else {
            return false; // 本帧已有进展时不再越过字符串字节硬预算。
        }
        self.released_any = true;
        self.advance.string_bytes = self.advance.string_bytes.saturating_add(bytes);
        true
    }

    /// 完成一个普通顶层条目。
    fn finish_top_level(&mut self) {
        self.top_level_items -= 1;
        self.released_any = true;
        self.advance.top_level_items += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetirementStep {
    Progress, // 至少释放了一个受预算控制的条目或字符串。
    Complete, // 当前游标完整释放，调用者可丢弃空壳。
    Blocked,  // 当前独立预算耗尽，必须等下一帧。
}

/// 把水绑定转换为不会一次析构完整节点路径的拥有型游标。
fn water_surface_cursor(
    surface: feathrise_map_content::MapPlacementWaterSurface,
) -> PlacementWaterIdentityRetirement {
    PlacementWaterIdentityRetirement {
        node_path: surface.node_path.into_iter(), // Vec::into_iter只移动缓冲所有权，不扫描路径段。
        mesh_name: Some(surface.mesh_name),       // 网格名延后到同一水绑定预算内释放。
    }
}

/// 把一个规划唯一性身份转换为相同的路径/网格名游标。
fn water_identity_cursor(identity: (Vec<String>, String)) -> PlacementWaterIdentityRetirement {
    PlacementWaterIdentityRetirement {
        node_path: identity.0.into_iter(), // HashSet拥有型迭代器取出的Vec仍按段释放。
        mesh_name: Some(identity.1),       // 重复身份的网格名同样受字节预算。
    }
}

/// 把完整强句柄计划转换为嵌套析构游标；标量字段在当前常数工作内释放。
fn pending_plan_cursor(plan: PendingPlacementPlan) -> PlacementPlanRetirementCursor {
    PlacementPlanRetirementCursor {
        id: Some(plan.record.id),                 // 稳定ID保留到字节预算允许的帧。
        asset_path: Some(plan.record.asset_path), // 资产路径保留到字节预算允许的帧。
        scene: Some(plan.scene),                  // 强句柄在该计划所有水绑定释放后才计为顶层完成。
        water_surfaces: plan.record.water_surfaces.into_iter(), // Vec所有权O(1)转成前向游标。
        current_water_surface: None,              // 首个水绑定只在有独立额度时取出。
    }
}

/// 把规划中未完成的单记录转换为与正式计划相同的嵌套析构游标。
fn current_record_cursor(
    record: feathrise_map_content::MapPlacementRecord,
) -> PlacementPlanRetirementCursor {
    PlacementPlanRetirementCursor {
        id: Some(record.id),                 // 已冻结ID仍按字符串字节预算释放。
        asset_path: Some(record.asset_path), // 已冻结路径仍按字符串字节预算释放。
        scene: None,                         // 未完成记录尚未请求强Scene0句柄。
        water_surfaces: record.water_surfaces.into_iter(), // 已通过校验的水绑定前缀逐项退役。
        current_water_surface: None,         // 具体路径段在后续固定预算帧取出。
    }
}

/// 推进一个水绑定/身份的一个字符串步骤。
fn advance_water_identity(
    cursor: &mut PlacementWaterIdentityRetirement,
    budget: &mut PlacementPlanRetireBudget,
) -> RetirementStep {
    if budget.water_surfaces == 0 {
        return RetirementStep::Blocked; // 没有水绑定额度时不预先释放其内部路径。
    }
    if let Some(segment) = cursor.node_path.as_slice().first() {
        if budget.path_segments == 0 || !budget.charge_string(segment.len()) {
            return RetirementStep::Blocked; // 路径段同时受数量和总字节两道门。
        }
        let _ = cursor.node_path.next(); // 当前段在这一受预算操作末尾析构。
        budget.path_segments -= 1;
        budget.advance.path_segments += 1;
        return RetirementStep::Progress;
    }
    if let Some(mesh_name) = cursor.mesh_name.as_ref() {
        if !budget.charge_string(mesh_name.len()) {
            return RetirementStep::Blocked; // 网格名不能绕过全帧字符串字节预算。
        }
        let _ = cursor.mesh_name.take(); // 名称释放后该水绑定不再持有任何动态字符串。
    }
    budget.water_surfaces -= 1;
    budget.advance.water_surfaces += 1;
    budget.released_any = true;
    RetirementStep::Complete // 调用者现在可安全丢弃空路径/名称壳。
}

/// 推进一个计划的嵌套水绑定、作者字符串或最终强句柄壳。
fn advance_plan_cursor(
    cursor: &mut PlacementPlanRetirementCursor,
    budget: &mut PlacementPlanRetireBudget,
) -> RetirementStep {
    if budget.top_level_items == 0 {
        return RetirementStep::Blocked; // 第129个计划不能先释放内部字符串再延期计数。
    }
    if cursor.current_water_surface.is_none() {
        if budget.water_surfaces == 0 && !cursor.water_surfaces.as_slice().is_empty() {
            return RetirementStep::Blocked; // 尚有水绑定时必须先取得独立水额度。
        }
        if let Some(surface) = cursor.water_surfaces.next() {
            cursor.current_water_surface = Some(water_surface_cursor(surface)); // O(1)移动当前绑定，不析构其路径。
        }
    }
    if let Some(surface) = cursor.current_water_surface.as_mut() {
        match advance_water_identity(surface, budget) {
            RetirementStep::Complete => {
                cursor.current_water_surface = None; // 仅空壳在完成本次水预算后释放。
                return RetirementStep::Progress;
            }
            step => return step, // 路径推进或预算阻塞原样返回。
        }
    }
    if let Some(id) = cursor.id.as_ref() {
        if !budget.charge_string(id.len()) {
            return RetirementStep::Blocked; // placement ID等待下一帧字节额度。
        }
        let _ = cursor.id.take(); // 字符串按精确字节计费后释放。
        return RetirementStep::Progress;
    }
    if let Some(path) = cursor.asset_path.as_ref() {
        if !budget.charge_string(path.len()) {
            return RetirementStep::Blocked; // GLB路径等待下一帧字节额度。
        }
        let _ = cursor.asset_path.take(); // 字符串按精确字节计费后释放。
        return RetirementStep::Progress;
    }
    let _ = cursor.scene.take(); // 空水或全部水绑定已退役后才释放一个强句柄。
    budget.finish_top_level();
    RetirementStep::Complete
}

/// 把中途规划的所有大容器O(1)转换为拥有型前向游标。
fn planning_retirement(planning: PlacementPlanning) -> PlacementPlanningRetirement {
    let (current_record, water_identities) = match planning.current_record {
        Some(record) => {
            let PlacementRecordPlanning {
                record,
                water_identities,
                ..
            } = record; // 单记录作者快照与重复身份集合分别移入前向游标。
            let identities = water_identities.into_iter().peekable(); // HashSet桶只转移所有权，不从头反复扫描。
            (Some(current_record_cursor(record)), Some(identities))
        }
        None => (None, None), // 普通记录边界没有额外单记录游标。
    };
    PlacementPlanningRetirement {
        current_record,
        water_identities,
        current_water_identity: None,
        plans: planning.plans.into_iter(), // Vec转拥有型游标是O(1)。
        current_plan: None,
        placement_ids: planning.ids.into_iter().peekable(), // Peekable允许读取下一字符串长度后再真正移动。
        scene_ids: planning.scene_ids.into_iter(),
        queued_scene_ids: planning.queued_scene_ids.into_iter(),
        scene_checks: planning.scene_checks.into_iter(),
    }
}

/// 把完整候选计划与Scene跟踪器O(1)转换为同代拥有型游标。
fn candidate_retirement(
    pending: Option<PendingPlacementRevision>,
    tracker: Option<PlacementSceneLoadTracker>,
) -> PlacementCandidatePlanRetirement {
    let plans = pending
        .map(|pending| pending.plans.into_iter())
        .unwrap_or_else(|| Vec::new().into_iter()); // 缺失半边仍构造安全空游标并释放另一半。
    let Some(tracker) = tracker else {
        return PlacementCandidatePlanRetirement {
            plans,
            current_plan: None,
            placement_ids: std::collections::HashSet::new().into_iter().peekable(),
            scene_ids: std::collections::HashSet::new().into_iter(),
            ready_scene_ids: std::collections::HashSet::new().into_iter(),
            queued_scene_ids: std::collections::HashSet::new().into_iter(),
            scene_checks: std::collections::VecDeque::new().into_iter(),
        }; // 防御性不一致状态仍不得同步析构pending计划。
    };
    PlacementCandidatePlanRetirement {
        plans,
        current_plan: None,
        placement_ids: tracker.placement_ids.into_iter().peekable(),
        scene_ids: tracker.scene_ids.into_iter(),
        ready_scene_ids: tracker.ready_scene_ids.into_iter(),
        queued_scene_ids: tracker.queued_scene_ids.into_iter(),
        scene_checks: tracker.scene_checks.into_iter(),
    }
}

/// 释放一个受顶层数量和字符串字节共同约束的placement ID。
fn advance_placement_id(
    ids: &mut std::iter::Peekable<std::collections::hash_set::IntoIter<String>>,
    budget: &mut PlacementPlanRetireBudget,
) -> RetirementStep {
    let Some(id) = ids.peek() else {
        return RetirementStep::Complete; // 拥有型迭代器已耗尽，后续drop只释放空桶壳。
    };
    if budget.top_level_items == 0 || !budget.charge_string(id.len()) {
        return RetirementStep::Blocked;
    }
    let _ = ids.next(); // 已预扣字符串字节后移动并释放当前稳定ID。
    budget.finish_top_level();
    RetirementStep::Progress
}

/// 释放一个无动态作者字符串的Scene ID或轮询队列项。
fn advance_scalar_item<T>(
    entries: &mut impl ExactSizeIterator<Item = T>,
    budget: &mut PlacementPlanRetireBudget,
) -> RetirementStep {
    if entries.len() == 0 {
        return RetirementStep::Complete; // 空迭代器无需消耗顶层预算。
    }
    if budget.top_level_items == 0 {
        return RetirementStep::Blocked;
    }
    let _ = entries.next(); // AssetId是固定大小值，一个顶层额度对应一个条目。
    budget.finish_top_level();
    RetirementStep::Progress
}

/// 按固定次序推进中途规划，当前嵌套游标阻塞时不越过它释放后续容器。
fn advance_planning_retirement(
    planning: &mut PlacementPlanningRetirement,
    budget: &mut PlacementPlanRetireBudget,
) -> RetirementStep {
    if let Some(record) = planning.current_record.as_mut() {
        return match advance_plan_cursor(record, budget) {
            RetirementStep::Complete => {
                planning.current_record = None;
                RetirementStep::Progress
            }
            step => step,
        };
    }
    if planning.current_water_identity.is_none() {
        if let Some(identity) = planning
            .water_identities
            .as_mut()
            .and_then(|identities| identities.next())
        {
            planning.current_water_identity = Some(water_identity_cursor(identity)); // 所有权移动不释放内部路径。
        } else {
            planning.water_identities = None; // 耗尽迭代器只剩空HashSet桶壳。
        }
    }
    if let Some(identity) = planning.current_water_identity.as_mut() {
        return match advance_water_identity(identity, budget) {
            RetirementStep::Complete => {
                planning.current_water_identity = None;
                RetirementStep::Progress
            }
            step => step,
        };
    }
    if planning.current_plan.is_none() && planning.plans.len() > 0 {
        planning.current_plan = planning.plans.next().map(pending_plan_cursor); // 只取一个计划进入嵌套游标。
    }
    if let Some(plan) = planning.current_plan.as_mut() {
        return match advance_plan_cursor(plan, budget) {
            RetirementStep::Complete => {
                planning.current_plan = None;
                RetirementStep::Progress
            }
            step => step,
        };
    }
    match advance_placement_id(&mut planning.placement_ids, budget) {
        RetirementStep::Complete => {}
        step => return step,
    }
    match advance_scalar_item(&mut planning.scene_ids, budget) {
        RetirementStep::Complete => {}
        step => return step, // Scene归属集尚有内容时不越过它处理后续队列。
    }
    match advance_scalar_item(&mut planning.queued_scene_ids, budget) {
        RetirementStep::Complete => {}
        step => return step, // 排队去重集尚有内容时保持稳定释放次序。
    }
    match advance_scalar_item(&mut planning.scene_checks, budget) {
        RetirementStep::Complete => {}
        step => return step, // 首轮检查队列逐项完成后规划阶段才算完全退役。
    }
    RetirementStep::Complete
}

/// 按相同预算推进完整候选计划和Scene身份集合。
fn advance_candidate_retirement(
    candidate: &mut PlacementCandidatePlanRetirement,
    budget: &mut PlacementPlanRetireBudget,
) -> RetirementStep {
    if candidate.current_plan.is_none() && candidate.plans.len() > 0 {
        candidate.current_plan = candidate.plans.next().map(pending_plan_cursor); // 嵌套水路径不随Vec::next直接析构。
    }
    if let Some(plan) = candidate.current_plan.as_mut() {
        return match advance_plan_cursor(plan, budget) {
            RetirementStep::Complete => {
                candidate.current_plan = None;
                RetirementStep::Progress
            }
            step => step,
        };
    }
    match advance_placement_id(&mut candidate.placement_ids, budget) {
        RetirementStep::Complete => {}
        step => return step,
    }
    match advance_scalar_item(&mut candidate.scene_ids, budget) {
        RetirementStep::Complete => {}
        step => return step,
    }
    match advance_scalar_item(&mut candidate.ready_scene_ids, budget) {
        RetirementStep::Complete => {}
        step => return step,
    }
    match advance_scalar_item(&mut candidate.queued_scene_ids, budget) {
        RetirementStep::Complete => {}
        step => return step,
    }
    match advance_scalar_item(&mut candidate.scene_checks, budget) {
        RetirementStep::Complete => {}
        step => return step,
    }
    RetirementStep::Complete
}

/// 推进一帧退役；返回四组真实计数供严格预算回归使用。
pub(super) fn advance_retired_plan(
    retired: &mut PlacementPlanRetirement,
) -> PlacementPlanRetireAdvance {
    let mut budget = PlacementPlanRetireBudget::default();
    loop {
        if let Some(planning) = retired.planning.as_mut() {
            match advance_planning_retirement(planning, &mut budget) {
                RetirementStep::Progress => continue,
                RetirementStep::Complete => {
                    retired.planning = None; // 所有拥有型迭代器均耗尽后只释放空分配壳。
                    continue;
                }
                RetirementStep::Blocked => break,
            }
        }
        if let Some(candidate) = retired.candidate.as_mut() {
            match advance_candidate_retirement(candidate, &mut budget) {
                RetirementStep::Progress => continue,
                RetirementStep::Complete => {
                    retired.candidate = None; // 完整候选的大容器已经逐项释放完毕。
                    continue;
                }
                RetirementStep::Blocked => break,
            }
        }
        break; // 两阶段都已完成。
    }
    budget.advance
}

/// 把当前规划/候选大容器原样移入唯一固定容量退役槽。
pub(super) fn retire_plan_data(state: &mut MapPlacementRuntimeState) {
    let has_plan_data =
        state.planning.is_some() || state.pending.is_some() || state.scene_load.is_some(); // O(1)判断是否需要占用退役槽。
    if !has_plan_data {
        return; // 重复取消保持幂等，不创建空退役任务。
    }
    assert!(
        state.retired_plan.is_none(),
        "placement规划退役槽已占用；候选背压不变量被破坏",
    ); // 新规划必须等唯一旧代完全释放，因此生产路径绝不能覆盖在途大容器。
    let planning = state.planning.take().map(planning_retirement); // 中途规划各容器只转为拥有型迭代游标。
    let pending = state.pending.take(); // 完整计划所有权O(1)移出活动候选身份。
    let scene_load = state.scene_load.take(); // ID集合和检查队列与对应候选同代移出。
    let candidate = if pending.is_some() || scene_load.is_some() {
        Some(candidate_retirement(pending, scene_load)) // Vec/HashSet/VecDeque只调用into_iter，不遍历内容。
    } else {
        None // 规划中途取消没有完整候选阶段。
    };
    state.retired_plan = Some(PlacementPlanRetirement {
        planning,
        candidate,
    });
}

pub(super) fn queue_retired_tree(
    commands: &mut Commands,
    state: &mut MapPlacementRuntimeState,
    root: Entity,
    roots: Vec<Entity>,
) {
    commands.entity(root).insert((Visibility::Hidden, avian3d::prelude::RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)); // 同一发布边界撤销整批内置盒物理和角色地面探测，分帧回收期间不再承托玩家。
    debug_assert!(
        state.retired_trees.len() < MAX_RETIRED_PLACEMENT_TREES,
        "placement退役队列容量不变量被破坏"
    ); // 新候选受清理背压，正常路径最多只有既有、活动和候选三棵树。
    state
        .retired_trees
        .push_back(PlacementTreeCleanup::new(root, roots)); // Vec所有权O(1)移动，不在取消帧复制8192根。
}

pub(super) fn retire_candidate(commands: &mut Commands, state: &mut MapPlacementRuntimeState) {
    if let Some(staging) = state.staging.take() {
        staging.gate.retire(); // 取消候选永久进入退役态，物理Pending标记残留也不能再冒充保存证明。
        queue_retired_tree(commands, state, staging.root, staging.roots);
        state.retired_trees.back_mut().expect("刚登记候选树").restore_carry = staging.reuse.carry;
    }
    retire_plan_data(state); // 文件换代或换图只做O(1)所有权转移，旧游标和Scene身份立即失效。
    state.asset_retry_requested = false;
}

pub(super) fn retire_active_batch(commands: &mut Commands, state: &mut MapPlacementRuntimeState) {
    if let Some(active) = state.active.take() {
        active.gate.retire(); // O(1)永久撤销整批正式身份，旧根即使尚未销毁也不再提供保存证据或接受作者写入。
        super::super::terrain_collision::retire_formal_terrain(commands, &active.terrain_roots);
        queue_retired_tree(commands, state, active.root, active.roots);
    }
}

/// 每帧只释放固定数量规划项；完成前唯一退役槽对新候选形成明确背压。
pub(crate) fn cleanup_retired_placement_plan(mut state: InstanceResMut<MapPlacementRuntimeState>) {
    let Some(retired) = state.retired_plan.as_mut() else {
        return; // 稳定帧无退役所有权时O(1)返回。
    };
    let _ = advance_retired_plan(retired); // 四组预算在内部共同推进同一拥有型游标。
    if retired.planning.is_none() && retired.candidate.is_none() {
        state.retired_plan = None; // 只丢弃已清空的固定大小壳，不触发隐藏全表析构。
    }
}
