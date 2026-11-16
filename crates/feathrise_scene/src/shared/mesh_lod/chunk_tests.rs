//! 内置网格的真实后台分块与混合显示回归；检查原始作者、碰撞、迟到票据和完整退役。
use super::*;
use bevy::{camera::primitives::Aabb, mesh::VertexAttributeValues};
use feathrise_scene_content::{ScenePlacementAssetTarget, ScenePlacementRecord};
use std::collections::{BTreeMap, BTreeSet};

#[path = "../../../tests/fixtures/editable-chunk-lod-v1.rs"]
mod fixture;

fn source() -> Mesh { crate::editable_mesh_mesh(fixture::geometry()) }

#[test]
fn dense_chunk_preview_reports_actual_reduction_and_restores_the_identical_source() {
    use bevy::{asset::RenderAssetUsages, mesh::{Indices, PrimitiveTopology}};
    let (mut app, visual, _, original) = setup();
    let root = app.world().get::<ChildOf>(visual).unwrap().parent();
    // 与实际细分立方体一致：每个三角拥有独立的作者角点、UV 和线框标记。
    let mut positions = Vec::new();
    for y in 0..24 {
        for x in 0..24 {
            // 二进制可精确表示的格距保证相邻三角共点，不把浮点加法尾差误当真实接缝。
            let [x, y] = [x as f32 / 16.0 - 1.0, y as f32 / 16.0 - 1.0];
            let next_x = x + 1.0 / 16.0;
            let next_y = y + 1.0 / 16.0;
            positions.extend([[x,y,0.0], [next_x,y,0.0], [next_x,next_y,0.0],
                [x,y,0.0], [next_x,next_y,0.0], [x,next_y,0.0]]);
        }
    }
    let count = positions.len();
    let dense = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0,0.0,1.0]; count])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, [[0.0,0.0], [1.0,0.0], [1.0,1.0]].repeat(count / 3))
        .with_inserted_attribute(crate::ATTRIBUTE_AUTHORING_WIRE, vec![0.0; count])
        .with_inserted_indices(Indices::U32((0..count as u32).collect()));
    app.world_mut().resource_mut::<Assets<Mesh>>().insert(original.id(), dense).unwrap();
    let overlay = app.world_mut().spawn((Mesh3d(original.clone()), MeshLodAuxiliary,
        MeshMaterial3d::<StandardMaterial>(default()), ChildOf(root))).id();
    app.world_mut().resource_mut::<MeshLodAuthoringState>().active = true;
    let request = MeshLodInspectionRequest { root, original: false, triangles: true };
    app.world_mut().resource_mut::<MeshLodInspection>().request = Some(request);
    tests::until(&mut app, |world| world.resource::<MeshLodInspection>().sample
        .is_some_and(|sample| sample.displayed_triangles < sample.original_triangles));
    let report = app.world().resource::<MeshLodInspection>().sample.unwrap();
    let adopted = app.world().get::<Mesh3d>(visual).unwrap();
    assert_ne!(adopted.0, original);
    assert_eq!(report.original_triangles, count / 3);
    assert!(!app.world().resource::<resources::MeshLodRuntime>().targets.contains_key(&overlay));
    assert!(report.displayed_triangles < count / 6, "{report:?}");
    assert_eq!(report.displayed_triangles, app.world().resource::<Assets<Mesh>>()
        .get(&adopted.0).unwrap().indices().unwrap().len() / 3);
    let installed = app.world().resource::<Assets<Mesh>>().get(&adopted.0).unwrap();
    assert_eq!(installed.indices().unwrap().iter().collect::<BTreeSet<_>>().len(), installed.count_vertices());
    assert!(app.world().get::<StaticMeshChunkLodStatus>(visual).unwrap().levels.len() > 2);

    app.world_mut().resource_mut::<MeshLodInspection>().request =
        Some(MeshLodInspectionRequest { original: true, ..request });
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(visual).unwrap().0, original);
    assert_eq!(app.world().resource::<MeshLodInspection>().sample.unwrap().displayed_triangles, count / 3);
    app.world_mut().resource_mut::<MeshLodInspection>().request = Some(request);
    tests::until(&mut app, |world| world.get::<Mesh3d>(visual).unwrap().0 != original);
    app.world_mut().resource_mut::<MeshLodInspection>().request = None;
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(visual).unwrap().0, original);
    assert!(app.world().resource::<MeshLodInspection>().sample.is_none());
}

fn setup() -> (App, Entity, Entity, Handle<Mesh>) {
    let mut app = tests::app();
    let record: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id":"editable", "asset_path":"", "asset_target": ScenePlacementAssetTarget::EditableMesh {
            geometry: fixture::geometry(), color_srgb: [160; 3], roughness_milli: 700, collision: true,
        }, "translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1]
    })).unwrap();
    let root = app.world_mut().spawn((crate::ScenePlacementInstance::new(
        &feathrise_scene_content::SceneId::parse("MAP-chunks").unwrap(), &record),
        avian3d::prelude::RigidBody::Static, Transform::default(), GlobalTransform::IDENTITY)).id();
    let material = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial::default());
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(source());
    let entity = tests::visual(app.world_mut(), root, source.clone());
    app.world_mut().entity_mut(entity).insert((MeshMaterial3d(material), Aabb::from_min_max(Vec3::splat(-1.5), Vec3::splat(1.5)),
        avian3d::prelude::Collider::cuboid(3.0, 3.0, 3.0)));
    tests::camera(app.world_mut());
    let camera = app.world_mut().query_filtered::<Entity, With<Camera3d>>().single(app.world()).unwrap();
    (app, entity, camera, source)
}

#[test]
fn identical_chunk_levels_share_one_mesh_until_the_last_instance_releases_it() {
    let (mut app, first, _, source) = setup();
    let other_root = tests::owner_root(app.world_mut(), false);
    let other = tests::visual(app.world_mut(), other_root, source.clone());
    let material = app.world().get::<MeshMaterial3d<StandardMaterial>>(first).unwrap().clone();
    let bounds = *app.world().get::<Aabb>(first).unwrap();
    app.world_mut().entity_mut(other).insert((material, bounds));
    tests::until(&mut app, |world| world.get::<StaticMeshChunkLodStatus>(first).is_some()
        && world.get::<StaticMeshChunkLodStatus>(other).is_some());
    let shared = app.world().get::<Mesh3d>(first).unwrap().0.clone();
    assert_eq!(app.world().get::<Mesh3d>(other).unwrap().0, shared);
    assert_ne!(shared, source);

    app.world_mut().despawn(first);
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(other).unwrap().0, shared);
    assert!(app.world().resource::<Assets<Mesh>>().contains(shared.id()));

    app.world_mut().resource_mut::<feathrise_settings::RenderSettings>().mesh_lod.display_enabled = false;
    tests::until(&mut app, |world| !world.resource::<Assets<Mesh>>().contains(shared.id()));
    assert_eq!(app.world().get::<Mesh3d>(other).unwrap().0, source);
}

#[test]
fn editable_chunks_preserve_all_shared_boundary_edges_and_mix_independent_levels() {
    let chunks = feathrise_terrain::build_editable_mesh_chunk_lods(source()).unwrap();
    assert_eq!(chunks.chunks.len(), 2);
    assert!(chunks.chunks.iter().any(|chunk| chunk.levels.len() > 1), "真实内置网格应能减面");
    fn boundary(indices: &[u32]) -> BTreeSet<[u32; 2]> {
        let mut edges = BTreeMap::<[u32; 2], usize>::new();
        for triangle in indices.chunks_exact(3) { for edge in [[triangle[0], triangle[1]], [triangle[1], triangle[2]], [triangle[2], triangle[0]]] {
            let key = [edge[0].min(edge[1]), edge[0].max(edge[1])];
            *edges.entry(key).or_default() += 1;
        } }
        edges.into_iter().filter_map(|(edge, count)| (count == 1).then_some(edge)).collect()
    }
    for chunk in &chunks.chunks {
        let original = boundary(&chunk.levels[0].indices);
        for level in &chunk.levels[1..] { assert_eq!(boundary(&level.indices), original); }
    }
    let mut found = None;
    for step in 1..100 {
        let pose = Transform::from_xyz(step as f32 * 0.05, 0.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y);
        let view = types::View { inverse: pose.to_matrix().inverse(),
            focal_pixels: 500.0, orthographic: false, layers: default() };
        let levels = chunks.select(&[view], &GlobalTransform::IDENTITY, &default(), 1.0, 0.15, &[]);
        if levels.contains(&0) && levels.iter().any(|&level| level > 0) { found = Some(levels); break; }
    }
    let levels = found.expect("同一物体的近块与远块必须可以采用不同档位");
    let composed = chunks.compose(&levels).unwrap();
    assert!(composed.indices().unwrap().len() < source().indices().unwrap().len());
    assert!(chunks.compose(&[]).is_err());
    assert!(chunks.compose(&vec![usize::MAX; chunks.chunks.len()]).is_err());
}

#[test]
fn actual_chunk_pipeline_restores_editing_and_keeps_collision_and_author_geometry() {
    let (mut app, entity, camera, original) = setup();
    let shape = app.world().get::<avian3d::prelude::Collider>(entity).unwrap().shape().clone();
    tests::until(&mut app, |world| world.get::<StaticMeshChunkLodStatus>(entity).is_some());
    let status = app.world().get::<StaticMeshChunkLodStatus>(entity).unwrap();
    assert_eq!(status.levels.len(), 2);
    assert!(status.triangles < 108);
    let stable = app.world().get::<Mesh3d>(entity).unwrap().0.clone();
    for _ in 0..10 { app.update(); }
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, stable);
    app.world_mut().resource_mut::<MeshLodAuthoringState>().active = true;
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, original);
    assert!(app.world().resource::<MeshLodRuntimeStatus>().authoring_ready);
    assert!(app.world().get::<StaticMeshChunkLodStatus>(entity).is_none());
    app.world_mut().resource_mut::<MeshChunkLodPreview>().active = true;
    tests::until(&mut app, |world| world.get::<StaticMeshChunkLodStatus>(entity).is_some());
    assert!(app.world().resource::<MeshLodRuntimeStatus>().authoring_ready);
    // 通过真正相机移动选择混合档，物理形状身份不能跟随显示变化。
    let mut mixed = false;
    for step in 1..100 {
        app.world_mut().entity_mut(camera).insert(GlobalTransform::from(
            Transform::from_xyz(step as f32 * 0.05, 0.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y)));
        for _ in 0..10 { app.update(); std::thread::yield_now(); }
        if let Some(status) = app.world().get::<StaticMeshChunkLodStatus>(entity)
            && status.levels.contains(&0) && status.levels.iter().any(|&level| level > 0) { mixed = true; break; }
    }
    assert!(mixed);
    assert!(std::sync::Arc::ptr_eq(&shape.0, &app.world().get::<avian3d::prelude::Collider>(entity).unwrap().shape().0));
    assert_eq!(app.world().get::<StaticMeshLodOriginal>(entity).unwrap().mesh, original);
    let original_mesh = app.world().resource::<Assets<Mesh>>().get(&original).unwrap();
    assert_eq!(original_mesh.indices().unwrap().len() / 3, 108);
    assert!(original_mesh.contains_attribute(crate::ATTRIBUTE_AUTHORING_WIRE));
    let VertexAttributeValues::Float32x3(positions) = original_mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { panic!() };
    assert_eq!(positions.len(), 324);
    app.world_mut().resource_mut::<feathrise_settings::RenderSettings>().mesh_lod.display_enabled = false;
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, original);
    app.world_mut().despawn(entity);
    tests::until(&mut app, |world| {
        let runtime = world.resource::<resources::MeshLodRuntime>();
        runtime.targets.is_empty() && runtime.cache_bytes == 0 && runtime.composition.is_none() && runtime.release.is_none()
    });
}

#[test]
fn textured_override_restores_full_author_attributes_and_plain_material_can_resume() {
    let (mut app, entity, _, original) = setup();
    tests::until(&mut app, |world| world.get::<StaticMeshChunkLodStatus>(entity).is_some());
    let material = app.world().get::<MeshMaterial3d<StandardMaterial>>(entity).unwrap().0.clone();
    // 高光贴图是当前启用的可选材质扩展；它同样依赖作者UV，不能遗漏在保原判断之外。
    app.world_mut().resource_mut::<Assets<StandardMaterial>>().get_mut(&material).unwrap()
        .specular_texture = Some(Handle::default());
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, original);
    assert!(app.world().get::<StaticMeshChunkLodStatus>(entity).is_none());
    for _ in 0..10 { app.update(); }
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, original);
    assert!(app.world().resource::<Assets<Mesh>>().get(&original).unwrap().contains_attribute(Mesh::ATTRIBUTE_UV_0));
    app.world_mut().resource_mut::<Assets<StandardMaterial>>().get_mut(&material).unwrap().specular_texture = None;
    tests::until(&mut app, |world| world.get::<StaticMeshChunkLodStatus>(entity).is_some());
}

#[test]
fn composition_cannot_cross_owner_before_bounded_discovery_catches_up() {
    let (mut app, entity, camera, original) = setup();
    tests::until(&mut app, |world| world.resource::<resources::MeshLodRuntime>().composition.is_some());
    let old_owner = app.world().get::<ChildOf>(entity).unwrap().parent();
    let instance = app.world().get::<crate::ScenePlacementInstance>(old_owner).unwrap().clone();
    let new_owner = app.world_mut().spawn((instance, avian3d::prelude::RigidBody::Static)).id();
    app.world_mut().entity_mut(entity).insert(ChildOf(new_owner));
    let view = types::View {
        inverse: app.world().get::<GlobalTransform>(camera).unwrap().to_matrix().inverse(),
        focal_pixels: 500.0, orthographic: false, layers: default(),
    };
    // 模拟固定预算尚未轮到层级发现：只轮询已经运行的worker，不让登记系统先修正旧目标身份。
    let started = std::time::Instant::now();
    loop {
        app.world_mut().resource_scope(|world, mut runtime: Mut<resources::MeshLodRuntime>| {
            chunks::poll(world, &mut runtime, std::slice::from_ref(&view), Some(default()), false, false);
        });
        if app.world().resource::<resources::MeshLodRuntime>().composition.is_none() { break; }
        assert!(started.elapsed().as_secs() < 5, "真实组合worker未返回");
        std::thread::yield_now();
    }
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, original);
    assert!(app.world().get::<StaticMeshChunkLodStatus>(entity).is_none());
}

#[test]
fn hot_reload_during_composition_rejects_old_result_and_does_not_resurrect_deleted_target() {
    let (mut app, entity, _, original) = setup();
    tests::until(&mut app, |world| world.resource::<resources::MeshLodRuntime>().composition.is_some());
    app.world_mut().resource_mut::<Assets<Mesh>>().insert(original.id(), source()).unwrap();
    app.world_mut().resource_mut::<MeshLodAuthoringState>().active = true;
    tests::until(&mut app, |world| world.resource::<resources::MeshLodRuntime>().composition.is_none());
    assert_eq!(app.world().get::<Mesh3d>(entity).unwrap().0, original);
    assert!(app.world().get::<StaticMeshChunkLodStatus>(entity).is_none());
    app.world_mut().resource_mut::<MeshLodAuthoringState>().active = false;
    tests::until(&mut app, |world| world.resource::<resources::MeshLodRuntime>().composition.is_some());
    app.world_mut().despawn(entity);
    tests::until(&mut app, |world| {
        let runtime = world.resource::<resources::MeshLodRuntime>();
        runtime.targets.is_empty() && runtime.composition.is_none() && runtime.cache_bytes == 0
    });
    assert!(!app.world().entities().contains(entity));
}
