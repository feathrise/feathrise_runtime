//! 正式批次的真实采用与分帧退役只消费自身所有权，独立作者预览保留画面和碰撞。

use super::*;
use avian3d::prelude::{Collider, ColliderDisabled, RigidBody, RigidBodyDisabled};
use bevy::ecs::system::RunSystemOnce;

/// 沿用父夹具已经完成依赖就绪的边界，第二份保存候选仍经过生产最终复验和原子采用。
fn install_followup_candidate(app: &mut App, map: &MapId, record: &MapPlacementRecord) -> (Entity, Entity) {
    let gate = PlacementInstanceGate::new(false);
    let container = app.world_mut().spawn((
        PlacementStagingRoot, Visibility::Hidden, Transform::IDENTITY,
        RigidBody::Static, RigidBodyDisabled, bevy_tnua::TnuaNotPlatform,
    )).id();
    let root = app.world_mut().spawn((
        MapPlacementInstance::new_pending(map, record, gate.clone()),
        PlacementPendingInstance,
        PlacementPendingRevision { generation: 5, revision: 10 },
        Transform::IDENTITY,
        Visibility::Hidden, // 作者域遮蔽的是单个正式根；整批容器激活不能解除这层覆盖。
        ChildOf(container),
    )).id();
    let mut state = app.world_mut().resource_mut::<MapPlacementRuntimeState>();
    assert!(state.staging.is_none() && state.retired_plan.is_none());
    state.staging = Some(PlacementStaging {
        reuse: Default::default(),
        terrain_roots: Vec::new(),
        root: container,
        roots: vec![root],
        gate,
        generation: 5,
        revision: 10,
        building_preset_revision: TEST_BUILDING_PRESET_REVISION,
        building_clearances: BuildingVegetationClearanceState::empty_regions(),
        building_clearance_floor_heights: Default::default(),
        next_plan: 1,
        ready_roots: [root].into_iter().collect(),
        blocked_scene_assets: Default::default(),
        total_instances: 1,
        container_observed: true,
        eligibility_epoch: 1,
        verification_epoch: 1,
        verification_cursor: 0,
    });
    state.pending = Some(PendingPlacementRevision {
        terrain_holes: Default::default(),
        terrain_patch_count: 0,
        generation: 5,
        revision: 10,
        map_id: map.clone(),
        building_preset_revision: TEST_BUILDING_PRESET_REVISION,
        building_clearances: BuildingVegetationClearanceState::empty_regions(),
        building_clearance_floor_heights: Default::default(),
        plans: Vec::new(),
    });
    (container, root)
}

#[test]
fn formal_candidate_replacement_preserves_author_preview_and_per_root_visibility_mask() {
    let (mut app, old_roots) = candidate_app(1, true);
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_systems(Update, (finish_once, cleanup_retired_placement_plan.after(finish_once)));
    let map = MapId::parse("MAP-pending-adoption").unwrap();
    let mut saved = placement_record("candidate-0");
    saved.asset_path.clear();
    saved.asset_target = MapPlacementAssetTarget::Cuboid {
        size_mm: [1_000; 3], color_srgb: [160; 3], roughness_milli: 700, collision: true,
    };
    let (old_container, old_gate) = {
        let state = app.world().resource::<MapPlacementRuntimeState>();
        let staging = state.staging.as_ref().unwrap();
        (staging.root, staging.gate.clone())
    };
    app.world_mut().entity_mut(old_container).insert((
        Transform::IDENTITY, RigidBody::Static, RigidBodyDisabled, bevy_tnua::TnuaNotPlatform,
    ));
    app.world_mut().entity_mut(old_roots[0]).insert((
        MapPlacementInstance::new_pending(&map, &saved, old_gate),
        Transform::IDENTITY, Visibility::Inherited,
    ));
    // 宽旧树确保正式回收确实跨帧执行，不能只证明新批次Ready时还没触碰作者根。
    for _ in 0..=PLACEMENT_TREE_CLEANUP_BUDGET_PER_FRAME {
        app.world_mut().spawn((Visibility::Inherited, ChildOf(old_roots[0])));
    }
    app.update();
    let retired_identity = app.world().get::<MapPlacementInstance>(old_roots[0]).unwrap().clone();
    assert!(retired_identity.is_active());
    assert_eq!(app.world().resource::<MapPlacementRuntimeStatus>().revision, 1);
    assert!(app.world().resource::<MapPlacementRuntimeState>().retired_plan.is_none());

    let mut later_edit = saved.clone();
    later_edit.translation = [9.0, 4.0, 2.0];
    let author_transform = Transform::from_translation(Vec3::from_array(later_edit.translation));
    let author = app.world_mut().spawn((
        MapPlacementInstance::new(&map, &later_edit), author_transform,
        Visibility::Inherited, RigidBody::Static,
    )).id();
    let target = later_edit.asset_target;
    app.world_mut().run_system_once(move |mut commands: Commands| {
        assert!(crate::shared::placement_data::spawn_builtin_placement_visual(&mut commands, author, target));
    }).unwrap();
    let author_visual = app.world().get::<Children>(author).unwrap()[0];
    assert!(app.world().get::<Collider>(author_visual).is_some());
    let (new_container, new_root) = install_followup_candidate(&mut app, &map, &saved);
    assert!(app.world().get::<MapPlacementInstance>(new_root).unwrap().is_pending());

    app.update(); // 真正采用第二批：旧共享门退役、旧容器静默、新共享门激活。

    assert!(!retired_identity.is_active() && !retired_identity.is_pending());
    assert_eq!(app.world().get::<Visibility>(old_container), Some(&Visibility::Hidden));
    assert!(app.world().get::<RigidBodyDisabled>(old_container).is_some());
    assert!(app.world().get::<bevy_tnua::TnuaNotPlatform>(old_container).is_some());
    assert_eq!(app.world().get::<Visibility>(new_container), Some(&Visibility::Inherited));
    assert!(app.world().get::<RigidBodyDisabled>(new_container).is_none());
    assert!(app.world().get::<bevy_tnua::TnuaNotPlatform>(new_container).is_none());
    assert!(app.world().get::<MapPlacementInstance>(new_root).unwrap().is_active());
    assert_eq!(app.world().get::<Visibility>(new_root), Some(&Visibility::Hidden));
    assert_eq!(app.world().resource::<MapPlacementRuntimeStatus>().revision, 2);
    assert_eq!(app.world().resource::<MapPlacementRuntimeState>().active.as_ref().unwrap().root, new_container);
    assert!(!app.world().resource::<MapPlacementRuntimeState>().retired_trees.is_empty());

    app.add_systems(Update, cleanup_retired_placement_trees.after(finish_once));
    for frame in 0..8 {
        app.update();
        assert_eq!(app.world().get::<Transform>(author), Some(&author_transform));
        assert_eq!(app.world().get::<Visibility>(author), Some(&Visibility::Inherited));
        assert!(app.world().get::<MapPlacementInstance>(author).unwrap().is_active());
        assert!(app.world().get::<RigidBody>(author).is_some());
        assert!(app.world().get::<RigidBodyDisabled>(author).is_none());
        assert!(app.world().get::<bevy_tnua::TnuaNotPlatform>(author).is_none());
        assert!(app.world().get::<Collider>(author_visual).is_some());
        assert!(app.world().get::<ColliderDisabled>(author_visual).is_none());
        assert_eq!(app.world().get::<ChildOf>(author_visual).map(ChildOf::parent), Some(author));
        assert!(app.world().get::<ChildOf>(author).is_none());
        assert_eq!(app.world().get::<Visibility>(new_root), Some(&Visibility::Hidden));
        if frame == 0 {
            assert!(app.world().get_entity(old_container).is_ok(), "旧宽树应继续占用分帧清理槽");
        }
        if app.world().resource::<MapPlacementRuntimeState>().retired_trees.is_empty() { break; }
    }
    assert!(app.world().get_entity(old_container).is_err());
    assert!(app.world().get_entity(old_roots[0]).is_err());
    assert!(app.world().resource::<MapPlacementRuntimeState>().retired_trees.is_empty());
    assert_eq!(app.world().resource::<MapPlacementRuntimeStatus>().revision, 2);
}
