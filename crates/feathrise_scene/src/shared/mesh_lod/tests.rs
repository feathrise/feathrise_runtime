//! 真实注册/后台/显示系统回归，覆盖原始来源与过期结果；不替代窗口和GPU验收。
use super::*;
use bevy::{
    asset::RenderAssetUsages,
    camera::{ComputedCameraValues, RenderTargetInfo, primitives::Aabb},
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
};
use feathrise_scene_content::{SceneId, ScenePlacementRecord};
use std::time::{Duration, Instant};

pub(super) fn grid() -> Mesh {
    let side = 24usize;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        (0..side * side)
            .map(|index| [(index % side) as f32, 0.0, (index / side) as f32])
            .collect::<Vec<_>>(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; side * side]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        (0..side * side)
            .map(|index| [(index % side) as f32 / 23.0, (index / side) as f32 / 23.0])
            .collect::<Vec<_>>(),
    );
    let mut indices = Vec::new();
    for z in 0..side - 1 {
        for x in 0..side - 1 {
            let a = (z * side + x) as u32;
            let b = a + side as u32;
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

pub(super) fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<feathrise_settings::RenderSettings>();
    install(&mut app);
    app
}

pub(super) fn owner_root(world: &mut World, building: bool) -> Entity {
    let record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"static", "asset_path":"scenes/LOD/static.glb", "translation":[0,0,0], "yaw_degrees":0,"scale":[1,1,1],
        "building_preset_id": if building {Some("house")} else {None},
    })).unwrap();
    world
        .spawn((
            crate::ScenePlacementInstance::new(&SceneId::parse("MAP-LOD").unwrap(), &record),
            avian3d::prelude::RigidBody::Static,
            Transform::default(),
            GlobalTransform::IDENTITY,
        ))
        .id()
}

pub(super) fn visual(world: &mut World, root: Entity, handle: Handle<Mesh>) -> Entity {
    world
        .spawn((
            Mesh3d(handle),
            MeshMaterial3d::<StandardMaterial>(Handle::default()),
            Transform::default(),
            GlobalTransform::IDENTITY,
            ChildOf(root),
            Aabb::from_min_max(Vec3::ZERO, Vec3::new(23.0, 0.0, 23.0)),
        ))
        .id()
}

fn camera(world: &mut World) {
    world.spawn((
        Camera3d::default(),
        Camera {
            computed: ComputedCameraValues {
                target_info: Some(RenderTargetInfo {
                    physical_size: UVec2::new(1920, 1080),
                    scale_factor: 1.0,
                }),
                ..default()
            },
            ..default()
        },
        Projection::Perspective(PerspectiveProjection::default()),
        GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 1000.0)),
    ));
}

pub(super) fn until(app: &mut App, mut ready: impl FnMut(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.update();
        if ready(app.world()) {
            return;
        }
        std::thread::yield_now();
    }
    assert!(ready(app.world()), "真实后台任务应在测试期限内完成");
}

#[test]
fn actual_static_display_lod_keeps_source_metadata_and_recovers_for_authoring() {
    let mut app = app();
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let root = owner_root(app.world_mut(), true); // 建筑显示同样接入，物理结构不简化。
    let a = visual(app.world_mut(), root, source.clone());
    let b = visual(app.world_mut(), root, source.clone());
    camera(app.world_mut());
    until(&mut app, |world| {
        world.get::<Mesh3d>(a).is_some_and(|mesh| mesh.0 != source)
    });
    assert_eq!(
        app.world().get::<StaticMeshLodOriginal>(a).unwrap().mesh,
        source
    );
    assert_eq!(
        app.world().get::<Mesh3d>(a).unwrap().0,
        app.world().get::<Mesh3d>(b).unwrap().0
    ); // 同资产只生成一份LOD。
    let lod = app
        .world()
        .resource::<Assets<Mesh>>()
        .get(&app.world().get::<Mesh3d>(a).unwrap().0)
        .unwrap();
    assert!(lod.indices().unwrap().len() < grid().indices().unwrap().len());
    assert!(lod.attribute(Mesh::ATTRIBUTE_UV_0).is_some());
    assert_eq!(lod.asset_usage, grid().asset_usage);
    let prepared = app
        .world()
        .resource::<MeshLodRuntimeStatus>()
        .prepared_sources;
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(
        app.world()
            .resource::<MeshLodRuntimeStatus>()
            .prepared_sources,
        prepared
    ); // 自己换柄不重生成。
    app.world_mut()
        .resource_mut::<MeshLodAuthoringState>()
        .active = true;
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(a).unwrap().0, source);
    assert_eq!(app.world().get::<Mesh3d>(b).unwrap().0, source);
    assert!(
        app.world()
            .resource::<MeshLodRuntimeStatus>()
            .authoring_ready
    );
}

#[test]
fn authoring_world_picks_wait_until_all_lod_primitives_restore_across_the_fixed_frame_budget() {
    let mut app = app();
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let root = owner_root(app.world_mut(), false);
    let entities = (0..40)
        .map(|_| visual(app.world_mut(), root, source.clone()))
        .collect::<Vec<_>>();
    camera(app.world_mut());
    until(&mut app, |world| {
        world.resource::<resources::MeshLodRuntime>().coarse.len() == 40
    });
    app.world_mut()
        .resource_mut::<MeshLodAuthoringState>()
        .active = true;
    app.update();
    assert!(
        !app.world()
            .resource::<MeshLodRuntimeStatus>()
            .authoring_ready
    );
    assert_eq!(
        app.world()
            .resource::<resources::MeshLodRuntime>()
            .coarse
            .len(),
        8
    );
    app.update();
    assert!(
        app.world()
            .resource::<MeshLodRuntimeStatus>()
            .authoring_ready
    );
    assert!(
        entities
            .into_iter()
            .all(|entity| app.world().get::<Mesh3d>(entity).unwrap().0 == source)
    );
}

#[test]
fn external_mesh_replacement_and_owner_removal_never_adopt_previous_lod_source() {
    let mut app = app();
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let root = owner_root(app.world_mut(), false);
    let entity = visual(app.world_mut(), root, source.clone());
    camera(app.world_mut());
    until(&mut app, |world| {
        world
            .get::<Mesh3d>(entity)
            .is_some_and(|mesh| mesh.0 != source)
    });
    let replacement = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    app.world_mut()
        .entity_mut(entity)
        .insert(Mesh3d(replacement.clone()));
    until(&mut app, |world| {
        world
            .get::<StaticMeshLodOriginal>(entity)
            .is_some_and(|original| original.mesh == replacement)
    });
    app.world_mut()
        .entity_mut(root)
        .remove::<crate::ScenePlacementInstance>();
    app.update();
    assert!(app.world().get::<StaticMeshLodOriginal>(entity).is_none());
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, replacement);
    assert!(
        !app.world()
            .resource::<resources::MeshLodRuntime>()
            .targets
            .contains_key(&entity)
    );
}

#[test]
fn inherited_disabled_display_retains_original_and_does_not_launch_generation() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<feathrise_settings::RenderSettings>()
        .mesh_lod
        .display_enabled = false;
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let root = owner_root(app.world_mut(), false);
    let entity = visual(app.world_mut(), root, source.clone());
    camera(app.world_mut());
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, source);
    assert!(
        app.world()
            .resource::<resources::MeshLodRuntime>()
            .task
            .is_none()
    );
}

#[test]
fn late_parent_source_registration_uses_bounded_discovery_and_dynamic_props_are_excluded() {
    let mut app = app();
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let root = app.world_mut().spawn(Transform::default()).id();
    let entity = visual(app.world_mut(), root, source.clone());
    app.update();
    assert!(app.world().get::<StaticMeshLodOriginal>(entity).is_none());
    let temporary = owner_root(app.world_mut(), false);
    let identity = app
        .world()
        .get::<crate::ScenePlacementInstance>(temporary)
        .unwrap()
        .clone();
    app.world_mut().entity_mut(root).insert(identity);
    until(&mut app, |world| {
        world.get::<StaticMeshLodOriginal>(entity).is_some()
    });
    app.world_mut()
        .entity_mut(root)
        .insert(avian3d::prelude::RigidBody::Dynamic);
    app.update();
    assert!(app.world().get::<StaticMeshLodOriginal>(entity).is_none());
}

#[test]
fn screen_error_projection_accounts_for_shear_and_safe_camera_depth() {
    let view = types::View {
        inverse: Mat4::IDENTITY,
        focal_pixels: 1000.0,
        orthographic: false,
        layers: default(),
    };
    let pose = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, -100.0));
    let pixels = view.error_pixels(&pose, 0.0, 0.01);
    assert!(pixels < 1.0);
    assert!(
        view.error_pixels(&GlobalTransform::IDENTITY, 1.0, 0.01)
            .is_infinite()
    );
    let shear = GlobalTransform::from(Mat4::from_cols(
        Vec4::X,
        Vec4::new(4.0, 1.0, 0.0, 0.0),
        Vec4::Z,
        Vec4::new(0.0, 0.0, -100.0, 1.0),
    ));
    assert!(view.error_pixels(&shear, 0.0, 0.01) > pixels * 2.0);
}

#[test]
fn bounded_capture_keeps_custom_attributes_indices_and_full_metadata() {
    let mut source = grid();
    source.enable_raytracing = false;
    let custom = bevy::mesh::MeshVertexAttribute::new(
        "LODTest",
        9283746,
        bevy::render::render_resource::VertexFormat::Uint32,
    );
    source.insert_attribute(
        custom,
        VertexAttributeValues::Uint32((0..24 * 24).collect()),
    );
    let mut assets = Assets::<Mesh>::default();
    let handle = assets.add(source.clone());
    let key = types::SourceKey {
        id: handle.id(),
        revision: 0,
        epoch: 0,
    };
    let mut capture = capture::Capture::new(key, &source).unwrap();
    let mut frames = 0;
    while !capture.step(&source, 128).unwrap() {
        frames += 1;
        assert!(frames < 1000);
    }
    assert!(frames > 1);
    let captured = capture.finish().unwrap();
    assert_eq!(captured.attribute(custom), source.attribute(custom));
    assert_eq!(captured.indices(), source.indices());
    assert!(!captured.enable_raytracing);
    assert_eq!(captured.asset_usage, source.asset_usage);
}

#[test]
fn offscreen_preview_camera_layers_do_not_force_world_mesh_back_to_original() {
    let mut app = app();
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let root = owner_root(app.world_mut(), false);
    let entity = visual(app.world_mut(), root, source.clone());
    camera(app.world_mut());
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            computed: ComputedCameraValues {
                target_info: Some(RenderTargetInfo {
                    physical_size: UVec2::new(4096, 4096),
                    scale_factor: 1.0,
                }),
                ..default()
            },
            ..default()
        },
        Projection::Perspective(PerspectiveProjection::default()),
        GlobalTransform::IDENTITY,
        bevy::camera::visibility::RenderLayers::layer(18),
    )); // 近面相交的独立预览镜头属于另一层。
    until(&mut app, |world| {
        world
            .get::<Mesh3d>(entity)
            .is_some_and(|mesh| mesh.0 != source)
    });
}

#[test]
fn overflowing_asset_events_invalidate_once_and_adopt_new_source_geometry() {
    let mut app = app();
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let root = owner_root(app.world_mut(), false);
    let entity = visual(app.world_mut(), root, source.clone());
    camera(app.world_mut());
    until(&mut app, |world| {
        world
            .get::<Mesh3d>(entity)
            .is_some_and(|mesh| mesh.0 != source)
    });
    let old_key = app.world().resource::<resources::MeshLodRuntime>().targets[&entity].key;
    let mut replacement = grid();
    if let VertexAttributeValues::Float32x3(positions) =
        replacement.attribute_mut(Mesh::ATTRIBUTE_POSITION).unwrap()
    {
        for position in positions {
            position[1] = 50.0;
        }
    }
    app.world_mut()
        .resource_mut::<Assets<Mesh>>()
        .insert(source.id(), replacement)
        .unwrap();
    for _ in 0..129 {
        app.world_mut()
            .resource_mut::<Messages<bevy::asset::AssetEvent<Mesh>>>()
            .write(bevy::asset::AssetEvent::Modified { id: source.id() });
    }
    app.update();
    assert_ne!(
        app.world().resource::<resources::MeshLodRuntime>().targets[&entity].key,
        old_key
    );
    until(&mut app, |world| {
        world
            .get::<Mesh3d>(entity)
            .is_some_and(|mesh| mesh.0 != source)
    });
    let mesh = app
        .world()
        .resource::<Assets<Mesh>>()
        .get(&app.world().get::<Mesh3d>(entity).unwrap().0)
        .unwrap();
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        unreachable!()
    };
    assert!(positions.iter().all(|position| position[1] == 50.0)); // 原始分配换代后不能再采用旧高度派生。
}

#[test]
fn pending_large_asset_retirement_keeps_credit_until_the_real_background_release_finishes() {
    use resources::{CACHE_BYTES, MeshLodRuntime, UPLOAD_BUDGET, Upload};
    use types::{VisualCache, VisualLevel};
    let mut app = app();
    let mut tiny = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD,
    );
    tiny.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0; 3]; 3]);
    let tiny_bytes = capture::mesh_bytes(&tiny);
    let tiny_handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(tiny);
    let old_small_source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let old_large_source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let new_source = app.world_mut().resource_mut::<Assets<Mesh>>().add(grid());
    let mut large_levels = Vec::new();
    for _ in 0..4 {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD,
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0; 3]; UPLOAD_BUDGET / 12 / 3 * 3],
        );
        let bytes = capture::mesh_bytes(&mesh);
        let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
        large_levels.push(VisualLevel {
            mesh: handle,
            error: 0.0,
            bytes,
        });
    }
    let large_handles = large_levels
        .iter()
        .map(|level| level.mesh.clone())
        .collect::<Vec<_>>();
    let large_bytes = large_levels.iter().map(|level| level.bytes).sum::<usize>();
    let initial_credit = tiny_bytes + large_bytes;
    let pending = grid();
    let upload_bytes = capture::mesh_bytes(&pending);
    assert!(initial_credit <= CACHE_BYTES && initial_credit + upload_bytes > CACHE_BYTES);
    let new_key;
    {
        let mut runtime = app.world_mut().resource_mut::<MeshLodRuntime>();
        let small_key = runtime.key(old_small_source.id());
        let large_key = runtime.key(old_large_source.id());
        runtime.cache.insert(
            small_key,
            VisualCache {
                levels: vec![VisualLevel {
                    mesh: tiny_handle,
                    error: 0.0,
                    bytes: tiny_bytes,
                }],
                bytes: tiny_bytes,
                pins: 0,
                complete: true,
                failed: false,
                retry_credit: None,
            },
        );
        runtime.cache.insert(
            large_key,
            VisualCache {
                levels: large_levels,
                bytes: large_bytes,
                pins: 0,
                complete: true,
                failed: false,
                retry_credit: None,
            },
        );
        runtime.queue_cache(small_key);
        runtime.queue_cache(large_key);
        runtime.cache_bytes = initial_credit;
        runtime.watch(new_source.id());
        new_key = runtime.key(new_source.id());
        runtime.retain_reference(new_key);
        runtime.cache.insert(
            new_key,
            VisualCache {
                levels: Vec::new(),
                bytes: 0,
                pins: 0,
                complete: false,
                failed: false,
                retry_credit: None,
            },
        );
        runtime.queue_cache(new_key);
        runtime.upload = Some(Upload {
            key: new_key,
            pending: vec![feathrise_terrain::StaticMeshLod {
                mesh: pending,
                max_error: 0.0,
            }]
            .into(),
        });
    }
    app.update(); // 先从Assets取走前端小网格；后面四个大数组仍真实驻留。
    assert_eq!(
        app.world().resource::<MeshLodRuntime>().cache_bytes,
        initial_credit
    );
    assert_eq!(app.world().resource::<MeshLodRuntime>().credit_revision, 0);
    assert!(app.world().resource::<MeshLodRuntime>().upload.is_some());
    assert!(
        large_handles
            .iter()
            .all(|handle| app.world().resource::<Assets<Mesh>>().contains(handle.id()))
    );

    let release_ready = |app: &App| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !app
            .world()
            .resource::<MeshLodRuntime>()
            .release
            .as_ref()
            .unwrap()
            .is_finished()
        {
            assert!(Instant::now() < deadline, "后台退役应完成");
            std::thread::yield_now();
        }
    };
    release_ready(&app);
    app.update();
    assert_eq!(
        app.world().resource::<MeshLodRuntime>().cache_bytes,
        initial_credit - tiny_bytes
    );
    assert!(app.world().resource::<MeshLodRuntime>().upload.is_some()); // 小信用归还仍不能为等待中的大数组重复透支。
    assert_eq!(app.world().resource::<MeshLodRuntime>().credit_revision, 1);
    let first_large_bytes = app.world().resource::<MeshLodRuntime>().release_credit;
    assert!(first_large_bytes > upload_bytes);
    release_ready(&app);
    app.update();
    let runtime = app.world().resource::<MeshLodRuntime>();
    assert!(runtime.cache[&new_key].complete); // 大数组真实析构、信用归还后继续原完整上传。
    assert!(runtime.upload.is_none());
    assert_eq!(
        runtime.cache_bytes,
        initial_credit - tiny_bytes - first_large_bytes + upload_bytes
    );
    assert!(runtime.cache_bytes <= CACHE_BYTES);
    until(&mut app, |world| {
        let runtime = world.resource::<MeshLodRuntime>();
        runtime.retired.is_empty()
            && runtime.release.is_none()
            && runtime.cache_bytes == upload_bytes
    });
}
