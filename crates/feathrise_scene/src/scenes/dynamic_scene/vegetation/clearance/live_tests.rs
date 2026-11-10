//! 真实草窗密度交接：连续刷印沿用根实体，建筑净空变化也不能恢复旧缓存草地。
use super::super::resources::{DynamicVegetationBuild, DynamicVegetationGroundSource};
use super::*;
use feathrise_scene_content::{
    ActiveDynamicScene, SceneId, SceneVegetationAsset, ResolvedVegetationPresetCatalog,
    VegetationPresetCatalogAsset,
};
use std::time::{Duration, Instant};

fn snapshot(value: u8) -> super::super::DynamicBladeDraftSnapshot {
    let map = SceneId::parse("MAP-density-live").unwrap();
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(
        crate::test_fixtures::VEGETATION_CATALOG_JSON,
    )
    .unwrap();
    let catalog = ResolvedVegetationPresetCatalog::build(&catalog).unwrap();
    let mut layer = catalog
        .resolve("hn01_ordinary_grass")
        .unwrap()
        .create_scene_layer(&format!("scenes/{map}"));
    layer.resolution = [3; 2];
    super::super::DynamicBladeDraftSnapshot {
        source_revision: 1,
        sources: BTreeMap::from([(layer.id.clone(), Arc::from(vec![value; 9]))]),
        densities: BTreeMap::from([(layer.id.clone(), Handle::default())]),
        nonzero_layers: if value > 0 {
            [layer.id.clone()].into_iter().collect()
        } else {
            default()
        },
        definition: SceneVegetationAsset {
            schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
            scene_id: map,
            layers: vec![layer],
            authoring: None,
        },
    }
}

fn fixture() -> (App, Entity, Vec<Entity>) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Image>>()
        .init_resource::<BuildingVegetationClearanceState>();
    let draft = snapshot(90);
    let layer = &draft.definition.layers[0];
    let id = layer.runtime_layer_id.unwrap();
    let root = app.world_mut().spawn(Visibility::Inherited).id();
    let windows = (0..4)
        .map(|_| {
            app.world_mut()
                .spawn((
                    ChildOf(root),
                    DynamicBladeWindow { snap_cells: 1 },
                    VegetationBladeLayer(feathrise_environment::vegetation::VegetationLayerId(id)),
                    GpuBladeDensityMap::new(Handle::default()),
                ))
                .id()
        })
        .collect();
    app.insert_resource(ActiveDynamicScene {
        scene_id: Some(draft.definition.scene_id.clone()),
    })
    .insert_resource(DynamicVegetationStatus {
        phase: DynamicVegetationPhase::Ready,
        blade_layer_count: 1,
        blade_window_count: 4,
        ..default()
    })
    .insert_resource(DynamicVegetationState {
        scene_id: Some(draft.definition.scene_id.clone()),
        generation: 1,
        revision: 1,
        active_root: Some(root),
        mesh_source: Some(Arc::new(DynamicVegetationBuild {
            generation: 1,
            revision: 1,
            mesh_layers: vec![],
            volume_mesh_layers: Arc::from([]),
            blade_layers: vec![DynamicBladeLayerBuild {
                runtime_layer_id: id,
                resolution: [3; 2],
                density: Arc::from(vec![0; 9]),
            }],
            ground: DynamicVegetationGroundSource::Clipmap,
            world_min: [-1.; 2],
            world_max: [1.; 2],
        })),
        ..default()
    })
    .init_resource::<super::super::DynamicBladeDraft>()
    .add_systems(Update, drive_dynamic_blade_clearance);
    app.world_mut()
        .resource_mut::<super::super::DynamicBladeDraft>()
        .publish(draft);
    (app, root, windows)
}

fn wait_for(app: &mut App, revision: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app
        .world()
        .resource::<DynamicVegetationState>()
        .blade_clearance_applied_revision
        != Some((0, Some(revision)))
    {
        assert!(Instant::now() < deadline, "草窗应采用完整最新密度");
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn density_refresh_keeps_all_four_window_entities_and_uses_latest_author_after_close() {
    let (mut app, root, windows) = fixture();
    wait_for(&mut app, 1);
    for value in [120, 150, 0] {
        let mut next = snapshot(value);
        next.source_revision = app
            .world()
            .resource::<super::super::DynamicBladeDraft>()
            .revision
            + 1;
        app.world_mut()
            .resource_mut::<super::super::DynamicBladeDraft>()
            .publish_density(next);
        let revision = app
            .world()
            .resource::<super::super::DynamicBladeDraft>()
            .revision;
        assert_ne!(
            app.world()
                .resource::<DynamicVegetationStatus>()
                .applied_density_revision,
            Some(revision),
            "只有发布尚未替换真实四窗时，不得借旧Ready认定最新笔划已显示"
        );
        wait_for(&mut app, revision);
        assert_eq!(
            app.world()
                .resource::<DynamicVegetationStatus>()
                .applied_density_revision,
            Some(revision)
        );
        assert_eq!(
            app.world().resource::<DynamicVegetationState>().active_root,
            Some(root)
        );
        assert_eq!(
            app.world().resource::<DynamicVegetationState>().revision,
            1,
            "纯密度不能重建植被根"
        );
        let image_id = app
            .world()
            .resource::<Assets<Image>>()
            .iter()
            .find(|(_, image)| image.data.as_deref() == Some(vec![value; 9].as_slice()))
            .map(|(id, _)| id)
            .unwrap();
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .get_strong_handle(image_id)
            .unwrap();
        for window in &windows {
            assert_eq!(app.world().get::<feathrise_environment::vegetation::blade::GpuBladeSourceRevision>(*window).unwrap().0, revision);
            let binding = app.world().get::<GpuBladeDensityMap>(*window).unwrap();
            assert_eq!(binding, &GpuBladeDensityMap::new(image.clone()));
            assert_eq!(app.world().get::<ChildOf>(*window).unwrap().parent(), root);
        }
    }
}

#[test]
fn old_density_task_cannot_replace_windows_after_root_revision_changes() {
    let (mut app, _, windows) = fixture();
    app.update();
    let task = app
        .world_mut()
        .resource_mut::<DynamicVegetationState>()
        .blade_clearance_task
        .take()
        .unwrap();
    let (release, held) = std::sync::mpsc::channel();
    app.world_mut()
        .resource_mut::<DynamicVegetationState>()
        .blade_clearance_task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let result = task.await;
        held.recv().unwrap(); // 仅测试制造确实迟到的完成值，生产路径不阻塞。
        result
    }));
    app.world_mut()
        .resource_mut::<DynamicVegetationState>()
        .revision += 1;
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app
        .world()
        .resource::<DynamicVegetationState>()
        .blade_clearance_task
        .is_some()
    {
        assert!(Instant::now() < deadline);
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    for window in windows {
        assert_eq!(
            app.world().get::<GpuBladeDensityMap>(window).unwrap(),
            &GpuBladeDensityMap::new(Handle::default())
        );
    }
    assert_eq!(
        app.world()
            .resource::<DynamicVegetationStatus>()
            .applied_density_revision,
        None,
        "旧根的迟到完成值不得发布新根采用回执"
    );
}

#[test]
fn missing_active_windows_cannot_publish_a_density_adoption_receipt() {
    let (mut app, _, windows) = fixture();
    for window in windows {
        app.world_mut().despawn(window);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.world().resource::<DynamicVegetationStatus>().phase != DynamicVegetationPhase::Failed
    {
        assert!(Instant::now() < deadline);
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = app.world().resource::<DynamicVegetationStatus>();
    assert_eq!(status.applied_density_revision, None);
    assert!(
        status.detail.contains("0/4"),
        "完整窗数来自建根回执，不能由同一空查询自证完整"
    );
}
