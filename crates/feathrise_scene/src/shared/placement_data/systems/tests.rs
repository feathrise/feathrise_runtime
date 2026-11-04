//! 显式placement校验、修订证据与候选原子采用回归。

mod incremental_planning; // 八千记录分帧、换代取消与Scene轮询预算使用独立大规模夹具。
mod author_preview_ownership; // 真实正式候选采用与退役不能吞掉独立作者根或解除其逐根遮蔽。

use feathrise_scene_core::residency::{InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use super::*; // 直接验证私有候选辅助函数和组件生命期。
use bevy::asset::AssetPlugin;
use bevy::world_serialization::{WorldAsset, WorldInstance, WorldSerializationPlugin};

const TEST_BUILDING_PRESET_REVISION: u64 = 17; // 候选夹具固定到同一建筑目录快照。

#[test]
fn placement_container_without_scene_scope_is_retired_with_its_map() {
    use bevy::state::app::StatesPlugin;
    use feathrise_scene_core::residency::{ResidentMapEntity, SceneResidencyActivity};
    use crate::shared::scene_residency::{close_resident_scene, mark_resident_document_open, ResidentScenes};
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), StatesPlugin))
        .init_state::<ScenePhase>()
        .init_resource::<ActiveDynamicMap>()
        .init_resource::<Assets<Image>>();
    crate::shared::scene_residency::install(&mut app);
    crate::register_resident_scene_root::<PlacementStagingRoot>(&mut app);
    app.world_mut().resource_mut::<SceneResidencyActivity>().enabled = true;
    let map = MapId::parse("MAP-empty-placement-lifetime").unwrap();
    app.world_mut().resource_mut::<ActiveDynamicMap>().select(map.clone());
    app.world_mut().resource_mut::<NextState<ScenePhase>>().set(ScenePhase::Active);
    app.update();
    mark_resident_document_open(app.world_mut(), map.clone());
    let root = app.world_mut().spawn((PlacementStagingRoot, Transform::default(), Visibility::Inherited)).id();
    let child = app.world_mut().spawn((Transform::default(), ChildOf(root))).id();
    app.update();
    assert!(app.world().get::<ResidentMapEntity>(root).is_some());
    assert!(app.world().get::<DespawnOnExit<ScenePhase>>(root).is_none());
    close_resident_scene(app.world_mut(), map.clone());
    app.world_mut().resource_mut::<NextState<ScenePhase>>().set(ScenePhase::Idle);
    for _ in 0..100 {
        app.update();
        if !app.world().resource::<ResidentScenes>().contains(&map) { break; }
    }
    assert!(app.world().get_entity(root).is_err() && app.world().get_entity(child).is_err(),
        "空候选根与普通子节点必须由同一地图的固定预算回收");
}

#[test]
fn placement_paths_stay_inside_assets_and_use_scene_formats() {
    assert!(validate_asset_path("gltf/树木/tree.glb"));
    assert!(validate_asset_path("maps/MAP-1/private.gltf"));
    assert!(!validate_asset_path("../secret.glb"));
    assert!(!validate_asset_path("textures/tree.png"));
}

#[test]
fn placement_attempt_revision_advances_before_a_candidate_can_become_ready() {
    let mut state = MapPlacementRuntimeState::default();
    let mut status = MapPlacementRuntimeStatus::default();

    begin_placement_revision(&mut state, &mut status);

    assert_eq!(state.revision, 1); // 私有候选取得本次完整构建身份。
    assert_eq!(status.attempt_revision, 1); // 保存门可立即区分发布后的新尝试。
    assert_eq!(status.progress_revision, 1); // 新计划尝试同时形成一次真实停滞门推进。
    assert_eq!(status.revision, 0); // 公开采用修订只能由完整候选原子替换后推进。
}

#[test]
fn world_asset_lifecycle_revokes_ready_credit_before_candidate_finish() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        WorldSerializationPlugin,
    )); // 用Bevy公开Spawner真实生成两个不可伪造的InstanceId。
    let (first, second) = {
        let mut assets = app.world_mut().resource_mut::<Assets<WorldAsset>>();
        (
            assets.add(WorldAsset::new(World::new())),
            assets.add(WorldAsset::new(World::new())),
        )
    };
    let staging_root = app
        .world_mut()
        .spawn((PlacementStagingRoot, Visibility::Hidden))
        .id();
    let candidate = app
        .world_mut()
        .spawn((
            WorldAssetRoot(first),
            PlacementPendingInstance,
            PlacementPendingRevision {
                generation: 17,
                revision: 4,
            },
            ChildOf(staging_root),
        ))
        .id();
    app.update(); // SpawnScene首次给根安装WorldInstance；本测试不触发ready observer。
    let first_instance = **app
        .world()
        .get::<WorldInstance>(candidate)
        .expect("首个WorldInstance必须已经安装");
    let map_id = MapId::parse("MAP-world-instance-revision").expect("测试地图ID合法");
    let gate = PlacementInstanceGate::new(false);
    let mut state = MapPlacementRuntimeState::default();
    state.active_map_id = Some(map_id.clone());
    state.pending = Some(PendingPlacementRevision {
        terrain_holes: Default::default(),
        terrain_patch_count: 0,
        generation: 17,
        revision: 4,
        map_id,
        building_preset_revision: TEST_BUILDING_PRESET_REVISION,
        building_clearances: BuildingVegetationClearanceState::empty_regions(),
        building_clearance_floor_heights: Default::default(),
        plans: Vec::new(),
    });
    state.staging = Some(PlacementStaging {
        reuse: Default::default(),
        terrain_roots: Vec::new(),
        root: staging_root,
        roots: vec![candidate],
        gate,
        generation: 17,
        revision: 4,
        building_preset_revision: TEST_BUILDING_PRESET_REVISION,
        building_clearances: BuildingVegetationClearanceState::empty_regions(),
        building_clearance_floor_heights: Default::default(),
        next_plan: 1,
        ready_roots: std::iter::once(candidate).collect(),
        blocked_scene_assets: Default::default(),
        total_instances: 1,
        container_observed: true,
        eligibility_epoch: 3,
        verification_epoch: 3,
        verification_cursor: 1, // 模拟旧信用已经走完最终复验，下一次finish本可直接采用。
    });
    app.insert_resource(state)
        .init_resource::<MapPlacementRuntimeStatus>()
        .init_resource::<BuildingVegetationClearanceState>()
        .init_resource::<super::super::resources::PlacementSettlementQueue>()
        .init_resource::<super::super::resources::PlacementWorldAssetInvalidations>()
        .add_observer(super::super::settlement::invalidate_discarded_candidate_world_asset)
        .add_observer(super::super::settlement::cancel_removed_candidate_world_asset); // 根替换事件必须在Spawner改写WorldInstance前同步撤销旧信用。

    app.world_mut()
        .entity_mut(candidate)
        .insert(WorldAssetRoot(second));
    app.update(); // SpawnScene先换成新InstanceId；故意没有新WorldInstanceReady observer。
    let second_instance = **app
        .world()
        .get::<WorldInstance>(candidate)
        .expect("换代后的WorldInstance必须仍存在");
    assert_ne!(first_instance, second_instance);

    app.add_systems(Update, finish_once); // 生命周期observer已经在根替换操作发生时完成失效，无需稳定帧查询。
    app.update();

    let state = app.world().resource::<MapPlacementRuntimeState>();
    let staging = state
        .staging
        .as_ref()
        .expect("新实例尚未Ready，候选必须保留");
    assert!(!staging.ready_roots.contains(&candidate)); // 旧实例信用已经立即撤销。
    assert_eq!(staging.verification_cursor, 0); // 已完成的旧复验前缀也必须作废。
    assert!(state.active.is_none()); // 新Ready到达前绝不能原子采用旧信用候选。
}

fn finish_once(
    mut commands: Commands,
    mut state: InstanceResMut<MapPlacementRuntimeState>,
    mut status: InstanceResMut<MapPlacementRuntimeStatus>,
    mut building_clearances: InstanceResMut<BuildingVegetationClearanceState>,
    mut visibilities: feathrise_scene_core::residency::InstanceQuery<&mut Visibility, With<PlacementStagingRoot>>,
) {
    finish_placement_candidate(
        &mut commands,
        &mut state,
        &mut status,
        &mut building_clearances,
        &mut visibilities,
    );
}

fn placement_record(id: impl Into<String>) -> MapPlacementRecord {
    MapPlacementRecord {
        mesh_lod: None,
        terrain_binding: None,
        editor: Default::default(),
        terrain_patch: None,
        id: id.into(), // 测试根使用调用方指定的稳定记录ID。
        asset_path: "gltf/test-placement.glb".to_owned(), // 合法Scene0来源供根证据冻结。
        asset_target: Default::default(), // 默认目标保持既有Scene0候选语义。
        building_preset_id: None, // 普通批次夹具不声明建筑预设身份。
        courtyard: None, // 独立摆放记录不属于任何院落副本。
        translation: [0.0, 0.0, 0.0], // 候选生命周期测试不需要位移。
        height_mode: feathrise_map_content::MapPlacementHeightMode::FixedHeight, // 运行时批次测试使用确定世界Y。
        terrain_offset: 0.0,        // 固定高度不携带离地偏移。
        yaw_degrees: 0.0,           // 候选生命周期测试不需要旋转。
        scale: [1.0, 1.0, 1.0],     // 单位缩放保持记录有效。
        water_surfaces: Vec::new(), // 这些测试只验证批次门，不进入整网格水队列。
    }
}

#[test]
fn placement_batch_gate_distinguishes_pending_active_and_retired_roots() {
    let map_id = MapId::parse("MAP-placement-gate").expect("测试地图ID合法");
    let record = placement_record("gate-root"); // 三态变化期间不可变来源证据保持同一快照。
    let gate = PlacementInstanceGate::new(false); // 正式隐藏候选从Pending开始。
    let instance = MapPlacementInstance::new_pending(&map_id, &record, gate.clone());

    assert!(instance.is_pending()); // 当前候选可以参与同快照保存等待。
    assert!(!instance.is_active()); // 候选尚不可被F12写回。
    gate.activate(); // 模拟整批容器可见后的O(1)正式采用。
    assert!(!instance.is_pending()); // 已采用根不再冒充隐藏候选。
    assert!(instance.is_active()); // 整批根同时成为作者活动集合。
    gate.retire(); // 模拟换图或新修订替换后的O(1)退役。
    assert!(!instance.is_pending()); // 物理Pending标记即使残留也不能恢复候选信用。
    assert!(!instance.is_active()); // 退役根不再提供保存或作者写回资格。
}

fn candidate_app(total: usize, ready: bool) -> (App, Vec<Entity>) {
    let mut app = App::new();
    let staging_root = app
        .world_mut()
        .spawn((PlacementStagingRoot, Visibility::Hidden))
        .id();
    let gate = PlacementInstanceGate::new(false);
    let map_id = MapId::parse("MAP-pending-adoption").expect("测试地图ID合法");
    let roots = (0..total)
        .map(|index| {
            let record = placement_record(format!("candidate-{index}")); // 每根冻结自己的完整作者记录。
            app.world_mut()
                .spawn((
                    MapPlacementInstance::new_pending(&map_id, &record, gate.clone()),
                    PlacementPendingInstance,
                    PlacementPendingRevision {
                        generation: 5,
                        revision: 9,
                    },
                    ChildOf(staging_root),
                ))
                .id()
        })
        .collect::<Vec<_>>();
    let mut state = MapPlacementRuntimeState::default();
    state.staging = Some(PlacementStaging {
        reuse: Default::default(),
        terrain_roots: Vec::new(),
        root: staging_root,
        roots: roots.clone(),
        gate,
        generation: 5,
        revision: 9,
        building_preset_revision: TEST_BUILDING_PRESET_REVISION,
        building_clearances: BuildingVegetationClearanceState::empty_regions(),
        building_clearance_floor_heights: Default::default(),
        next_plan: total,
        ready_roots: ready
            .then(|| roots.iter().copied().collect())
            .unwrap_or_default(),
        blocked_scene_assets: Default::default(),
        total_instances: total,
        container_observed: true, // 测试容器已由World直接spawn，可立即进入复验。
        eligibility_epoch: u64::from(ready),
        verification_epoch: u64::from(ready),
        verification_cursor: 0,
    });
    state.active_map_id = Some(map_id.clone());
    state.pending = Some(PendingPlacementRevision {
        terrain_holes: Default::default(),
        terrain_patch_count: 0,
        generation: 5,
        revision: 9,
        map_id,
        building_preset_revision: TEST_BUILDING_PRESET_REVISION,
        building_clearances: BuildingVegetationClearanceState::empty_regions(),
        building_clearance_floor_heights: Default::default(),
        plans: Vec::new(), // finish只消费已建立候选的地图身份，不回读计划。
    });
    app.insert_resource(state)
        .init_resource::<MapPlacementRuntimeStatus>()
        .init_resource::<BuildingVegetationClearanceState>();
    (app, roots)
}

fn spawn_and_finish_zero_candidate(
    mut commands: Commands,
    mut state: InstanceResMut<MapPlacementRuntimeState>,
    mut status: InstanceResMut<MapPlacementRuntimeStatus>,
    mut building_clearances: InstanceResMut<BuildingVegetationClearanceState>,
    mut visibilities: feathrise_scene_core::residency::InstanceQuery<&mut Visibility, With<PlacementStagingRoot>>,
) {
    if state.staging.is_none() && state.active.is_none() {
        let map_id = MapId::parse("MAP-empty-placement").expect("测试地图ID合法");
        let root = commands
            .spawn((PlacementStagingRoot, Visibility::Hidden))
            .id(); // 生产路径同样只拿到Commands预留Entity，本帧Query尚不可见。
        state.active_map_id = Some(map_id.clone());
        state.pending = Some(PendingPlacementRevision {
            terrain_holes: Default::default(),
            terrain_patch_count: 0,
            generation: 2,
            revision: 4,
            map_id,
            building_preset_revision: TEST_BUILDING_PRESET_REVISION,
            building_clearances: BuildingVegetationClearanceState::empty_regions(),
            building_clearance_floor_heights: Default::default(),
            plans: Vec::new(),
        });
        state.staging = Some(PlacementStaging {
            reuse: Default::default(),
            terrain_roots: Vec::new(),
            root,
            roots: Vec::new(),
            gate: PlacementInstanceGate::new(false),
            generation: 2,
            revision: 4,
            building_preset_revision: TEST_BUILDING_PRESET_REVISION,
            building_clearances: BuildingVegetationClearanceState::empty_regions(),
            building_clearance_floor_heights: Default::default(),
            next_plan: 0,
            ready_roots: Default::default(),
            blocked_scene_assets: Default::default(),
            total_instances: 0,
            container_observed: false,
            eligibility_epoch: 0,
            verification_epoch: 0,
            verification_cursor: 0,
        });
    }
    finish_placement_candidate(
        &mut commands,
        &mut state,
        &mut status,
        &mut building_clearances,
        &mut visibilities,
    );
}

#[test]
fn zero_placement_candidate_waits_until_deferred_container_really_exists() {
    let mut app = App::new();
    app.init_resource::<MapPlacementRuntimeState>()
        .init_resource::<MapPlacementRuntimeStatus>()
        .init_resource::<BuildingVegetationClearanceState>()
        .add_systems(Update, spawn_and_finish_zero_candidate);

    app.update();
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().phase,
        MapPlacementRuntimePhase::Building
    ); // 首帧只创建Commands实体，不得把Query不可见误报Failed或提前Ready。
    assert!(
        app.world()
            .resource::<MapPlacementRuntimeState>()
            .staging
            .is_some()
    );

    app.update();
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().phase,
        MapPlacementRuntimePhase::Ready
    ); // 下一帧真实观察到容器后，空批次以常数时间安全采用。
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        1
    );
}

#[test]
fn whole_candidate_adoption_releases_pending_marker_from_every_known_root() {
    let (mut app, roots) = candidate_app(1, true);
    let candidate = roots[0];
    let staging_root = app
        .world()
        .resource::<MapPlacementRuntimeState>()
        .staging
        .as_ref()
        .expect("测试候选存在")
        .root;
    app.add_systems(
        Update,
        (
            finish_once,
            cleanup_active_placement_markers.after(finish_once),
        ),
    );

    app.update();

    assert!(
        app.world()
            .get::<PlacementPendingInstance>(candidate)
            .is_none()
    ); // 只有整批就绪后才对F12暴露为已采用正式根。
    assert!(
        app.world()
            .get::<PlacementPendingRevision>(candidate)
            .is_none()
    );
    assert_eq!(
        app.world()
            .resource::<MapPlacementRuntimeState>()
            .active
            .as_ref()
            .map(|active| active.root),
        Some(staging_root),
    );
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().phase,
        MapPlacementRuntimePhase::Ready,
    );
    let map_id = MapId::parse("MAP-pending-adoption").expect("测试地图ID合法");
    let clearance = app
        .world()
        .resource::<BuildingVegetationClearanceState>()
        .snapshot_for(&map_id)
        .expect("无建筑批次也必须发布已完成的空净空快照");
    assert!(clearance.regions.is_empty()); // 删除最后建筑后消费者据此恢复全部植被。
}

#[test]
fn candidate_adoption_and_retirement_toggle_the_single_shared_builtin_body() {
    use avian3d::prelude::{RigidBody, RigidBodyDisabled};
    let (mut app, _) = candidate_app(1, true);
    let root = app.world().resource::<MapPlacementRuntimeState>().staging.as_ref().unwrap().root;
    app.world_mut().entity_mut(root).insert((RigidBody::Static, RigidBodyDisabled));
    app.add_systems(Update, finish_once);
    assert!(app.world().get::<RigidBodyDisabled>(root).is_some());
    app.update();
    assert!(app.world().get::<RigidBodyDisabled>(root).is_none());
    app.add_systems(Update, |mut commands: Commands, mut state: InstanceResMut<MapPlacementRuntimeState>| {
        retirement::retire_active_batch(&mut commands, &mut state);
    });
    app.update();
    assert!(app.world().get::<RigidBodyDisabled>(root).is_some());
    assert_eq!(app.world().get::<Visibility>(root), Some(&Visibility::Hidden));
}

#[test]
fn instance_placement_hot_updates_keep_adopted_layer_but_allow_the_initial_candidate() {
    let (mut app, roots) = candidate_app(1, true);
    assert!(!keep_adopted_instance_placements(true, app.world().resource::<MapPlacementRuntimeState>()));
    app.add_systems(Update, finish_once);
    app.update(); // 真正经过完整候选复验和原子采用后，才开始冻结旧子实例的内容层。
    app.world_mut().resource_mut::<MapPlacementRuntimeState>().dirty = true;
    let state = app.world().resource::<MapPlacementRuntimeState>();
    assert!(keep_adopted_instance_placements(true, state));
    assert!(!keep_adopted_instance_placements(false, state), "父地图继续采用原有热更新事务");
    assert!(app.world().get::<MapPlacementInstance>(roots[0]).unwrap().is_active());
    assert_eq!(app.world().resource::<MapPlacementRuntimeStatus>().phase, MapPlacementRuntimePhase::Ready);
}

#[test]
fn candidate_with_missing_current_root_credit_cannot_be_adopted() {
    let (mut app, roots) = candidate_app(1, false); // 构造数量已排入但当前根Ready信用被热更撤销的候选。
    let candidate = roots[0];
    app.add_systems(Update, finish_once); // 只运行最终原子采用门。

    app.update(); // 缺失当前根信用时必须继续Building。

    let world = app.world(); // 读取本帧候选与公开状态。
    assert!(
        world
            .resource::<MapPlacementRuntimeState>()
            .staging
            .is_some()
    ); // 候选未被提前消费。
    assert_eq!(
        world
            .resource::<MapPlacementRuntimeState>()
            .active
            .as_ref()
            .map(|active| active.root),
        None
    ); // 旧Ready标量不得导致新根采用。
    assert!(world.get::<PlacementPendingInstance>(candidate).is_some()); // 隔离标记仍保留。
    assert!(world.get::<PlacementPendingRevision>(candidate).is_some()); // 修订身份仍可接纳后续World ready。
    assert_eq!(
        world.resource::<MapPlacementRuntimeStatus>().phase,
        MapPlacementRuntimePhase::Building
    ); // 可见状态准确表示尚在联合就绪。
}

#[test]
fn large_candidate_verifies_and_cleans_markers_across_frames_without_partial_author_identity() {
    let total = PLACEMENT_READY_VERIFY_BUDGET_PER_FRAME + 1;
    let (mut app, roots) = candidate_app(total, true);
    app.add_systems(
        Update,
        (
            finish_once,
            cleanup_active_placement_markers.after(finish_once),
        ),
    );

    app.update(); // 第一帧只核对固定前缀，不能采用。
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().phase,
        MapPlacementRuntimePhase::Building
    );
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        0
    );
    assert!(roots.iter().all(|root| {
        !app.world()
            .get::<MapPlacementInstance>(*root)
            .expect("候选根仍存在")
            .is_active()
    })); // 复验中全部根共享关闭门，不按已核对前缀逐根暴露。

    app.update(); // 第二帧完成复验并O(1)采用，随后只清首批物理标记。
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().phase,
        MapPlacementRuntimePhase::Ready
    );
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        1
    );
    assert!(roots.iter().all(|root| {
        app.world()
            .get::<MapPlacementInstance>(*root)
            .expect("活动根仍存在")
            .is_active()
    })); // 即使物理标记只清了一部分，F12看到的整批作者身份也已同时切换。
    assert_eq!(
        roots
            .iter()
            .filter(|root| app
                .world()
                .get::<PlacementPendingInstance>(**root)
                .is_some())
            .count(),
        1
    ); // 证明测试确实处于部分物理清理的中间帧。

    app.update(); // 剩余标记收口不得重复推进公开采用修订。
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        1
    );
    assert!(
        roots
            .iter()
            .all(|root| app.world().get::<PlacementPendingInstance>(*root).is_none())
    );
}

#[test]
fn hot_reload_revokes_an_in_progress_full_ready_verification() {
    let total = PLACEMENT_READY_VERIFY_BUDGET_PER_FRAME + 1;
    let (mut app, roots) = candidate_app(total, true);
    app.add_systems(Update, finish_once);

    app.update(); // 已核对前128根，但尚未完成整批证明。
    {
        let mut state = app.world_mut().resource_mut::<MapPlacementRuntimeState>();
        let staging = state.staging.as_mut().expect("候选仍在复验");
        assert!(staging.ready_roots.remove(&roots[0])); // 模拟新WorldInstanceReady先撤销旧水体信用。
        staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1);
    }
    app.update(); // 缺失最新信用时必须复位游标且保持隐藏。
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        0
    );
    assert!(
        !app.world()
            .get::<MapPlacementInstance>(roots[0])
            .expect("热更根仍在候选中")
            .is_active()
    );

    {
        let mut state = app.world_mut().resource_mut::<MapPlacementRuntimeState>();
        let staging = state.staging.as_mut().expect("候选等待最新Ready");
        assert!(staging.ready_roots.insert(roots[0])); // 模拟最新World和整网格水重新联合Ready。
        staging.eligibility_epoch = staging.eligibility_epoch.wrapping_add(1);
    }
    app.update(); // 新epoch第一帧仍只复验固定前缀。
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        0
    );
    app.update(); // 只有新epoch完整跨帧复验后才允许采用一次。
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        1
    );
    app.update();
    assert_eq!(
        app.world().resource::<MapPlacementRuntimeStatus>().revision,
        1
    ); // 候选已被消费，稳定帧不能重复采用。
}

#[test]
fn retired_wide_tree_is_detached_and_destroyed_with_a_fixed_multiframe_budget() {
    let mut app = App::new();
    let root = app.world_mut().spawn(Visibility::Hidden).id();
    let children = (0..=PLACEMENT_TREE_CLEANUP_BUDGET_PER_FRAME)
        .map(|_| {
            app.world_mut()
                .spawn((Visibility::Inherited, ChildOf(root)))
                .id()
        })
        .collect::<Vec<_>>();
    let before = app.world().iter_entities().count();
    let mut state = MapPlacementRuntimeState::default();
    state
        .retired_trees
        .push_back(PlacementTreeCleanup::new(root, Vec::new()));
    app.insert_resource(state)
        .add_systems(Update, cleanup_retired_placement_trees);

    app.update();
    let after_first = app.world().iter_entities().count();
    assert!(app.world().get_entity(root).is_ok()); // 宽根不能在第一帧借linked_spawn递归销毁整树。
    assert!(after_first < before);
    assert!(before - after_first <= PLACEMENT_TREE_CLEANUP_BUDGET_PER_FRAME); // 单帧销毁实体数不会超过总操作预算。
    assert!(
        children
            .iter()
            .any(|child| app.world().get_entity(*child).is_ok())
    ); // 超预算子节点明确留到后续帧。

    for _ in 0..8 {
        app.update(); // 固定预算最终仍能完整收敛，不遗留脱离父级的孤儿。
    }
    assert!(app.world().get_entity(root).is_err());
    assert!(
        children
            .iter()
            .all(|child| app.world().get_entity(*child).is_err())
    );
    assert!(
        app.world()
            .resource::<MapPlacementRuntimeState>()
            .retired_trees
            .is_empty()
    );
}
