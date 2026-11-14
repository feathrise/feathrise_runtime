//! 正式模型植被生成合同回归：无碰撞模型、拓扑失效、后台背压与同源实例变换隔离。
use super::resources::*;
use bevy::prelude::*;
use feathrise_scene_content::{
    ModelSurfaceVegetation, SceneId, SceneInstanceId, SceneInstancePath, ScenePlacementAssetTarget,
    SceneVegetationAsset, VegetationPresetCatalogAsset,
};
use std::{path::Path, sync::Arc};

fn identity() -> Identity {
    Identity {
        scene: SceneId("MAP-models".into()),
        path: SceneInstancePath::root(SceneId("MAP-models".into())),
        instance_generation: 1,
        document: 1,
        draft: Some(1),
        document_epoch: 0,
        geometry_epoch: 0,
        placement_revision: 1,
        index_revision: 1,
        presets: 1,
    }
}
#[test]
fn full_address_document_and_geometry_versions_refuse_late_results() {
    let source = identity();
    let mut variants = Vec::new();
    let mut other = source.clone();
    other.path = other.path.child(SceneInstanceId("sibling".into()));
    variants.push(other);
    let mut other = source.clone();
    other.document += 1;
    variants.push(other);
    let mut other = source.clone();
    other.geometry_epoch += 1;
    variants.push(other);
    let mut other = source.clone();
    other.instance_generation += 1;
    variants.push(other);
    let mut other = source.clone();
    other.index_revision += 1;
    variants.push(other);
    for late in variants {
        assert_ne!(source, late);
    }
}

#[test]
fn real_work_permits_remain_occupied_until_cpu_work_finishes() {
    let budget = ModelTaskBudget::default();
    let a = budget.acquire().unwrap();
    let b = budget.acquire().unwrap();
    assert!(budget.acquire().is_none());
    drop(a);
    assert!(budget.acquire().is_some());
    drop(b);
    assert!(budget.acquire().is_some());
}

#[test]
fn model_preset_capture_consumes_only_the_fixed_prefix_per_frame() {
    let (definition, _, presets) = model_input();
    let catalog = VegetationPresetCatalogAsset { schema_version: feathrise_scene_content::VEGETATION_PRESET_SCHEMA_VERSION,
        presets };
    let registry = feathrise_scene_content::VegetationPresetRegistry {
        catalog: Handle::default(), revision: 1,
        resolved: feathrise_scene_content::ResolvedVegetationPresetCatalog::build(&catalog).unwrap(), last_error: None,
    };
    let mut document = (*definition).clone();
    document.layers = (0..70).map(|index| {
        let mut layer = definition.layers[0].clone(); layer.id = format!("layer-{index}"); layer
    }).collect();
    let mut capture = Capture { identity: identity(), definition: Arc::new(document), cursor: 0,
        objects: Vec::new(), warnings: Vec::new(), presets: Vec::new() };
    assert!(!super::runtime::capture_presets(&mut capture, &registry, 32).unwrap());
    assert_eq!(capture.presets.len(), 32);
    assert!(!super::runtime::capture_presets(&mut capture, &registry, 32).unwrap());
    assert_eq!(capture.presets.len(), 64);
    assert!(super::runtime::capture_presets(&mut capture, &registry, 32).unwrap());
    assert_eq!(capture.presets.len(), 70, "尾段必须完整捕获，不能把预算作为内容截断");
}

#[test]
fn model_build_returns_only_used_profile_dependencies_and_background_bank_slots() {
    let (definition, input, mut presets) = model_input();
    let used = presets[0].id.clone();
    let mut unused = presets[0].clone();
    unused.id = "unused-profile".into(); unused.output_id = "unused-output".into();
    let mut document = (*definition).clone();
    document.layers.push(unused.create_scene_layer("unused"));
    presets.push(unused);
    let built = super::build::build(Path::new("unused"), Arc::new(document), vec![input], presets, vec![]).unwrap();
    assert_eq!(built.profiles.len(), 1, "未绘制的输出不能增加主线程资源轮询");
    assert_eq!(built.profiles[0].preset.id, used);
    let (bank, slot) = built.profiles[0].binding.unwrap();
    assert_eq!((bank.index(), slot.index()), (1, 0));
    assert_eq!(built.banks.len(), 1);
}

#[test]
fn empty_model_canvas_adopts_without_loading_or_resolving_unrelated_legacy_2d_profiles() {
    use super::super::super::mesh_profiles::{
        DynamicMeshProfileCache, DynamicMeshProfileRequests, DynamicMeshProfileState,
    };
    use feathrise_environment::vegetation::{
        VegetationMeshPrototypeRegistry, blade::GpuBladeProfileBanks,
    };
    use feathrise_scene_content::{ActiveDynamicScene, RuntimeAssetRoot, SceneContentRegistry};
    let (definition, _, _) = model_input();
    let mut document = (*definition).clone();
    document.model_surfaces = Arc::new(Vec::new());
    document.layers[0].runtime_profile = Some("legacy_2d_profile_without_a_model_binding".into());
    let document = Arc::new(document);
    let scene = document.scene_id.clone();
    let mut draft = super::super::DynamicModelVegetationDraft::default();
    draft.publish(scene.clone(), 7, document.clone());
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<SceneVegetationAsset>>()
        .init_resource::<SceneContentRegistry>()
        .init_resource::<ModelVegetationState>()
        .init_resource::<DynamicModelVegetationStatus>()
        .init_resource::<ModelAssetEpoch>()
        .init_resource::<ModelTaskBudget>()
        .init_resource::<super::index::ModelSourceIndex>()
        .init_resource::<crate::ScenePlacementRuntimeStatus>()
        .init_resource::<RuntimeAssetRoot>()
        .init_resource::<VegetationMeshPrototypeRegistry>()
        .init_resource::<GpuBladeProfileBanks>()
        .init_resource::<DynamicMeshProfileState>()
        .init_resource::<DynamicMeshProfileRequests>()
        .init_resource::<DynamicMeshProfileCache>()
        .insert_resource(ActiveDynamicScene {
            scene_id: Some(scene),
        })
        .insert_resource(draft)
        .add_systems(Update, super::runtime::drive);
    // 故意不安装VegetationPresetRegistry；空模型文档仍走正式drive并获得完整Ready回执。
    app.update();
    let status = app.world().resource::<DynamicModelVegetationStatus>();
    assert!(status.ready && !status.failed);
    assert_eq!(status.applied_revision, Some(7));
    assert_eq!(
        status.applied_document,
        Some(Arc::as_ptr(&document) as usize)
    );
    assert_eq!(status.blade_window_count, 0);
    assert_eq!(
        app.world()
            .resource::<DynamicMeshProfileRequests>()
            .requested_count(),
        0
    );
}

fn model_input() -> (
    Arc<SceneVegetationAsset>,
    ObjectInput,
    Vec<feathrise_scene_content::VegetationPresetDescriptor>,
) {
    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let target = ScenePlacementAssetTarget::Cuboid {
        size_mm: [2000, 200, 2000],
        color_srgb: [100; 3],
        roughness_milli: 700,
        collision: false,
    };
    let geometry =
        crate::paint_surface::load_model_surface_source(Path::new("unused"), "", target).unwrap();
    let hit = geometry
        .ray_surface(Mat4::IDENTITY, Vec3::new(0., 4., 0.), Vec3::NEG_Y, 20.)
        .unwrap();
    let mut density = ModelSurfaceVegetation::empty("bridge", geometry.topology(), 0.5);
    geometry
        .paint_vegetation(
            &mut density,
            Mat4::IDENTITY,
            &hit,
            "grass",
            0.8,
            1.,
            1.,
            false,
        )
        .unwrap();
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/vegetation_presets.json"
    )))
    .unwrap();
    let preset = catalog.presets.into_iter().next().unwrap();
    let layer=serde_json::from_value(serde_json::json!({"id":"grass","label":"草","kind":"blade",
        "runtime_profile":preset.id,"asset_path":"gltf/fixture_vegetation.gltf","density_source_path":"test.r8",
        "density_texture_path":"test.ktx2","resolution":[8,8],"spacing_meters":0.5,
        "scale_range":[1,1],"max_instances":1024,"seed":17})).unwrap();
    let definition = Arc::new(SceneVegetationAsset {
        scene_id: SceneId("MAP-models".into()),
        layers: vec![layer],
        model_surfaces: Arc::new(vec![Arc::new(density.clone())]),
        ..default()
    });
    (
        definition,
        ObjectInput {
            entity,
            id: "bridge".into(),
            path: String::new(),
            target,
            density: Arc::new(density),
        },
        vec![preset],
    )
}

#[test]
fn collision_disabled_model_uses_formal_gpu_samples_and_preserves_its_painted_side() {
    let (definition, input, presets) = model_input();
    let built = super::build::build(
        Path::new("unused"),
        definition,
        vec![input],
        presets,
        vec![],
    )
    .unwrap();
    assert!(built.warnings.is_empty());
    let layer = &built.objects[0].layers[0];
    assert!(layer.image.is_some());
    assert!(!layer.cells.is_empty());
    assert!(
        layer
            .cells
            .iter()
            .all(|cell| (cell[0][1] - 0.1).abs() < 0.00001 && cell[1][1] > 0.99),
        "桥面样本不能绕过连通边界出现在桥底"
    );
}

#[test]
fn two_centimeter_wall_sides_keep_exact_local_roots_and_normals_in_the_gpu_upload() {
    let (definition, mut input, presets) = model_input();
    input.target = ScenePlacementAssetTarget::Cuboid {
        size_mm: [20, 4000, 4000],
        color_srgb: [100; 3],
        roughness_milli: 700,
        collision: false,
    };
    let geometry = crate::paint_surface::load_model_surface_source(
        Path::new("unused"),
        "",
        input.target,
    )
    .unwrap();
    let mut density = ModelSurfaceVegetation::empty("bridge", geometry.topology(), 0.5);
    for direction in [Vec3::X, Vec3::NEG_X] {
        let hit = geometry
            .ray_surface(Mat4::IDENTITY, direction * 2., -direction, 3.)
            .unwrap();
        assert!(hit.normal.dot(direction) > 0.99);
        geometry
            .paint_vegetation(&mut density, Mat4::IDENTITY, &hit, "grass", 0.8, 1., 0., false)
            .unwrap();
    }
    let density = Arc::new(density);
    let mut document = (*definition).clone();
    document.model_surfaces = Arc::new(vec![density.clone()]);
    input.density = density;
    let built = super::build::build(
        Path::new("unused"),
        Arc::new(document),
        vec![input],
        presets,
        vec![],
    )
    .unwrap();
    let layer = &built.objects[0].layers[0];
    assert!(layer.cells.iter().any(|cell| cell[0][0] < 0.));
    assert!(layer.cells.iter().any(|cell| cell[0][0] > 0.));
    let upload = layer.image.as_ref().unwrap().data.as_ref().unwrap();
    for (index, cell) in layer.cells.iter().enumerate() {
        assert!((cell[0][0].abs() - 0.01).abs() < 0.000001);
        assert!(cell[0][0] * cell[1][0] > 0., "薄壁两面不能互借根点或法线");
        // 实际上传的三个RGBA记录必须逐字保留精确根点，不允许在量化或编码中并到中间平面。
        for (component, expected) in cell.iter().flatten().enumerate() {
            let start = index * 48 + component * 4;
            let actual = f32::from_le_bytes(upload[start..start + 4].try_into().unwrap());
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }
}

#[test]
fn topology_change_preserves_author_data_and_reports_rebinding_instead_of_reusing_old_triangles() {
    let (definition, mut input, presets) = model_input();
    let original = input.density.clone();
    let mut stale = (*original).clone();
    stale.topology[0] ^= 1;
    input.density = Arc::new(stale);
    let built = super::build::build(
        Path::new("unused"),
        definition,
        vec![input],
        presets,
        vec![],
    )
    .unwrap();
    assert!(built.objects.is_empty());
    assert!(built.warnings[0].contains("重新绑定"));
    assert!(!original.samples.is_empty());
}

#[test]
fn source_hierarchy_moves_gpu_vegetation_and_each_instance_uses_its_own_focus() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin))
        .add_systems(
            PostUpdate,
            super::follow::follow.before(bevy::transform::TransformSystems::Propagate),
        );
    app.world_mut().spawn((
        crate::SceneRuntimeObserver,
        Transform::from_xyz(20., 4., 7.),
    ));
    app.world_mut().spawn((
        feathrise_environment::vegetation::blade::GpuBladeView::default(),
        Transform::from_xyz(23., 20., 9.),
    ));
    let mut records = Vec::new();
    for pose in [
        Transform::from_xyz(4., 1., 2.),
        Transform::from_xyz(-30., 8., 10.)
            .with_rotation(Quat::from_rotation_y(1.3))
            .with_scale(Vec3::new(2., 3., 4.)),
    ] {
        let source = app.world_mut().spawn(pose).id();
        let window = app
            .world_mut()
            .spawn((
                ModelWindow { source },
                Transform::from_xyz(0., 0.1, 0.),
                feathrise_environment::vegetation::blade::GpuBladePatch::square(
                    Vec3::ZERO,
                    16.,
                    17,
                    1,
                ),
                ChildOf(source),
            ))
            .id();
        records.push((source, window));
    }
    for offset in [0., 30.] {
        for (source, _) in &records {
            app.world_mut()
                .get_mut::<Transform>(*source)
                .unwrap()
                .translation
                .x += offset;
        }
        app.update();
        for (source, window) in &records {
            let world = app.world();
            let frame = world.get::<GlobalTransform>(*source).unwrap().to_matrix();
            let inverse = frame.inverse();
            let eye = inverse.transform_point3(Vec3::new(23., 20., 9.));
            let player = inverse.transform_point3(Vec3::new(20., 4., 7.));
            assert!(
                world
                    .get::<feathrise_environment::vegetation::blade::GpuBladePatch>(*window)
                    .unwrap()
                    .focus_radius_xyz()
                    .abs_diff_eq(Vec3::new(eye.x, player.y, eye.z), 0.0001)
            );
            assert!(
                world
                    .get::<GlobalTransform>(*window)
                    .unwrap()
                    .translation()
                    .abs_diff_eq(frame.transform_point3(Vec3::new(0., 0.1, 0.)), 0.0001)
            );
        }
    }
}
