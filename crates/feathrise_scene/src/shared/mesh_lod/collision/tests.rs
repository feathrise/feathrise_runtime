//! 真实worker和物理形状回归：相机不决定承托、安全失败保原、单点替换无重复。
use super::*;
use crate::MeshLodAuthoringState;
use crate::shared::mesh_lod::tests::{grid, owner_root, until};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<feathrise_settings::RenderSettings>()
        .init_resource::<MeshLodAuthoringState>()
        .init_resource::<super::super::MeshLodRuntimeStatus>();
    install(&mut app);
    app.world_mut()
        .resource_mut::<feathrise_settings::RenderSettings>()
        .mesh_lod
        .collision_enabled = true;
    app
}

fn source() -> Collider {
    let mesh = grid();
    let Some(VertexAttributeValues::Float32x3(points)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        unreachable!()
    };
    let indices = mesh
        .indices()
        .unwrap()
        .iter()
        .map(|index| index as u32)
        .collect::<Vec<_>>();
    Collider::trimesh_with_config(
        points.iter().copied().map(Vec3::from_array).collect(),
        indices
            .chunks_exact(3)
            .map(|triangle| [triangle[0], triangle[1], triangle[2]])
            .collect(),
        TrimeshFlags::FIX_INTERNAL_EDGES,
    )
}

#[test]
fn actual_far_collision_lod_recovers_near_player_without_duplicate_colliders() {
    let mut app = app();
    let root = owner_root(app.world_mut(), false);
    let original = source();
    app.world_mut().entity_mut(root).insert(original.clone());
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Position(Vec3::splat(1000.0)),
            GlobalTransform::from_translation(Vec3::splat(1000.0)),
        ))
        .id();
    until(&mut app, |world| {
        world
            .get::<Collider>(root)
            .is_some_and(|collider| key(collider) != key(&original))
    });
    let coarse = app.world().get::<Collider>(root).unwrap();
    assert!(
        coarse
            .shape()
            .as_shape::<StaticTerrainShape>()
            .unwrap()
            .mesh()
            .indices()
            .len()
            < source_mesh(&original).unwrap().indices().len()
    );
    assert_eq!(
        app.world_mut()
            .query::<&Collider>()
            .iter(app.world())
            .count(),
        2
    );
    app.world_mut().entity_mut(body).insert((
        Position(Vec3::new(3.0, 1.0, 3.0)),
        GlobalTransform::from_translation(Vec3::new(3.0, 1.0, 3.0)),
    ));
    app.world_mut().run_schedule(FixedPreUpdate); // 不等全部地图轮转，下一物理步先恢复近区原始承托。
    assert_eq!(
        key(app.world().get::<Collider>(root).unwrap()),
        key(&original)
    );
    assert!(app.world().resource::<Runtime>().coarse.is_empty());
}

#[test]
fn no_focus_building_and_disabled_physics_never_simplify_original_support() {
    let mut app = app();
    let root = owner_root(app.world_mut(), false);
    let original = source();
    app.world_mut().entity_mut(root).insert(original.clone());
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(
        key(app.world().get::<Collider>(root).unwrap()),
        key(&original)
    );
    assert!(app.world().resource::<Runtime>().task.is_none());
    let building = owner_root(app.world_mut(), true);
    app.world_mut().entity_mut(building).insert(source());
    app.world_mut().spawn((
        RigidBody::Dynamic,
        Collider::sphere(0.5),
        Position(Vec3::splat(1000.0)),
        GlobalTransform::from_translation(Vec3::splat(1000.0)),
    ));
    app.world_mut()
        .resource_mut::<feathrise_settings::RenderSettings>()
        .mesh_lod
        .collision_enabled = false;
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(
        key(app.world().get::<Collider>(root).unwrap()),
        key(&original)
    );
    assert!(
        !app.world()
            .resource::<Runtime>()
            .targets
            .contains_key(&building)
    );
}

#[test]
fn production_wrapped_mesh_preserves_scale_winding_and_weak_source_lease() {
    let original = source();
    let mesh = source_mesh(&original).unwrap().clone();
    let mut wrapped = Collider::from(SharedShape::new(StaticTerrainShape::new(mesh)));
    wrapped.set_scale(Vec3::new(2.0, 3.0, 4.0), 10);
    let pointer = key(&wrapped);
    let built = cook(wrapped.clone()).unwrap();
    assert!(!built.levels.is_empty());
    for level in built.levels.iter() {
        assert_eq!(level.collider.scale(), Vec3::new(2.0, 3.0, 4.0));
        let ray = avian3d::parry::query::Ray::new(Vec3::new(3.0, 10.0, 3.0), Vec3::NEG_Y);
        let hit = level
            .collider
            .shape_scaled()
            .cast_local_ray_and_get_normal(&ray, 20.0, false)
            .unwrap();
        let normal = level
            .collider
            .shape_scaled()
            .feature_normal_at_point(hit.subshape, hit.feature, ray.point_at(hit.time_of_impact))
            .unwrap();
        assert!(normal.y > 0.9);
    }
    drop(wrapped);
    assert!(built.source.upgrade().is_none()); // 弱票据阻止地址复用，不强留原始大BVH。
    assert_ne!(key(&source()), pointer);
}

#[test]
fn rotated_unit_terrain_proxy_keeps_prepared_shape_identity_through_real_avian_insert_hooks() {
    use crate::shared::placement_data::TerrainPhysicsProxy;
    use std::sync::Arc;
    let mut app = app();
    app.init_resource::<Assets<Mesh>>()
        .add_plugins(ColliderBackendPlugin::<Collider>::default());
    let (rotation, pose) = (1..1000)
        .find_map(|index| {
            let rotation = Quat::from_euler(EulerRot::XYZ, index as f32 * 0.019, 0.64, 0.28);
            let pose = GlobalTransform::from(
                Transform::from_rotation(rotation).with_translation(Vec3::new(5.0, 2.0, 8.0)),
            );
            (pose.scale() != Vec3::ONE).then_some((rotation, pose))
        })
        .expect("夹具必须具有实际旋转分解的单位舍入残差");
    let position = Vec3::new(5.0, 2.0, 8.0);
    let entity = app
        .world_mut()
        .spawn((
            TerrainPhysicsProxy,
            RigidBody::Static,
            Transform::from_rotation(rotation).with_translation(position),
            pose,
            Position(position),
            Rotation(rotation),
        ))
        .id();
    let original = Collider::from(SharedShape::new(StaticTerrainShape::new(
        source_mesh(&source()).unwrap().clone(),
    )));
    let built = cook(original.clone()).unwrap();
    let coarse = built.levels.last().unwrap().collider.clone();
    for prepared in [original.clone(), coarse, original] {
        let source_shape = prepared.shape().0.clone();
        systems::adopt_collider(app.world_mut(), entity, prepared);
        let adopted = app.world().get::<Collider>(entity).unwrap();
        assert_eq!(adopted.scale(), Vec3::ONE);
        assert!(Arc::ptr_eq(&adopted.shape().0, &source_shape));
        assert!(Arc::ptr_eq(&adopted.shape_scaled().0, &source_shape)); // 真实on_insert没有重建任何BVH。
        assert_eq!(*app.world().get::<GlobalTransform>(entity).unwrap(), pose);
        assert_eq!(app.world().get::<Position>(entity).unwrap().0, position);
        assert_eq!(app.world().get::<Rotation>(entity).unwrap().0, rotation);
        app.world_mut().run_schedule(FixedPostUpdate);
        assert!(Arc::ptr_eq(
            &app.world()
                .get::<Collider>(entity)
                .unwrap()
                .shape_scaled()
                .0,
            &source_shape
        )); // 固定步继续读取精确Transform.scale。
    }
}

#[test]
fn existing_contact_edge_restores_original_before_next_physics_step() {
    let mut app = app();
    let root = owner_root(app.world_mut(), false);
    let original = source();
    app.world_mut().entity_mut(root).insert(original.clone());
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Position(Vec3::splat(1000.0)),
            GlobalTransform::from_translation(Vec3::splat(1000.0)),
        ))
        .id();
    until(&mut app, |world| {
        world
            .get::<Collider>(root)
            .is_some_and(|collider| key(collider) != key(&original))
    });
    app.world_mut().init_resource::<ContactGraph>();
    app.world_mut()
        .resource_mut::<ContactGraph>()
        .add_edge(ContactEdge::new(root, body))
        .unwrap();
    app.world_mut().run_schedule(FixedPreUpdate);
    assert_eq!(
        key(app.world().get::<Collider>(root).unwrap()),
        key(&original)
    );
    assert!(app.world().resource::<Runtime>().coarse.is_empty());
}

#[test]
fn changing_avian_coarse_scale_restores_canonical_full_topology_and_does_not_promote_lod_to_source()
{
    let mut app = app();
    let root = owner_root(app.world_mut(), false);
    let original = source();
    let original_triangles = source_mesh(&original).unwrap().indices().len();
    app.world_mut().entity_mut(root).insert(original.clone());
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Position(Vec3::splat(1000.0)),
            GlobalTransform::from_translation(Vec3::splat(1000.0)),
        ))
        .id();
    until(&mut app, |world| {
        world
            .get::<Collider>(root)
            .is_some_and(|collider| key(collider) != key(&original))
    });
    let mut changed = app.world().get::<Collider>(root).unwrap().clone();
    changed.set_scale(Vec3::splat(2.0), 10); // 模拟Avian对当前immutable派生Collider重缩放后重新insert。
    app.world_mut()
        .entity_mut(root)
        .insert((GlobalTransform::from_scale(Vec3::splat(2.0)), changed));
    app.update();
    let restored = app.world().get::<Collider>(root).unwrap();
    assert_eq!(
        source_mesh(restored).unwrap().indices().len(),
        original_triangles
    );
    assert_eq!(restored.scale(), Vec3::splat(2.0));
    assert!(app.world().resource::<Runtime>().targets[&root].rescale_guard);
    app.world_mut().entity_mut(body).insert((
        Position(Vec3::new(3.0, 1.0, 3.0)),
        GlobalTransform::from_translation(Vec3::new(3.0, 1.0, 3.0)),
    ));
    app.world_mut().run_schedule(FixedPreUpdate);
    assert_eq!(
        source_mesh(app.world().get::<Collider>(root).unwrap())
            .unwrap()
            .indices()
            .len(),
        original_triangles
    );
}

#[test]
fn a_near_dynamic_body_with_position_but_no_global_transform_keeps_original_support() {
    let mut app = app();
    let root = owner_root(app.world_mut(), false);
    let original = source();
    app.world_mut().entity_mut(root).insert(original.clone());
    app.world_mut().spawn((
        RigidBody::Dynamic,
        Collider::sphere(0.5),
        Position(Vec3::splat(1000.0)),
        GlobalTransform::from_translation(Vec3::splat(1000.0)),
    ));
    app.world_mut().spawn((
        RigidBody::Dynamic,
        Collider::sphere(0.5),
        Position(Vec3::new(3.0, 1.0, 3.0)),
    ));
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(
        key(app.world().get::<Collider>(root).unwrap()),
        key(&original)
    );
    assert!(app.world().resource::<Runtime>().task.is_none());
}
