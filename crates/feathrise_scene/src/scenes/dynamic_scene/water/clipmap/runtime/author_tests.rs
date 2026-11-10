//! 正式水运行时SystemParam回归：当前画布借根作者值，停放对象/容器以及Ready信用严格隔离。
use super::*;
use crate::scenes::dynamic_scene::instances::{
    SubsceneInstance, SubsceneRuntime, TerrainObjectAuthoringTarget, TerrainObjectInstance,
};
use feathrise_scene_content::{SceneInstancePath, SceneTerrainDescriptor, SceneTerrainObject, TerrainId};
use feathrise_scene_core::residency::{InstanceResourceTable, SceneRuntimeScope};

#[derive(Resource, Default)]
struct Observed(Option<u64>);
fn observe(params: ClipmapWaterRuntimeParams, active: Res<ActiveDynamicScene>, mut out: ResMut<Observed>) {
    out.0 = params.draft.for_scene(active.scene_id.as_ref().unwrap()).map(|draft| draft.source_revision);
}
fn snapshot(map: &SceneId, revision: u64) -> crate::scenes::dynamic_scene::water::DynamicClipmapWaterDraftSnapshot {
    crate::scenes::dynamic_scene::water::DynamicClipmapWaterDraftSnapshot {
        source_revision: revision,
        definition: SceneWaterAsset { scene_id: map.clone(), ..default() },
        sources: BTreeMap::new(), masks: BTreeMap::new(),
    }
}
fn object(id: TerrainId) -> TerrainObjectInstance {
    TerrainObjectInstance { definition: Arc::new(SceneTerrainObject {
        id, display_name: "静水作者回归".into(), enabled: true, transform: default(),
        world_min: [0.; 2], world_max: [32.; 2], terrain: SceneTerrainDescriptor::empty(),
        clipmap_stitches: Arc::from([]), mesh_lod: None, voxel: None, surface_path: None,
        vegetation_path: None, water_path: None, puddles: None,
    }) }
}

#[test]
fn water_runtime_param_reads_current_root_and_preserves_parked_siblings_after_real_owner_swap() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
        .init_resource::<Assets<SceneManifestAsset>>().init_resource::<Assets<SceneWaterAsset>>()
        .init_resource::<Assets<SceneWaterMaskAsset>>().init_resource::<Assets<Image>>()
        .init_resource::<Assets<SparseClipmapSurfaceMaterial>>().init_resource::<Assets<ClipmapHeightMaterial>>()
        .init_resource::<Assets<Mesh>>().init_resource::<Assets<ClipmapWaterMaterial>>()
        .init_resource::<Assets<DynamicWaterMaterial>>().init_resource::<WaterGpuUploadQueue>()
        .init_resource::<WaterGpuUploadReceipts>().init_resource::<SparseClipmapRuntime>()
        .init_resource::<DynamicClipmapGroundSurface>().init_resource::<DynamicWaterState>()
        .init_resource::<DynamicWaterStatus>().init_resource::<ClipmapWaterRuntime>()
        .init_resource::<DynamicClipmapWaterDraft>().init_resource::<TerrainObjectAuthoringTarget>()
        .init_resource::<SceneRuntimeScope>().init_resource::<Observed>()
        .insert_resource(feathrise_environment::terrain_surface::TerrainSurfaceFallbackImages {
            zero_weight: default(), white_base_color_array: default(), neutral_arm_array: default(),
            neutral_normal_array: default(), zero_index: default(), neutral_layer_table: default(),
        })
        .add_systems(Update, observe);
    crate::scenes::dynamic_scene::author_sources::install(&mut app);
    let map = SceneId::parse("MAP-water-author-param").unwrap();
    app.insert_resource(ActiveDynamicScene { scene_id: Some(map.clone()) });
    let ids = ["water-a", "water-b"].map(|id| TerrainId::parse(id).unwrap());
    let batch = app.world_mut().spawn_empty().id();
    let entities = ids.clone().map(|id| app.world_mut().spawn((object(id.clone()), SubsceneInstance {
        path: SceneInstancePath::root(map.clone()).terrain(id), source_scene_id: map.clone(), generation: 1, batch,
    })).id());
    app.insert_resource(SubsceneRuntime::test_active(batch, BTreeMap::from([
        (SceneInstancePath::root(map.clone()).terrain(ids[0].clone()), entities[0]),
        (SceneInstancePath::root(map.clone()).terrain(ids[1].clone()), entities[1]),
    ])));
    let mut table = InstanceResourceTable::<DynamicClipmapWaterDraft>::default();
    for (entity, revision) in entities.into_iter().zip([11, 22]) {
        let mut draft = DynamicClipmapWaterDraft::default(); draft.publish(snapshot(&map, revision));
        table.values.insert(entity, draft);
    }
    app.insert_resource(table);
    let select = |app: &mut App, index: usize| {
        let mut target = app.world_mut().resource_mut::<TerrainObjectAuthoringTarget>();
        target.scene_id = Some(map.clone()); target.terrain = Some(ids[index].clone()); drop(target);
        crate::scenes::dynamic_scene::instances::test_apply_authoring_target(app.world_mut());
    };
    let read = |app: &mut App, index: usize| {
        app.world_mut().resource_mut::<SceneRuntimeScope>().instance = Some(entities[index]);
        app.update(); app.world().resource::<Observed>().0
    };
    select(&mut app, 0);
    app.world_mut().resource_mut::<DynamicClipmapWaterDraft>().publish(snapshot(&map, 12));
    assert_eq!(read(&mut app, 0), Some(12), "正式Params必须消费当前对象的根未保存版本");
    assert_eq!(read(&mut app, 1), Some(22), "兄弟对象只读自己的实例值");
    select(&mut app, 1);
    app.world_mut().resource_mut::<DynamicClipmapWaterDraft>().publish(snapshot(&map, 23));
    assert_eq!(read(&mut app, 0), Some(12), "停放A保留刚才的未保存版本");
    assert_eq!(read(&mut app, 1), Some(23));
    app.insert_resource(feathrise_scene_content::SceneInstanceAuthorPreview {
        scene_id: Some(map), terrains: Some(Arc::from([])), ..default()
    });
    app.world_mut().resource_mut::<SceneRuntimeScope>().instance = None;
    app.update();
    assert_eq!(app.world().resource::<Observed>().0, None, "空场景容器不能借用当前水网作者根");
}

#[derive(Resource)]
struct StatusRequest { map: SceneId, ids: [TerrainId; 2] }
#[derive(Resource, Default)]
struct StatusObserved { current: [Option<DynamicWaterPhase>; 2], saved: [Option<Entity>; 2], root: Option<DynamicWaterPhase> }
fn observe_status(request: Res<StatusRequest>, status: crate::TerrainObjectResource<DynamicWaterStatus>,
    mut out: ResMut<StatusObserved>) {
    for index in 0..2 {
        out.current[index] = status.for_object(&request.map, &request.ids[index]).map(|status| status.phase);
        out.saved[index] = status.for_saved_object(&request.map, &request.ids[index]).map(|(entity, _)| entity);
    }
    out.root = status.for_root(&request.map).map(|status| status.phase);
}

#[test]
fn current_water_status_never_borrows_root_or_same_revision_sibling_during_target_handoff() {
    let mut app = App::new();
    let map = SceneId::parse("MAP-water-status-owner").unwrap();
    let ids = ["status-a", "status-b"].map(|id| TerrainId::parse(id).unwrap());
    let batch = app.world_mut().spawn_empty().id();
    let entities = [app.world_mut().spawn_empty().id(), app.world_mut().spawn_empty().id()];
    app.insert_resource(SubsceneRuntime::test_active(batch, BTreeMap::from([
        (SceneInstancePath::root(map.clone()).terrain(ids[0].clone()), entities[0]),
        (SceneInstancePath::root(map.clone()).terrain(ids[1].clone()), entities[1]),
    ])));
    let ready = || DynamicWaterStatus { scene_id: Some(map.clone()), input_draft_revision: Some(1),
        phase: DynamicWaterPhase::Ready, desired_generation: 7, applied_generation: 7, ..default() };
    app.insert_resource(ready());
    app.insert_resource(ActiveDynamicScene { scene_id: Some(map.clone()) });
    app.world_mut().resource_mut::<DynamicWaterStatus>().phase = DynamicWaterPhase::Building;
    app.insert_resource(InstanceResourceTable::<DynamicWaterStatus> { values: BTreeMap::from([
        (entities[0], ready()), (entities[1], ready()),
    ]) });
    app.insert_resource(TerrainObjectAuthoringTarget::test_applied(map.clone(), ids[0].clone(), entities[0]));
    app.insert_resource(StatusRequest { map, ids: ids.clone() });
    app.init_resource::<StatusObserved>().add_systems(Update, observe_status);
    app.update();
    assert_eq!(app.world().resource::<StatusObserved>().current, [Some(DynamicWaterPhase::Ready), None]);
    assert_eq!(app.world().resource::<StatusObserved>().root, Some(DynamicWaterPhase::Building),
        "独立根静水不能因当前选中了Ready的private画布而借用其保存信用");
    app.world_mut().resource_mut::<TerrainObjectAuthoringTarget>().terrain = Some(ids[1].clone());
    app.update();
    assert_eq!(app.world().resource::<StatusObserved>().current, [None, None],
        "交接帧即使两对象map/数字修订完全相同也不能借Ready");
    assert_eq!(app.world().resource::<StatusObserved>().saved, entities.map(Some),
        "保存仍可等待停放对象，不能依赖检查器选择");
    crate::scenes::dynamic_scene::instances::test_apply_authoring_target(app.world_mut());
    app.update();
    assert_eq!(app.world().resource::<StatusObserved>().current, [None, Some(DynamicWaterPhase::Ready)]);
}
