//! placement联合门生命周期事件、定向FIFO与固定预算回归测试。

use super::super::components::PlacementInstanceGate; // 测试根与候选批次共享正式Pending门。
use super::*; // 测试直接验证私有联合门状态机和observer入口。
use crate::shared::placement_data::resources::{ScenePlacementRuntimePhase, PlacementStaging}; // 构造最小隐藏候选并读取公开失败阶段。
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldInstanceSpawner}; // 根移除与不可伪造实例票据回归共用正式类型。
use feathrise_scene_content::{SceneId, ScenePlacementHeightMode, ScenePlacementRecord}; // 联合结算查询读取普通非建筑根的完整作者身份。

const TEST_BUILDING_PRESET_REVISION: u64 = 17; // 联合门夹具固定到一份空建筑目录快照。

/// 为联合门测试构造一条普通非建筑 Scene0 记录。
fn ordinary_record(id: &str) -> ScenePlacementRecord {
    ScenePlacementRecord {
        mesh_lod: None,
        terrain_binding: None,
        editor: Default::default(),
        terrain_patch: None,
        id: id.to_owned(),
        asset_path: "gltf/test-settlement.glb".to_owned(),
        asset_target: ScenePlacementAssetTarget::Scene0,
        building_preset_id: None, // 普通根不需要等待建筑碰撞层级门。
        courtyard: None,          // 独立摆放记录不属于任何院落副本。
        translation: [0.0, 0.0, 0.0],
        height_mode: ScenePlacementHeightMode::FixedHeight,
        terrain_offset: 0.0,
        yaw_degrees: 0.0,
        scale: [1.0; 3],
        water_surfaces: Vec::new(),
    }
}

fn ordinary_pending_instance(id: &str, gate: PlacementInstanceGate) -> ScenePlacementInstance {
    let scene_id = SceneId::parse("MAP-settlement-test").expect("测试场景ID合法");
    ScenePlacementInstance::new_pending(&scene_id, &ordinary_record(id), gate)
}

/// 给测试App安装与正式插件相同的定向结算资源、observer和固定预算系统。
fn install_settlement_runtime(app: &mut App) {
    app.init_resource::<ScenePlacementRuntimeState>()
        .init_resource::<ScenePlacementRuntimeStatus>()
        .init_resource::<PlacementSettlementQueue>()
        .init_resource::<PlacementWorldAssetInvalidations>()
        .init_resource::<WorldAssetLifecycleIndex>()
        .add_observer(invalidate_discarded_candidate_world_asset)
        .add_observer(cancel_removed_candidate_world_asset)
        .add_observer(queue_inserted_placement_world_ready)
        .add_observer(queue_inserted_placement_water_status)
        .add_observer(queue_inserted_placement_collision_ready)
        .add_systems(Update, process_pending_placement_settlements); // Update只消费FIFO，不迭代全部候选Query。
}

/// 为碰撞代次回归构造一条 Scene0 建筑记录。
fn building_record(id: &str) -> ScenePlacementRecord {
    let mut record = ordinary_record(id); // 其余字段沿用合法普通Scene0夹具。
    record.building_preset_id = Some("building-test".to_owned()); // 非空预设身份启用建筑联合门。
    record // 返回只改变建筑语义的记录。
}

/// 建立一份只包含指定根的当前隐藏候选事务。
fn state_with_candidate(
    staging_root: Entity,        // 隐藏容器身份。
    candidate: Entity,           // 唯一候选根。
    generation: u32,             // 测试代次。
    revision: u32,               // 测试修订。
    initially_ready: bool,       // 是否模拟上轮World实例已经取得联合信用。
    gate: PlacementInstanceGate, // 与根共享的正式批次门。
) -> ScenePlacementRuntimeState {
    let mut state = ScenePlacementRuntimeState::default(); // 其余运行时字段保持无活动批次默认值。
    let ready_roots = if initially_ready {
        std::iter::once(candidate).collect() // 热重载测试从已有旧Ready信用开始。
    } else {
        Default::default() // 首次加载没有任何根信用。
    };
    state.staging = Some(PlacementStaging {
        reuse: Default::default(),
        terrain_roots: Vec::new(),
        primitive_collision_roots: Vec::new(),
        root: staging_root,     // 当前唯一隐藏容器。
        roots: vec![candidate], // 最终完整复验只允许这一根。
        gate,                   // 候选期作者门保持关闭并与根共享。
        generation,             // 与根组件完全一致。
        revision,               // 与根组件完全一致。
        building_preset_revision: TEST_BUILDING_PRESET_REVISION, // 采用阶段所需建筑目录身份。
        building_clearances: crate::BuildingVegetationClearanceState::empty_regions(), // 本组联合门测试没有建筑。
        building_clearance_floor_heights: Default::default(),
        next_plan: 1,                             // 唯一根已经排入World实例化。
        ready_roots,                              // 按测试场景选择空或旧信用。
        blocked_scene_assets: Default::default(), // 普通联合门夹具没有Scene资产扫尾。
        total_instances: 1,                       // Ready集合硬门固定为一项。
        container_observed: true,                 // 本测试不覆盖容器首次可见探测。
        eligibility_epoch: usize::from(initially_ready) as u64, // 旧信用场景已经经历一次资格变化。
        verification_epoch: 0,                    // 尚未开始最终根表复验。
        verification_cursor: 0,                   // 尚未读取任何根。
    });
    state // 返回可直接插入App的唯一候选状态。
}

#[test]
fn settled_root_keeps_pending_marker_until_whole_candidate_adoption() {
    let mut app = App::new(); // 不安装完整场景插件，隔离联合门行为。
    install_settlement_runtime(&mut app); // observer必须在门组件Insert前注册。
    let staging_root = app.world_mut().spawn_empty().id(); // 隐藏容器只提供事务身份。
    let gate = PlacementInstanceGate::new(false); // 根和候选批次使用同一Pending门。
    let candidate = app
        .world_mut()
        .spawn((
            ordinary_pending_instance("settled-root", gate.clone()), // 生产查询需要普通非建筑根身份。
            PlacementPendingInstance, // 单根结算后仍保持物理候选隔离。
            PlacementPendingRevision {
                generation: 7, // 当前测试代次。
                revision: 3,   // 当前测试修订。
            },
            DynamicWaterMeshOverrideStatus::Ready, // 水体结果已经明确成功。
        ))
        .id(); // 此时没有World门，因此状态observer不会入队。
    app.insert_resource(state_with_candidate(
        staging_root,
        candidate,
        7,
        3,
        false,
        gate,
    )); // 建立当前事务后再打开World门。
    app.world_mut()
        .entity_mut(candidate)
        .insert(PlacementWorldInstanceReady(0)); // 无资产夹具使用初始epoch零。

    assert_eq!(app.world().resource::<PlacementSettlementQueue>().len(), 1); // 结算前没有全根扫描工作。
    app.update(); // 固定预算系统直接读取唯一事件实体。

    let world = app.world(); // 读取结算后的根组件和事务信用。
    assert!(world.get::<PlacementPendingInstance>(candidate).is_some()); // 单根就绪不得暴露隐藏候选给F12热改。
    assert!(world.get::<PlacementPendingRevision>(candidate).is_some()); // 修订身份保留到整批采用。
    assert!(
        world
            .get::<PlacementWorldInstanceReady>(candidate)
            .is_none()
    ); // 一次性World门已经消费。
    assert!(
        world
            .resource::<ScenePlacementRuntimeState>()
            .staging
            .as_ref()
            .expect("候选仍等待整批采用")
            .ready_roots
            .contains(&candidate)
    ); // 当前根取得可撤销的联合Ready信用。
}

#[test]
fn same_frame_world_ready_and_pending_status_are_order_independent() {
    let mut app = App::new(); // 模拟GLB热更同帧由两个observer写入World门与水状态。
    install_settlement_runtime(&mut app); // 使用正式FIFO合并路径。
    let staging_root = app.world_mut().spawn_empty().id(); // 候选容器身份。
    let gate = PlacementInstanceGate::new(false); // 根和候选批次使用同一Pending门。
    let pending = PlacementPendingRevision {
        generation: 8, // 当前测试代次。
        revision: 4,   // 当前测试修订。
    };
    let candidate = app
        .world_mut()
        .spawn((
            ordinary_pending_instance("order-independent", gate.clone()), // 明确走普通根既有联合门。
            PlacementPendingInstance,                                     // 热更期间继续隐藏。
            PlacementPendingRevision {
                generation: pending.generation, // 根代次与事务一致。
                revision: pending.revision,     // 根修订与事务一致。
            },
            DynamicWaterMeshOverrideStatus::Ready, // 模拟上轮旧实例成功状态。
        ))
        .id();
    app.insert_resource(state_with_candidate(
        staging_root,
        candidate,
        8,
        4,
        true,
        gate,
    )); // 旧实例已经授信。
    {
        let mut state = app.world_mut().resource_mut::<ScenePlacementRuntimeState>(); // 新World ready首先撤销旧信用。
        assert!(invalidate_candidate_root_readiness(
            &mut state, candidate, &pending
        )); // 同修订换代必须命中当前根。
    }
    app.world_mut().entity_mut(candidate).insert((
        PlacementWorldInstanceReady(0),          // 新World层级已完成。
        DynamicWaterMeshOverrideStatus::Pending, // 新水体仍在有界预检。
    )); // 两次Insert无论内部组件顺序如何都只占一个FIFO席位。

    assert_eq!(app.world().resource::<PlacementSettlementQueue>().len(), 1); // 同根事件已经合并。
    app.update(); // 处理时读取同帧最终Pending值。
    assert!(
        !app.world()
            .resource::<ScenePlacementRuntimeState>()
            .staging
            .as_ref()
            .expect("候选仍存在")
            .ready_roots
            .contains(&candidate)
    ); // 旧Ready不能因observer顺序短暂回流。
    assert!(
        app.world()
            .get::<PlacementWorldInstanceReady>(candidate)
            .is_some()
    ); // World门保留等待后续水状态事件。

    app.world_mut()
        .entity_mut(candidate)
        .insert(DynamicWaterMeshOverrideStatus::Ready); // 后续水体成功Insert再次定向入队。
    app.update(); // 只有最新最终状态才可授信。
    assert!(
        app.world()
            .resource::<ScenePlacementRuntimeState>()
            .staging
            .as_ref()
            .expect("候选仍存在")
            .ready_roots
            .contains(&candidate)
    ); // 第二次联合门完整成功后恢复Ready。
}

#[test]
fn stale_building_collision_generation_cannot_authorize_reloaded_world() {
    let mut app = App::new(); // 不安装Avian，只直接构造两代完成边沿证据。
    install_settlement_runtime(&mut app); // 使用正式定向联合结算路径。
    let staging_root = app.world_mut().spawn_empty().id(); // 当前隐藏候选容器。
    let gate = PlacementInstanceGate::new(false); // 根和候选共享Pending门。
    let scene_id = SceneId::parse("MAP-settlement-test").expect("测试场景ID合法");
    let candidate = app
        .world_mut()
        .spawn((
            ScenePlacementInstance::new_pending(
                &scene_id,
                &building_record("reloaded-building"),
                gate.clone(),
            ), // 明确启用建筑门。
            PlacementPendingInstance, // 根仍属于隐藏候选。
            PlacementPendingRevision {
                generation: 13, // 当前测试场景代次。
                revision: 9,    // 当前测试文件修订。
            },
            PlacementWorldInstanceReady(0), // 新World已经完成联合结算所需的渲染门。
            DynamicWaterMeshOverrideStatus::Ready, // 本夹具没有水覆盖且已明确Ready。
            PlacementBuildingCollisionGeneration {
                world_ready: 2,     // Scene0已经进入第二代World。
                collision_ready: 1, // Avian仍只完成第一代旧碰撞。
            },
            PlacementBuildingCollisionReady(1), // 陈旧组件不能仅凭存在就授权。
        ))
        .id();
    app.insert_resource(state_with_candidate(
        staging_root,
        candidate,
        13,
        9,
        false,
        gate,
    ));
    assert_eq!(
        app.world_mut()
            .resource_mut::<PlacementSettlementQueue>()
            .enqueue(candidate),
        PlacementSettlementEnqueueResult::Queued
    ); // 模拟新World完成后根已定向入队。

    app.update(); // 旧碰撞代次必须保持候选未授信。
    assert!(
        !app.world()
            .resource::<ScenePlacementRuntimeState>()
            .staging
            .as_ref()
            .expect("候选仍存在")
            .ready_roots
            .contains(&candidate)
    );

    {
        let mut generation = app
            .world_mut()
            .get_mut::<PlacementBuildingCollisionGeneration>(candidate)
            .expect("建筑根必须保存碰撞代次");
        generation.collision_ready = 2; // 模拟Avian完成当前第二代真实网格。
    }
    app.world_mut()
        .entity_mut(candidate)
        .insert(PlacementBuildingCollisionReady(2)); // Insert观察器重新定向唤醒同根。
    app.update();
    assert!(
        app.world()
            .resource::<ScenePlacementRuntimeState>()
            .staging
            .as_ref()
            .expect("候选仍等待整批采用")
            .ready_roots
            .contains(&candidate)
    ); // 只有World与碰撞代次完全相等才授信。
}

#[test]
fn failed_latest_water_keeps_credit_revoked_and_fails_revision() {
    let mut app = App::new(); // 构造一根等待最新水结果的隐藏候选。
    install_settlement_runtime(&mut app); // 使用状态Insert定向事件。
    let staging_root = app.world_mut().spawn_empty().id(); // 失败时保留隐藏容器供修复重试。
    let gate = PlacementInstanceGate::new(false); // 根和候选批次使用同一Pending门。
    let candidate = app
        .world_mut()
        .spawn((
            ordinary_pending_instance("failed-water", gate.clone()), // 普通根只等待World与水状态。
            PlacementPendingInstance,                                // 失败候选不得暴露。
            PlacementPendingRevision {
                generation: 9, // 当前测试代次。
                revision: 5,   // 当前测试修订。
            },
            DynamicWaterMeshOverrideStatus::Pending, // 初值不会在World门之前入队。
        ))
        .id();
    app.insert_resource(state_with_candidate(
        staging_root,
        candidate,
        9,
        5,
        false,
        gate,
    )); // 安装当前候选事务。
    app.world_mut().entity_mut(candidate).insert((
        PlacementWorldInstanceReady(0), // World门已经完成。
        DynamicWaterMeshOverrideStatus::Failed("新GLB缺少有效UV0".to_owned()), // 最新水预检明确失败。
    ));

    app.update(); // 固定预算结算必须拒绝整份修订。

    let world = app.world(); // 读取失败诊断和Ready集合。
    assert!(
        !world
            .resource::<ScenePlacementRuntimeState>()
            .staging
            .as_ref()
            .expect("失败候选保留等待修复")
            .ready_roots
            .contains(&candidate)
    ); // Failed根不得取得任何旧信用。
    assert_eq!(
        world.resource::<ScenePlacementRuntimeStatus>().phase,
        ScenePlacementRuntimePhase::Failed
    ); // 可见状态必须阻止候选原子采用。
    assert!(
        world
            .resource::<ScenePlacementRuntimeStatus>()
            .detail
            .contains("缺少有效UV0")
    ); // 保留水体模块精确诊断。
}

#[test]
fn settlement_fifo_covers_8192_legal_roots_and_has_o1_cancellation() {
    let mut queue = PlacementSettlementQueue::default(); // 纯队列测试不创建任何ECS根组件。
    let mut roots = Vec::with_capacity(MAX_PLACEMENT_SETTLEMENT_ROOTS); // 保存合法Entity供中间项取消。
    for raw in 1..=MAX_PLACEMENT_SETTLEMENT_ROOTS {
        let entity = Entity::from_raw_u32(raw as u32).expect("测试Entity索引必须合法"); // 每项拥有不同代次内索引。
        roots.push(entity); // 后续精确取消中间根。
        assert_eq!(
            queue.enqueue(entity),
            PlacementSettlementEnqueueResult::Queued
        ); // 上限内全部入队。
    }
    let overflow = Entity::from_raw_u32(9_001).expect("溢出测试Entity必须合法"); // 不与上限内根重复。
    assert_eq!(
        queue.enqueue(overflow),
        PlacementSettlementEnqueueResult::Full
    ); // 第8193个唯一根失败关闭。
    assert!(queue.cancel(roots[MAX_PLACEMENT_SETTLEMENT_ROOTS / 2])); // 双向链允许O(1)取消任意中间项。
    assert_eq!(
        queue.enqueue(overflow),
        PlacementSettlementEnqueueResult::Queued
    ); // 释放一个席位后可精确接纳新根。
    assert_eq!(queue.len(), MAX_PLACEMENT_SETTLEMENT_ROOTS); // 内存硬门始终不变。
}

#[test]
fn candidate_asset_discard_blocks_same_frame_old_ready_ticket() {
    let root = Entity::from_raw_u32(9_002).expect("换代测试Entity必须合法"); // 资源只按稳定Entity保存阻断身份。
    let mut spawner = WorldInstanceSpawner::default(); // 用Bevy公开API生成不可伪造的两个实例票据。
    let old_instance = spawner.spawn(Handle::<WorldAsset>::default()); // 模拟Discard发生时根上的旧票据。
    let new_instance = spawner.spawn(Handle::<WorldAsset>::default()); // 模拟World spawner随后写入的新票据。
    let mut invalidations = PlacementWorldAssetInvalidations::default(); // 独立验证同帧observer顺序门。
    invalidations.block(root, old_instance); // WorldAssetRoot Discard先冻结旧实例身份。
    assert!(!invalidations.accept_ready(root, old_instance)); // 已排队的旧Ready即使迟到也不能重新打开候选门。
    assert!(invalidations.accept_ready(root, new_instance)); // 不同InstanceId首次Ready才结束换代间隙。
    assert!(invalidations.accept_ready(root, new_instance)); // 阻断消费后同实例GLB热重载Ready仍保持合法。
}

#[test]
fn removing_world_asset_root_cancels_queued_settlement_and_fails_current_candidate() {
    let mut app = App::new(); // 验证根真实Remove不留下陈旧FIFO席位。
    install_settlement_runtime(&mut app); // 注册WorldAssetRoot生命周期observer。
    let staging_root = app.world_mut().spawn_empty().id(); // 当前隐藏容器。
    let gate = PlacementInstanceGate::new(false); // 根与当前候选共享正式Pending门。
    let candidate = app
        .world_mut()
        .spawn((
            ordinary_pending_instance("removed-world-root", gate.clone()), // 生命周期夹具保持生产根结构。
            WorldAssetRoot(Handle::<WorldAsset>::default()), // 被移除的权威Scene0根组件。
            PlacementPendingInstance,                        // 明确属于隐藏候选。
            PlacementPendingRevision {
                generation: 10, // 当前测试代次。
                revision: 6,    // 当前测试修订。
            },
        ))
        .id();
    app.insert_resource(state_with_candidate(
        staging_root,
        candidate,
        10,
        6,
        false,
        gate,
    )); // 根移除必须影响这份当前事务。
    assert_eq!(
        app.world_mut()
            .resource_mut::<PlacementSettlementQueue>()
            .enqueue(candidate),
        PlacementSettlementEnqueueResult::Queued
    ); // 模拟根已有一个待结算生命周期事件。

    app.world_mut()
        .entity_mut(candidate)
        .remove::<WorldAssetRoot>(); // Discard后紧跟Remove；无WorldInstance时仍由Remove完成取消。

    assert_eq!(app.world().resource::<PlacementSettlementQueue>().len(), 0); // 被移除根立即释放FIFO容量。
    assert_eq!(
        app.world().resource::<ScenePlacementRuntimeStatus>().phase,
        ScenePlacementRuntimePhase::Failed
    ); // 残缺候选不能继续等待或原子采用。
}

#[test]
fn stable_frames_with_8192_pending_roots_leave_directed_queue_empty() {
    let mut app = App::new(); // 构造场景允许的最坏普通placement根数量。
    install_settlement_runtime(&mut app); // 生产系统只有空FIFO检查。
    for index in 0..8_192usize {
        app.world_mut().spawn((
            PlacementPendingInstance, // 根仍处于隐藏候选阶段。
            PlacementPendingRevision {
                generation: 11, // 所有根共享同一测试代次。
                revision: 7,    // 所有根共享同一测试修订。
            },
            DynamicWaterMeshOverrideStatus::Pending, // 没有World门时状态Insert不会进入结算FIFO。
            Name::new(format!("稳定根_{index}")),    // 保证实体真实存在且易于失败定位。
        ));
    }
    let progress_before = app
        .world()
        .resource::<ScenePlacementRuntimeStatus>()
        .progress_revision; // 冻结稳定帧heartbeat。
    for _ in 0..300usize {
        app.update(); // 空FIFO每帧只做一次Option检查，不读取8192个根。
    }
    assert_eq!(app.world().resource::<PlacementSettlementQueue>().len(), 0); // 稳定Pending不会自我重排队。
    assert_eq!(
        app.world()
            .resource::<ScenePlacementRuntimeStatus>()
            .progress_revision,
        progress_before
    ); // 稳定空帧既不扫描产生副作用，也不伪造真实进展。
    let source = include_str!("../settlement.rs"); // 同时锁定生产模块不回退到变化过滤全原型Query。
    assert!(!source.contains(concat!("Changed<", "WorldInstance>"))); // World换代只允许生命周期observer定向处理。
}

#[test]
fn cancelled_candidate_retires_stale_settlements_at_128_per_frame() {
    let mut app = App::new(); // 数据换代可能留下本帧尚未消费的旧根事件。
    install_settlement_runtime(&mut app); // 当前state保持没有staging，模拟候选已经O(1)取消。
    let mut roots = Vec::with_capacity(PLACEMENT_SETTLEMENT_BUDGET_PER_FRAME * 2); // 精确建立两帧退役工作。
    for _ in 0..PLACEMENT_SETTLEMENT_BUDGET_PER_FRAME * 2 {
        let entity = app
            .world_mut()
            .spawn((
                PlacementPendingInstance, // 陈旧物理标记可能等待树回收器稍后移除。
                PlacementPendingRevision {
                    generation: 12, // 已取消旧代次。
                    revision: 8,    // 已取消旧修订。
                },
                PlacementWorldInstanceReady(0), // 确保处理Query能命中并进入无staging分支。
                DynamicWaterMeshOverrideStatus::Ready, // 陈旧成功值不得产生新信用。
            ))
            .id();
        roots.push(entity); // 队列随后按这些稳定Entity定向退役。
    }
    {
        let mut queue = app.world_mut().resource_mut::<PlacementSettlementQueue>(); // 生命周期observer因无当前staging没有自动入队。
        for entity in roots {
            assert_eq!(
                queue.enqueue(entity),
                PlacementSettlementEnqueueResult::Queued
            ); // 手动模拟取消前已经积累的事件。
        }
    }

    app.update(); // 第一帧只能消费固定128项。
    assert_eq!(
        app.world().resource::<PlacementSettlementQueue>().len(),
        PLACEMENT_SETTLEMENT_BUDGET_PER_FRAME
    ); // 不能退化成每帧只清理一个陈旧根。
    app.update(); // 第二帧消费剩余固定预算。
    assert_eq!(app.world().resource::<PlacementSettlementQueue>().len(), 0); // 两帧后队列完整释放。
}
