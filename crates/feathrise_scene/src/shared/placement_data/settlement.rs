//! 显式摆放World实例与整网格水体接管的联合就绪门。
//!
//! 【功能设计】`WorldInstanceReady`上的多个observer不依赖注册顺序；生命周期observer
//! 只把真正变化的根写入固定容量去重FIFO，Update再按固定预算读取同帧最终组件值。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use avian3d::prelude::ColliderConstructorHierarchyReady; // 建筑真实网格碰撞必须加入原子采用门。
use bevy::prelude::*; // 实体观察、查询、命令与资源更新共用Bevy ECS类型。
use bevy::world_serialization::{WorldAssetRoot, WorldInstance, WorldInstanceReady}; // 资产根换代与实例完成共同提供定向生命周期事件。
use feathrise_scene_content::ScenePlacementAssetTarget; // 直接primitive碰撞完成后无需等待World门。

use crate::shared::large_scene::DynamicWaterMeshOverrideStatus; // 水体接管是候选原子采用的显式必要条件。
use crate::shared::world_asset_lifecycle::{
    WorldAssetLifecycleIndex, WorldAssetRootEpochInvalidated, WorldAssetRootRegistration,
}; // 同InstanceId热重载仍用资产epoch撤销旧WorldReady。

use super::components::{
    ScenePlacementInstance, ScenePlacementProgressPulse, PlacementBuildingCollisionGeneration,
    PlacementBuildingCollisionReady, PlacementPendingInstance, PlacementPendingRevision,
    PlacementWorldInstanceReady,
}; // 本模块独占实例完成门标记的写入与消费。
use super::resources::{
    MAX_PLACEMENT_SETTLEMENT_ROOTS, PlacementSettlementEnqueueResult, PlacementSettlementQueue,
    PlacementWorldAssetInvalidations,
}; // 固定容量FIFO消除稳定帧全原型扫描，换代票据阻断旧Ready回流。
use super::resources::{ScenePlacementRuntimeState, ScenePlacementRuntimeStatus}; // 联合门按根撤销或登记当前候选Ready信用。
use super::systems::fail_revision; // 任一赋水失败都复用整份修订失败和旧根保留语义。

/// 每帧最多结算的定向根数；与候选生成预算一致，普通无水根不会积压。
const PLACEMENT_SETTLEMENT_BUDGET_PER_FRAME: usize = 128;

/// 撤销当前候选根上一次World实例化的Ready信用。
fn invalidate_candidate_root_readiness(
    state: &mut ScenePlacementRuntimeState, // 唯一候选事务持有可撤销集合。
    entity: Entity,                       // 再次实例化的placement根。
    pending: &PlacementPendingRevision,   // 根全生命期修订身份。
) -> bool {
    let Some(staging) = state.staging.as_mut() else {
        return false; // 候选已采用或销毁时，迟到事件不能新建信用。
    }; // 完成当前候选读取。
    if staging.generation != pending.generation || staging.revision != pending.revision {
        return false; // 旧代次或旧修订的热更事件不得污染新候选。
    } // 完成根与事务身份核对。
    staging.ready_roots.remove(&entity); // 不论新实例最终Pending、Failed还是Ready，先撤销旧实例信用。
    staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1); // 即使旧信用尚未登记，新World实例也必须让在途全表复验作废。
    state.asset_retry_requested = true; // 若整份修订曾Failed，允许同一正式链路重新进入候选轮询。
    true // 返回该事件确实属于当前候选。
} // 结束候选根Ready信用撤销。

/// 水体单根增量准备只在真实推进时定向触发；旧候选脉冲不能给当前保存门续命。
pub(crate) fn on_placement_progress(
    pulse: On<ScenePlacementProgressPulse>, // EntityEvent只唤醒所属根观察器，不分配全局消息队列。
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    pending_roots: Query<&PlacementPendingRevision, With<PlacementPendingInstance>>,
    state: InstanceRes<ScenePlacementRuntimeState>, // 只核对当前候选身份，不遍历根表。
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>,
) {
    let instance_scope = context.instance(pulse.entity);
    let Some(state) = state.for_instance(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    let Ok(pending) = pending_roots.get(pulse.entity) else {
        return; // 已采用、退役或外部实体的迟到推进不得影响正式交接门。
    };
    let matches_current = state.staging.as_ref().is_some_and(|staging| {
        staging.generation == pending.generation && staging.revision == pending.revision
    });
    if matches_current {
        status.advance_progress(); // 生产者承诺只在游标/阶段确实推进时触发，稳定轮询不会续命。
    }
}

/// `WorldAssetRoot`被替换或移除时同步撤销旧实例Ready，不等待下一帧扫描`WorldInstance`。
pub(super) fn invalidate_discarded_candidate_world_asset(
    discard: On<Discard<WorldAssetRoot>>, // Discard在旧根值仍可查询时触发，覆盖替换与真实移除都会到达。
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    roots: Query<(&PlacementPendingRevision, &WorldInstance), With<PlacementPendingInstance>>, // 只直接读取触发事件的一个隐藏候选根。
    mut invalidations: InstanceResMut<PlacementWorldAssetInvalidations>, // 旧InstanceId阻断同帧迟到Ready。
    mut settlements: InstanceResMut<PlacementSettlementQueue>, // 已排队的旧联合门请求立即O(1)撤销。
    mut state: InstanceResMut<ScenePlacementRuntimeState>,       // 当前候选Ready集合在事件发生时同步失效。
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>,     // 换代是一次真实进展，停滞计时从此重新开始。
    mut commands: Commands, // 门组件使用try_remove，实体若同帧销毁则静默忽略。
) {
    let instance_scope = context.instance(discard.entity);
    let Some(invalidations) = invalidations.for_instance_mut(instance_scope) else { return; };
    let Some(settlements) = settlements.for_instance_mut(instance_scope) else { return; };
    let Some(state) = state.for_instance_mut(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    let Ok((pending, instance)) = roots.get(discard.entity) else {
        return; // 正式已采用、退役或普通World根不属于候选联合门。
    };
    if !invalidate_candidate_root_readiness(state, discard.entity, pending) {
        return; // 旧代次根不得污染当前候选身份。
    }
    invalidations.block(discard.entity, **instance); // 新InstanceId Ready到达前拒绝当前旧票据回流。
    settlements.cancel(discard.entity); // 队列中基于旧实例的联合结算不再有效。
    commands
        .entity(discard.entity)
        .try_remove::<PlacementWorldInstanceReady>(); // 同帧先关闭门，后续只有新Ready能重新打开。
    status.advance_progress(); // 资产根真实换代只推进一次heartbeat。
}

/// `WorldAssetRoot`真正移除时取消换代阻断和待结算席位，并拒绝仍活动的残缺候选。
pub(super) fn cancel_removed_candidate_world_asset(
    remove: On<Remove<WorldAssetRoot>>, // Replace不会触发Remove，因此不会误清新实例前的阻断票据。
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    roots: Query<&PlacementPendingRevision, With<PlacementPendingInstance>>, // Remove前仍可读取旧候选修订。
    mut invalidations: InstanceResMut<PlacementWorldAssetInvalidations>, // 实体不再等待任何新World实例。
    mut settlements: InstanceResMut<PlacementSettlementQueue>,           // 根移除必须O(1)取消队列席位。
    mut state: InstanceResMut<ScenePlacementRuntimeState>, // 外部破坏当前候选时撤销旧Ready信用。
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>, // 残缺候选沿用整份修订失败语义。
) {
    let instance_scope = context.instance(remove.entity);
    let Some(invalidations) = invalidations.for_instance_mut(instance_scope) else { return; };
    let Some(settlements) = settlements.for_instance_mut(instance_scope) else { return; };
    let Some(state) = state.for_instance_mut(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    invalidations.clear(remove.entity); // 无论是否仍属当前候选，都释放轻量阻断身份。
    settlements.cancel(remove.entity); // 已移除根不再进入后续固定预算查询。
    let Ok(pending) = roots.get(remove.entity) else {
        return; // 已采用或退役根的正常清理不会改变当前事务。
    };
    if !invalidate_candidate_root_readiness(state, remove.entity, pending) {
        return; // 旧代次根移除只清自己的队列状态。
    }
    fail_revision(
        state,  // 当前候选已缺失一个权威World根，不能继续原子采用。
        status, // 发布可见失败并保留上一完整活动批次。
        "显式实例候选WorldAssetRoot在联合结算前被移除".to_owned(), // 精确说明生命周期失败原因。
    );
}

/// 记录当前候选根的Bevy实例已完整生成，不在observer内抢先结算。
pub(super) fn on_placement_instance_ready(
    ready: On<WorldInstanceReady>, // 携带本次完成的根实体和实例ID。
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    mut pending_roots: Query<
        (
            &PlacementPendingRevision,
            &WorldInstance,
            &WorldAssetRoot,
            Option<&mut PlacementBuildingCollisionGeneration>,
        ),
        (With<PlacementPendingInstance>, With<WorldAssetRoot>),
    >, // 公开隔离和私有修订都保留到整批采用，因此每次热更ready都能重新审核。
    mut invalidations: InstanceResMut<PlacementWorldAssetInvalidations>, // 资产根替换后只接受不同于旧票据的新Ready。
    mut asset_index: ResMut<WorldAssetLifecycleIndex>, // Ready同步授予当前资产epoch，不等待根失效扫尾。
    mut state: InstanceResMut<ScenePlacementRuntimeState>,       // observer同步撤销该根旧Ready信用。
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>,     // 新实例完成是候选真实推进而非稳定轮询。
    mut commands: Commands, // 延迟写入门标记，避免与水体observer产生查询冲突。
) {
    let instance_scope = context.instance(ready.entity);
    let Some(invalidations) = invalidations.for_instance_mut(instance_scope) else { return; };
    let Some(state) = state.for_instance_mut(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    let Ok((pending, instance, world_asset, collision_generation)) =
        pending_roots.get_mut(ready.entity)
    else {
        return; // 已采用、已废弃或非placement根不属于本门。
    }; // 完成候选根身份读取。
    if **instance != ready.instance_id {
        return; // 同一根已换过WorldAsset时，旧实例的迟到事件不得误放行。
    } // 完成当前实例ID核对。
    if !invalidations.accept_ready(ready.entity, ready.instance_id) {
        return; // WorldAssetRoot已Discard但spawner尚未写入新票据时，旧Ready不得重新打开门。
    }
    if !invalidate_candidate_root_readiness(state, ready.entity, pending) {
        return; // 事务已换代时不给迟到根留下联合门标记。
    } // 完成上一次就绪信用撤销。
    if let Some(mut collision_generation) = collision_generation {
        collision_generation.world_ready = collision_generation.world_ready.wrapping_add(1).max(1); // 首载与每次真实热更各推进一代，零值只保留给尚未完成。
    }
    let epoch = match asset_index.mark_world_ready(ready.entity, world_asset.0.id()) {
        Ok(epoch) => epoch,
        Err(WorldAssetRootRegistration::Full) => {
            fail_revision(
                state,
                status,
                "显式实例WorldAsset根索引超过固定上限".to_owned(),
            );
            return;
        }
        Err(_) => unreachable!("mark_world_ready只会返回容量失败"),
    };
    commands
        .entity(ready.entity)
        .insert(PlacementWorldInstanceReady(epoch)); // 下一Update只接受同一资产epoch的最终水状态。
    status.advance_progress(); // 当前候选取得了新的World实例完成证据。
} // 结束World实例完成观察。

/// 把一个生命周期变化根加入固定容量结算FIFO，异常爆量时整份修订失败关闭。
fn queue_candidate_settlement(
    entity: Entity,                             // 只定位发生World/水状态变化的精确根。
    pending: &PlacementPendingRevision,         // 入队前核对当前候选修订，避免无关根占用容量。
    settlements: &mut PlacementSettlementQueue, // 同根事件风暴只保留一个双向FIFO席位。
    state: &mut ScenePlacementRuntimeState,       // 队列满时拒绝当前正式候选。
    status: &mut ScenePlacementRuntimeStatus,     // 公开队列硬门诊断。
) {
    let matches_current = state.staging.as_ref().is_some_and(|staging| {
        staging.generation == pending.generation && staging.revision == pending.revision
    }); // 迟到代次不得进入当前队列。
    if !matches_current {
        return; // 已采用、退役或换代根不产生待结算工作。
    }
    if settlements.enqueue(entity) == PlacementSettlementEnqueueResult::Full {
        fail_revision(
            state,  // 超额事件不能截断后继续假装完整。
            status, // F3/F12获得明确硬门诊断。
            format!("显式实例联合结算根超过{MAX_PLACEMENT_SETTLEMENT_ROOTS}项固定上限"), // 固定容量与实现常量一致。
        );
    }
}

/// 为当前候选的一个完整根登记最终Ready信用，并统一执行集合硬上限与heartbeat。
fn grant_candidate_ready(
    entity: Entity,
    pending: &PlacementPendingRevision,
    state: &mut ScenePlacementRuntimeState,
    status: &mut ScenePlacementRuntimeStatus,
) {
    let matches_current = state.staging.as_ref().is_some_and(|staging| {
        staging.generation == pending.generation && staging.revision == pending.revision
    });
    if !matches_current {
        return; // 迟到碰撞或联合结算不能授信给新事务。
    }
    let overflow = state.staging.as_ref().is_some_and(|staging| {
        !staging.ready_roots.contains(&entity)
            && staging.ready_roots.len() >= staging.total_instances
    });
    if overflow {
        fail_revision(
            state,
            status,
            "显式实例候选Ready根集合超出已校验实例总数".to_owned(),
        );
        return;
    }
    let staging = state.staging.as_mut().expect("上方已确认当前候选存在");
    if staging.ready_roots.insert(entity) {
        staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1);
        status.advance_progress(); // 只有首次完整授信才推进停滞门。
    }
}

/// Avian完成建筑层级构造时只发布门组件；Insert观察器在命令落地后读取同帧最终状态。
pub(super) fn on_placement_building_collision_ready(
    ready: On<ColliderConstructorHierarchyReady>,
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    mut roots: Query<
        (
            &PlacementPendingRevision,
            &ScenePlacementInstance,
            &mut PlacementBuildingCollisionGeneration,
        ),
        With<PlacementPendingInstance>,
    >,
    state: InstanceRes<ScenePlacementRuntimeState>,
    mut commands: Commands,
    terrain_shapes: Query<(
        &super::TerrainPatchColliderReady,
        &super::TerrainPatchCollisionRequest,
    )>,
) {
    let instance_scope = context.instance(ready.entity);
    let Some(state) = state.for_instance(instance_scope) else { return; };
    let Ok((pending, instance, mut collision_generation)) = roots.get_mut(ready.entity) else {
        return; // F12预览、已采用或已退役根不参与正式候选门。
    };
    if instance.is_terrain_patch()
        && !terrain_shapes
            .get(ready.entity)
            .is_ok_and(|(shape, request)| shape.matches(request))
    {
        return; // 迟到的通用碰撞完成信号不能给新网格请求授信。
    }
    let matches_current = instance.is_building()
        && state.staging.as_ref().is_some_and(|staging| {
            staging.generation == pending.generation && staging.revision == pending.revision
        });
    if matches_current {
        collision_generation.collision_ready = if instance.is_terrain_patch() {
            // 三维地形自行以网格请求票据拒绝旧完成；同一World代的Mesh热更可多次重烘焙。
            collision_generation.world_ready
        } else {
            collision_generation.collision_ready.wrapping_add(1).max(1)
        }; // 既有建筑继续使用World/Avian完成边沿配对。
        commands
            .entity(ready.entity)
            .insert(PlacementBuildingCollisionReady(
                collision_generation.collision_ready,
            )); // 插入边沿负责后续入队或直接授信。
    }
}

/// 建筑碰撞门落地后，Scene0进入联合结算，直接primitive立即取得完整信用。
pub(super) fn queue_inserted_placement_collision_ready(
    insert: On<Insert<PlacementBuildingCollisionReady>>,
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    roots: Query<
        (
            &PlacementPendingRevision,
            &ScenePlacementInstance,
            &PlacementBuildingCollisionGeneration,
            &PlacementBuildingCollisionReady,
        ),
        With<PlacementPendingInstance>,
    >,
    mut settlements: InstanceResMut<PlacementSettlementQueue>,
    mut state: InstanceResMut<ScenePlacementRuntimeState>,
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>,
) {
    let instance_scope = context.instance(insert.entity);
    let Some(settlements) = settlements.for_instance_mut(instance_scope) else { return; };
    let Some(state) = state.for_instance_mut(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    let Ok((pending, instance, collision_generation, collision_ready)) = roots.get(insert.entity)
    else {
        return;
    };
    if !instance.is_building() {
        return; // 防御外部误插标记，普通资产不能改变既有就绪路径。
    }
    match instance.asset_target() {
        ScenePlacementAssetTarget::Scene0 => queue_candidate_settlement(
            insert.entity,
            pending,
            settlements,
            state,
            status,
        ),
        ScenePlacementAssetTarget::Primitive { .. }
            if collision_ready.0 == collision_generation.collision_ready =>
        {
            grant_candidate_ready(insert.entity, pending, state, status)
        }
        ScenePlacementAssetTarget::Primitive { .. } => {} // 陈旧或异常重复边沿不能授信。
        ScenePlacementAssetTarget::Cuboid { .. } | ScenePlacementAssetTarget::EditableCuboid { .. } | ScenePlacementAssetTarget::EditableMesh { .. } => {} // 有界内置几何不使用World或异步Trimesh完成事件。
    }
}

/// 相关资产epoch推进后按精确根撤销当前候选旧WorldReady；全局采用门已在事件收集时先关闭。
pub(super) fn invalidate_candidate_world_asset_epoch(
    invalidated: On<WorldAssetRootEpochInvalidated>,
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    roots: Query<
        (
            &PlacementPendingRevision,
            &WorldAssetRoot,
            Option<&PlacementWorldInstanceReady>,
        ),
        With<PlacementPendingInstance>,
    >,
    mut settlements: InstanceResMut<PlacementSettlementQueue>,
    mut state: InstanceResMut<ScenePlacementRuntimeState>,
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>,
    mut commands: Commands,
) {
    let instance_scope = context.instance(invalidated.entity);
    let Some(settlements) = settlements.for_instance_mut(instance_scope) else { return; };
    let Some(state) = state.for_instance_mut(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    let Ok((pending, world_asset, world_ready)) = roots.get(invalidated.entity) else {
        return;
    };
    if world_asset.0.id() != invalidated.asset_id
        || world_ready.is_some_and(|ready| ready.0 >= invalidated.epoch)
    {
        return; // 根已迁移资产或同帧新WorldReady已经追平本epoch，不撤销新信用。
    }
    if invalidated.ready_after_event {
        commands
            .entity(invalidated.entity)
            .try_insert(PlacementWorldInstanceReady(invalidated.epoch));
        status.advance_progress();
        return; // 积压期新Ready或debounce抑制后的旧Scene恢复信用，统一重新走联合水状态结算。
    }
    if !invalidate_candidate_root_readiness(state, invalidated.entity, pending) {
        return;
    }
    settlements.cancel(invalidated.entity);
    commands
        .entity(invalidated.entity)
        .try_remove::<PlacementWorldInstanceReady>();
    status.advance_progress();
}

/// World完成门落地时只定向唤醒该根，不立即读取可能尚未落地的水体状态命令。
pub(super) fn queue_inserted_placement_world_ready(
    insert: On<Insert<PlacementWorldInstanceReady>>, // 首次Add和同根重写都统一触发Insert。
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    roots: Query<&PlacementPendingRevision, With<PlacementPendingInstance>>, // 只读取事件实体。
    mut settlements: InstanceResMut<PlacementSettlementQueue>, // 固定容量、按Entity去重。
    mut state: InstanceResMut<ScenePlacementRuntimeState>,     // 只用于当前修订与溢出失败门。
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>,   // 溢出时发布原子失败。
) {
    let instance_scope = context.instance(insert.entity);
    let Some(settlements) = settlements.for_instance_mut(instance_scope) else { return; };
    let Some(state) = state.for_instance_mut(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    let Ok(pending) = roots.get(insert.entity) else {
        return; // 已采用、退役或非placement实体不进入队列。
    };
    queue_candidate_settlement(
        insert.entity,
        pending,
        settlements,
        state,
        status,
    ); // 延后到Update读取同帧最终水状态，消除observer注册顺序依赖。
}

/// 水体Pending/Ready/Failed整组件写入时只唤醒已经取得World完成门的候选根。
pub(super) fn queue_inserted_placement_water_status(
    insert: On<Insert<DynamicWaterMeshOverrideStatus>>, // runtime每次状态发布都使用整组件insert。
    context: feathrise_scene_core::residency::EntitySceneScope, // 迟到完成按实体归属路由，拒绝写入父图或其他副本。
    roots: Query<
        &PlacementPendingRevision,
        (
            With<PlacementPendingInstance>,
            With<PlacementWorldInstanceReady>,
        ),
    >, // World尚未ready时由后续门Insert负责入队，不保存无效中间事件。
    mut settlements: InstanceResMut<PlacementSettlementQueue>,  // 同根多次状态写入只占一个席位。
    mut state: InstanceResMut<ScenePlacementRuntimeState>,        // 核对当前候选修订。
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>,      // 队列溢出时失败关闭。
) {
    let instance_scope = context.instance(insert.entity);
    let Some(settlements) = settlements.for_instance_mut(instance_scope) else { return; };
    let Some(state) = state.for_instance_mut(instance_scope) else { return; };
    let Some(status) = status.for_instance_mut(instance_scope) else { return; };
    let Ok(pending) = roots.get(insert.entity) else {
        return; // 普通正式根、F12预览或尚未World ready根不属于候选结算。
    };
    queue_candidate_settlement(
        insert.entity,
        pending,
        settlements,
        state,
        status,
    ); // 结算系统将读取本帧最终枚举值而不是observer触发时的旧值。
}

/// 仅在World实例和该根的整批赋水都成功后，按固定预算结算定向候选实例。
pub(super) fn process_pending_placement_settlements(
    mut commands: Commands, // Ready后只消费world-ready一次性标记，pending保留到整批采用。
    mut settlements: InstanceResMut<PlacementSettlementQueue>, // 稳定空帧不建立任何全根Query迭代器。
    roots: Query<
        (
            &PlacementPendingRevision,       // 核对它仍属于当前代次和修订。
            &ScenePlacementInstance,           // 建筑根额外要求碰撞层级Ready。
            &DynamicWaterMeshOverrideStatus, // 空覆盖也必须由水体observer明确证明Ready。
            &PlacementWorldInstanceReady,    // 同InstanceId热重载必须核对当前资产epoch。
            Option<&PlacementBuildingCollisionGeneration>, // 建筑Scene热更与碰撞各自推进同一根代次。
            Option<&PlacementBuildingCollisionReady>,      // 普通根为None仍沿用既有就绪语义。
            Option<&WorldAssetRoot>, // 正式根强句柄定位资产epoch；无渲染单测允许epoch零夹具。
        ),
        (
            With<PlacementWorldInstanceReady>,
            With<PlacementPendingInstance>,
        ),
    >, // 层级未完整生成时绝不读取水体结果。
    asset_index: Res<WorldAssetLifecycleIndex>, // O(1)拒绝尚未由新WorldReady追平的旧门标记。
    mut state: InstanceResMut<ScenePlacementRuntimeState>, // 按Entity登记或撤销唯一隐藏候选的Ready信用。
    mut status: InstanceResMut<ScenePlacementRuntimeStatus>, // 赋水失败时发布可见诊断并保留旧完整根。
) {
    for _ in 0..PLACEMENT_SETTLEMENT_BUDGET_PER_FRAME {
        let Some(entity) = settlements.pop_front() else {
            return; // 队列耗尽后立即结束，稳定帧成本与候选总数无关。
        };
        let Ok((
            pending,
            instance,
            water_status,
            world_ready,
            collision_generation,
            collision_ready,
            world_asset,
        )) = roots.get(entity)
        else {
            continue; // 根已退役/移除或门已被另一次结算消费，陈旧事件只释放席位。
        };
        let Some(staging) = state.staging.as_ref() else {
            continue; // 候选已取消时仍按本帧128项预算释放陈旧席位，不能退化成每帧只清一项。
        }; // 完成当前事务读取。
        if staging.generation != pending.generation || staging.revision != pending.revision {
            continue; // 不同代次或修订的迟到根绝不污染当前计数。
        } // 完成候选身份核对。
        let collision_matches_world =
            collision_generation
                .zip(collision_ready)
                .is_some_and(|(generation, ready)| {
                    generation.world_ready > 0 && ready.0 == generation.world_ready
                });
        if instance.is_building() && !collision_matches_world {
            continue; // World或水体先完成、或热更新后仍是旧碰撞时继续等Avian。
        }
        let epoch_is_current = world_asset.is_none_or(|world_asset| {
            asset_index.request_epoch_is_current(entity, world_asset.0.id(), world_ready.0)
        });
        if !epoch_is_current {
            let staging = state.staging.as_mut().expect("上方已确认候选存在");
            if staging.ready_roots.remove(&entity) {
                staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1);
            }
            commands
                .entity(entity)
                .remove::<PlacementWorldInstanceReady>();
            continue; // 资产事件已先推进epoch，旧Status::Ready也不能重新授信。
        }
        if let Some(detail) = water_status.failure_detail() {
            let staging = state.staging.as_mut().expect("上方已确认候选存在");
            if staging.ready_roots.remove(&entity) {
                staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1); // Failed撤销信用时同步废弃在途复验。
            }
            fail_revision(
                &mut state,                                      // 失败状态停止当前候选的继续采用。
                &mut status, // F3/F12可读取精确的网格匹配或属性错误。
                format!("显式实例整网格水体接管失败：{detail}"), // 不丢失水体模块提供的具体节点诊断。
            ); // 完成整份修订失败发布。
            return; // 一个实例失败即足以拒绝整个隐藏候选。
        } // 完成赋水失败门。
        if !water_status.is_ready() {
            let staging = state.staging.as_mut().expect("上方已确认候选存在");
            if staging.ready_roots.remove(&entity) {
                staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1); // Pending仅在真实撤销时换epoch，稳定等待帧不反复重置。
            }
            continue; // Pending根继续等待后续帧，绝不因队列积压提前采用。
        } // 完成赋水就绪检查。
        commands
            .entity(entity) // 本次World实例联合门已消费，根修订身份仍持续存在。
            .remove::<PlacementWorldInstanceReady>(); // 后续热更只能由新`WorldInstanceReady`重新打开门。
        grant_candidate_ready(entity, pending, &mut state, &mut status); // HashSet使重复ready幂等。
    } // 完成本帧固定数量的定向候选根结算。
} // 结束placement联合就绪队列处理。

#[cfg(test)]
#[path = "settlement/tests.rs"]
mod tests; // 生命周期顺序、固定容量与稳定空帧回归独立放置，生产模块保持低于500行。
