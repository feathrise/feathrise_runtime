//! 真实协调器resource_scope内创建独立地形，换代只按稳定路径承接作者预览。

use super::*;
use bevy::asset::AssetPlugin;
use feathrise_scene_content::{
    SCENE_SCHEMA_VERSION, SceneContentHandles, SceneContentRegistry, SceneId, SceneInstancePath,
    SceneInstanceSource, SceneManifestAsset, ScenePuddleLiveOverride, ScenePuddleSaveHandoff,
    SceneTerrainDescriptor, SceneTerrainKind, SceneTerrainObject, TerrainId, expand_scene_instances,
};
use feathrise_scene_core::residency::SceneInstanceContextAppExt;

fn placement_revision_app() -> App {
    let mut app = App::new();
    app.add_message::<AssetEvent<feathrise_scene_content::ScenePlacementsAsset>>()
        .add_message::<crate::ScenePlacementRebuildRequest>()
        .init_resource::<InstanceAssetJournalEpoch>()
        .init_resource::<feathrise_scene_content::SceneAuthorPreview>()
        .insert_resource(feathrise_scene_content::BuildingPresetRegistry {
            catalog: Handle::default(), revision: 1, resolved: Default::default(), last_error: None,
        })
        .add_systems(Update, invalidate_submaps_for_placement_changes);
    app.update(); // 首次读取建立输入基准，不把默认资源当成热更新。
    app
}

#[test]
fn placement_and_building_changes_advance_outer_epoch_once_per_frame_and_not_when_stable() {
    let mut app = placement_revision_app();
    let id = Handle::<feathrise_scene_content::ScenePlacementsAsset>::default().id();
    app.world_mut().write_message(AssetEvent::Modified { id });
    app.world_mut().write_message(AssetEvent::Removed { id });
    app.world_mut().write_message(crate::ScenePlacementRebuildRequest { scene_id: SceneId::parse("MAP-revision").unwrap() });
    app.world_mut().resource_mut::<feathrise_scene_content::BuildingPresetRegistry>().revision = 2;
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1);
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1);
    app.world_mut().resource_mut::<feathrise_scene_content::BuildingPresetRegistry>().revision = 3;
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 2);
    app.world_mut().resource_mut::<feathrise_scene_content::SceneAuthorPreview>().active = Some(feathrise_scene_content::SceneAuthorPreviewHandles {
        scene_id: SceneId::parse("MAP-revision").unwrap(), placements: Handle::default(), portals: Handle::default(), boundaries: None,
    });
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 3);
}

#[test]
fn placement_revision_notice_overflow_still_invalidates_one_complete_outer_batch() {
    let mut app = placement_revision_app();
    let id = Handle::<feathrise_scene_content::ScenePlacementsAsset>::default().id();
    for _ in 0..260 { app.world_mut().write_message(AssetEvent::Modified { id }); }
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1);
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1);
}

#[test]
fn placement_epoch_retires_an_in_progress_outer_candidate_and_preserves_the_old_active_batch() {
    let (mut app, map) = fixture();
    create_candidate(&mut app, &map, Arc::from([object("epoch-candidate")]), 2);
    let (active, candidate) = {
        let runtime = app.world().resource::<SubsceneRuntime>();
        (runtime.active.as_ref().unwrap().root, runtime.candidate.as_ref().unwrap().root)
    };
    app.world_mut().entity_mut(active).insert(Visibility::Inherited).remove::<(RigidBodyDisabled, TnuaNotPlatform)>();
    app.world_mut().resource_scope(|world, mut runtime: Mut<SubsceneRuntime>| {
        runtime.journal_epoch = 3;
        retire_stale_candidate(world, &mut runtime, 2, 4); // 文档换代而场景图结构修订未变，也必须拒绝旧候选。
    });
    let runtime = app.world().resource::<SubsceneRuntime>();
    assert_eq!(runtime.active.as_ref().unwrap().root, active);
    assert!(runtime.candidate.is_none());
    assert_eq!(runtime.retired.front().unwrap().root, candidate);
    assert_eq!(app.world().get::<Visibility>(active), Some(&Visibility::Inherited));
    assert!(app.world().get::<RigidBodyDisabled>(active).is_none());
    assert_eq!(app.world().get::<Visibility>(candidate), Some(&Visibility::Hidden));
    assert!(app.world().get::<RigidBodyDisabled>(candidate).is_some());
}
mod asset_feedback_tests;

#[test]
fn startup_progress_reports_completed_instances_without_overwriting_later_stages() {
    use feathrise_scene_core::state::{LoadingProgress, LoadingStage, SceneStartupGate};
    let mut progress = LoadingProgress::default();
    progress.stage = LoadingStage::SceneInit;
    let mut app = App::new();
    app.insert_resource(State::new(ScenePhase::Active))
        .insert_resource(SceneStartupGate::waiting_for_collision(ScenePhase::Active))
        .insert_resource(SubsceneRuntimeStatus { total: 2, prepared: 1,
            loading_detail: Some("远处网格：正在验证GPU管线".into()), ..default() })
        .insert_resource(progress)
        .add_systems(Update, publish_startup);
    app.update();
    let progress = app.world().resource::<LoadingProgress>();
    assert_eq!(progress.percent(), 74.0);
    assert!(progress.label.contains("1/2") && progress.label.contains("远处网格"));
    app.world_mut().resource_mut::<LoadingProgress>().stage = LoadingStage::DynamicEntities;
    app.update();
    assert_eq!(app.world().resource::<LoadingProgress>().stage, LoadingStage::DynamicEntities);
    app.world_mut().resource_mut::<SubsceneRuntimeStatus>().last_error = Some("远处网格：GPU预热超过30秒".into());
    app.update();
    let progress = app.world().resource::<LoadingProgress>();
    assert_eq!(progress.stage, LoadingStage::Failed);
    assert!(progress.label.contains("GPU预热超过30秒"));
}

fn object(id: &str) -> SceneTerrainObject {
    SceneTerrainObject {clipmap_stitches: Default::default(),
        mesh_lod: None,
        id: TerrainId::parse(id).unwrap(),
        display_name: format!("独立地形{id}"),
        enabled: true,
        transform: Default::default(),
        world_min: [-4.; 2],
        world_max: [4.; 2],
        terrain: SceneTerrainDescriptor {resolved_stitches: Default::default(),
            kind: SceneTerrainKind::SparseClipmap,
            resolution: [33; 2],
            height_path: Some(format!(
                "scenes/MAP-context-scope/terrains/{id}/height_tiles.scene_height_manifest.json"
            )),
            ..SceneTerrainDescriptor::empty()
        },
        voxel: None,
        surface_path: None,
        vegetation_path: None,
        water_path: None,
        puddles: None,
    }
}

fn graph(
    map: &SceneId,
    objects: Arc<[SceneTerrainObject]>,
    revision: u64,
) -> Arc<[ExpandedSceneInstance]> {
    let source = Arc::new(SceneInstanceSource {
        scene_id: map.clone(),
        revision,
        world_bounds: Rect::from_corners(Vec2::splat(-8.), Vec2::splat(8.)),
        instances: Arc::from([]),
        terrains: Some(objects),
    });
    expand_scene_instances(map, &BTreeMap::from([(map.clone(), source)]))
        .unwrap()
        .into()
}

fn fixture() -> (App, SceneId) {
    let map = SceneId::parse("MAP-context-scope").unwrap();
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<SceneManifestAsset>()
        .init_resource::<SceneContentRegistry>()
        .init_resource::<SubsceneRuntime>()
        .init_resource::<super::super::resources::TerrainInstanceLookup>()
        .init_resource::<super::super::TerrainObjectAuthoringTarget>()
        .insert_resource(State::new(ScenePhase::Active))
        .register_instance_resource::<ActiveDynamicScene>()
        .register_instance_resource::<State<ScenePhase>>()
        .register_instance_resource::<SceneContentRegistry>()
        .register_instance_resource::<super::super::super::resources::DynamicSceneRuntimeState>()
        .init_instance_resource::<ScenePuddleLiveOverride>()
        .init_instance_resource::<ScenePuddleSaveHandoff>();
    let manifest = app
        .world_mut()
        .resource_mut::<Assets<SceneManifestAsset>>()
        .add(SceneManifestAsset {
            vegetation_paint_groups: Default::default(),
            schema_version: SCENE_SCHEMA_VERSION,
            id: map.clone(),
            display_name: "空场景实例scope回归".into(),
            notes: String::new(),
            runtime_scene: "data_driven".into(),
            world_min: [-8.; 2],
            world_max: [8.; 2],
            default_spawn: [0.; 3],
            environment: Default::default(),
            terrain: SceneTerrainDescriptor::empty(),
            terrains: Some(Arc::from([])),
            subscenes: Default::default(),
            puddles: None,
            asset_roots: Vec::new(),
        });
    app.world_mut()
        .resource_mut::<SceneContentRegistry>()
        .scenes
        .insert(
            map.clone(),
            SceneContentHandles {
                characters_path: None,
                manifest,
                portals: Handle::default(),
                placements: Handle::default(),
                surface: None,
                vegetation: None,
                water: None,
                boundaries: None,
            },
        );
    let empty = graph(&map, Arc::from([]), 1);
    app.world_mut()
        .resource_scope(|world, mut runtime: Mut<SubsceneRuntime>| {
            assert!(!world.contains_resource::<SubsceneRuntime>());
            runtime.root_scene_id = Some(map.clone());
            runtime.active = Some(begin_batch(world, &mut runtime, 1, empty));
        });
    (app, map)
}

fn create_candidate(app: &mut App, map: &SceneId, objects: Arc<[SceneTerrainObject]>, revision: u64) {
    let manifest = app
        .world()
        .resource::<SceneContentRegistry>()
        .handles(map)
        .unwrap()
        .manifest
        .clone();
    app.world_mut()
        .resource_mut::<Assets<SceneManifestAsset>>()
        .get_mut(&manifest)
        .unwrap()
        .terrains = Some(objects.clone());
    let graph = graph(map, objects, revision);
    app.world_mut()
        .resource_scope(|world, mut runtime: Mut<SubsceneRuntime>| {
            // 驱动器真实调用期间协调器不在World中；回读它会复现本次Missing resource崩溃。
            assert!(!world.contains_resource::<SubsceneRuntime>());
            let mut candidate = begin_batch(world, &mut runtime, revision, graph);
            let generation = runtime.generation;
            create_contexts(world, generation, &mut candidate, runtime.active.as_ref());
            assert_eq!(candidate.next, candidate.graph.len());
            runtime.candidate = Some(candidate);
        });
    assert_eq!(
        app.world().resource::<SubsceneRuntime>().root_scene_id.as_ref(),
        Some(map)
    );
}

#[test]
fn empty_scene_adds_independent_terrain_inside_the_real_coordinator_resource_scope() {
    let (mut app, map) = fixture();
    let terrain = object("new-ground");
    let path = SceneInstancePath::root(map.clone()).terrain(terrain.id.clone());
    // 来源句柄全在Assets内存中，创建上下文不运行加载调度，不读取或改写用户场景文件。
    create_candidate(&mut app, &map, Arc::from([terrain.clone()]), 2);
    let world = app.world();
    let runtime = world.resource::<SubsceneRuntime>();
    assert_eq!(runtime.generation, 2);
    assert!(runtime.active.as_ref().unwrap().index.is_empty());
    let candidate = runtime.candidate.as_ref().unwrap();
    assert_eq!(candidate.index.len(), 1);
    assert_eq!(candidate.contexts.len(), 1);
    let entity = candidate.index[&path];
    assert_eq!(candidate.contexts[0].entity, entity);
    assert_eq!(world.get::<SubsceneInstance>(entity).unwrap().path, path);
    assert_eq!(world.get::<SubsceneInstance>(entity).unwrap().generation, 2);
    assert_eq!(
        world.get::<ChildOf>(entity).unwrap().parent(),
        candidate.root
    );
    assert_eq!(
        *world
            .get::<super::super::TerrainObjectInstance>(entity)
            .unwrap()
            .definition,
        terrain
    );
    assert_eq!(
        world
            .resource::<InstanceResourceTable<ActiveDynamicScene>>()
            .values[&entity]
            .scene_id
            .as_ref(),
        Some(&map)
    );
    assert_eq!(
        *world
            .resource::<InstanceResourceTable<State<ScenePhase>>>()
            .values[&entity]
            .get(),
        ScenePhase::Active
    );
    assert!(world.resource::<InstanceResourceTable<super::super::super::resources::DynamicSceneRuntimeState>>().values[&entity].dirty);
    let source = &world
        .resource::<InstanceResourceTable<SceneContentRegistry>>()
        .values[&entity];
    assert_eq!(source.scenes.len(), 1);
    let source_manifest = world
        .resource::<Assets<SceneManifestAsset>>()
        .get(&source.handles(&map).unwrap().manifest)
        .unwrap();
    assert_eq!(source_manifest.id, map);
    assert_eq!(source_manifest.display_name, terrain.display_name);
    assert_eq!(source_manifest.terrain, terrain.terrain);
    assert!(source_manifest.terrains.is_none());
    assert!(
        world
            .resource::<InstanceResourceTable<ScenePuddleLiveOverride>>()
            .values[&entity]
            .scene_id
            .is_none()
    );
}

#[test]
fn scoped_context_rebuild_inherits_selected_preview_and_same_scene_sibling_by_stable_path() {
    let (mut app, map) = fixture();
    let first = object("ground-a");
    let second = object("ground-b");
    let first_path = SceneInstancePath::root(map.clone()).terrain(first.id.clone());
    let second_path = SceneInstancePath::root(map.clone()).terrain(second.id.clone());
    create_candidate(
        &mut app,
        &map,
        Arc::from([first.clone(), second.clone()]),
        2,
    );
    {
        let mut runtime = app.world_mut().resource_mut::<SubsceneRuntime>();
        runtime.active = runtime.candidate.take();
    }
    let old_first = app
        .world()
        .resource::<SubsceneRuntime>()
        .active_entity(&first_path)
        .unwrap();
    let old_second = app
        .world()
        .resource::<SubsceneRuntime>()
        .active_entity(&second_path)
        .unwrap();
    let first_mask = Arc::new(vec![11; 4]);
    let second_mask = Arc::new(vec![22; 4]);
    for (entity, revision, mask) in [
        (old_first, 11, first_mask),
        (old_second, 22, second_mask.clone()),
    ] {
        app.world_mut()
            .resource_mut::<InstanceResourceTable<ScenePuddleLiveOverride>>()
            .values
            .insert(
                entity,
                ScenePuddleLiveOverride {
                    scene_id: Some(map.clone()),
                    revision,
                    preview: true,
                    ..default()
                },
            );
        app.world_mut()
            .resource_mut::<InstanceResourceTable<ScenePuddleSaveHandoff>>()
            .values
            .insert(
                entity,
                ScenePuddleSaveHandoff {
                    scene_id: Some(map.clone()),
                    revision,
                    expected_mask: Some(mask),
                    ..default()
                },
            );
    }
    {
        let mut target = app
            .world_mut()
            .resource_mut::<super::super::TerrainObjectAuthoringTarget>();
        target.scene_id = Some(map.clone());
        target.terrain = Some(first.id.clone());
    }
    super::super::authoring_target::sync_preview_owner(app.world_mut());
    assert_eq!(
        app.world()
            .resource::<super::super::TerrainObjectAuthoringTarget>()
            .instance(),
        Some(old_first)
    );
    let edited_first = Arc::new(vec![111; 4]);
    app.world_mut()
        .resource_mut::<ScenePuddleLiveOverride>()
        .revision = 111;
    app.world_mut()
        .resource_mut::<ScenePuddleSaveHandoff>()
        .revision = 111;
    app.world_mut()
        .resource_mut::<ScenePuddleSaveHandoff>()
        .expected_mask = Some(edited_first.clone());

    // 同SceneId、同来源类型，交换创建次序；旧数组下标不能代替稳定对象路径。
    create_candidate(&mut app, &map, Arc::from([second, first]), 3);
    let world = app.world();
    let runtime = world.resource::<SubsceneRuntime>();
    assert_eq!(runtime.active_entity(&first_path), Some(old_first));
    assert_eq!(runtime.active_entity(&second_path), Some(old_second));
    let candidate = runtime.candidate.as_ref().unwrap();
    let new_first = candidate.index[&first_path];
    let new_second = candidate.index[&second_path];
    assert_ne!(new_first, old_first);
    assert_ne!(new_second, old_second);
    assert_eq!(candidate.contexts[0].entity, new_second);
    assert_eq!(candidate.contexts[1].entity, new_first);
    let live = world.resource::<InstanceResourceTable<ScenePuddleLiveOverride>>();
    assert_eq!(live.values[&new_first].revision, 111);
    assert_eq!(live.values[&new_first].scene_id.as_ref(), Some(&map));
    assert_eq!(live.values[&new_second].revision, 22);
    assert_eq!(live.values[&new_second].scene_id.as_ref(), Some(&map));
    assert_eq!(live.values[&old_second].revision, 22);
    assert!(live.values[&old_first].scene_id.is_none());
    assert_eq!(world.resource::<ScenePuddleLiveOverride>().revision, 111);
    let handoff = world.resource::<InstanceResourceTable<ScenePuddleSaveHandoff>>();
    assert!(Arc::ptr_eq(
        handoff.values[&new_first].expected_mask.as_ref().unwrap(),
        &edited_first
    ));
    assert!(Arc::ptr_eq(
        handoff.values[&new_second].expected_mask.as_ref().unwrap(),
        &second_mask
    ));
    assert!(Arc::ptr_eq(
        handoff.values[&old_second].expected_mask.as_ref().unwrap(),
        &second_mask
    ));
    assert!(handoff.values[&old_first].expected_mask.is_none());
    assert!(Arc::ptr_eq(
        world
            .resource::<ScenePuddleSaveHandoff>()
            .expected_mask
            .as_ref()
            .unwrap(),
        &edited_first
    ));
}

#[test]
fn global_scene_catalog_survives_cold_scene_creation_and_warm_scene_restore() {
    use feathrise_scene_core::residency::{restore_resident_resources, take_resident_resources};
    let (mut app, old_scene) = fixture();
    // 必须安装正式插件：单独注册测试资源会漏掉把全局目录误登记为场景私有快照的错误。
    app.init_asset::<Image>()
        .init_asset::<bevy::shader::Shader>()
        .add_plugins(super::super::super::DynamicScenePlugin);
    create_candidate(&mut app, &old_scene, Arc::from([object("old-ground")]), 2);
    let old_entity = app.world().resource::<SubsceneRuntime>().candidate.as_ref().unwrap().contexts[0].entity;
    let original_handles = app.world().resource::<SceneContentRegistry>().handles(&old_scene).unwrap().clone();
    app.world_mut().resource_mut::<SceneContentRegistry>().revision = 3;

    // 对应新建后自动进入冷场景：原场景的运行状态暂停，目录仍须服务所有场景。
    let mut parked = take_resident_resources(app.world_mut());
    assert!(app.world().resource::<SceneContentRegistry>().handles(&old_scene).is_some(),
        "暂停场景不能清空全局目录，否则来源树创建上下文会因场景未登记而崩溃");
    assert!(app.world().resource::<InstanceResourceTable<SceneContentRegistry>>().values.is_empty());
    let new_scene = SceneId::parse("MAP-new-after-parking").unwrap();
    let mut manifest = app.world().resource::<Assets<SceneManifestAsset>>().get(&original_handles.manifest).unwrap().clone();
    manifest.id = new_scene.clone();
    let new_manifest = app.world_mut().resource_mut::<Assets<SceneManifestAsset>>().add(manifest);
    let mut new_handles = original_handles.clone();
    new_handles.manifest = new_manifest;
    {
        let mut registry = app.world_mut().resource_mut::<SceneContentRegistry>();
        registry.scenes.insert(new_scene.clone(), new_handles.clone());
        registry.revision = 4;
    }
    app.world_mut().resource_mut::<SubsceneRuntime>().root_scene_id = Some(new_scene.clone());
    create_candidate(&mut app, &new_scene, Arc::from([object("new-ground")]), 3);
    assert_eq!(app.world().resource::<SubsceneRuntime>().candidate.as_ref().unwrap().contexts.len(), 1);

    // 返回旧场景只恢复私有上下文，不能把已经包含新场景的目录回滚为修订3。
    restore_resident_resources(app.world_mut(), &mut parked);
    let registry = app.world().resource::<SceneContentRegistry>();
    assert_eq!(registry.revision, 4);
    assert_eq!(registry.handles(&old_scene).unwrap().manifest, original_handles.manifest);
    assert_eq!(registry.handles(&new_scene).unwrap().manifest, new_handles.manifest);
    let contexts = &app.world().resource::<InstanceResourceTable<SceneContentRegistry>>().values;
    assert_eq!(contexts.len(), 1);
    assert!(contexts[&old_entity].handles(&old_scene).is_some());
    assert!(contexts[&old_entity].handles(&new_scene).is_none());
}
