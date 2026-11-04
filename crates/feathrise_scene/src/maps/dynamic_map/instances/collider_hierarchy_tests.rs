//! 真实物理插件验证独立Clipmap块的延迟绑定、迟到归属与退役，不以最终修补掩盖失效关系。

use super::super::{components::SubMapBatch, physics};
use avian3d::prelude::*;
use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce, log::tracing, prelude::*};
use feathrise_scene_core::residency::{
    MapInstanceContextAppExt, MapInstanceOwner, MapRuntimeScope, initialize_instance_context,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct HierarchyWarnings(Arc<Mutex<Vec<String>>>);

// 线程局部订阅只记录本测试同步spawn/flush的关系警告，不占用全局LogPlugin。
impl tracing::Subscriber for HierarchyWarnings {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::WARN
            && metadata.target() == "avian3d::collision::collider::collider_hierarchy"
    }
    fn register_callsite(&self, _: &tracing::Metadata<'_>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if !self.enabled(event.metadata()) {
            return;
        }
        let mut message = String::new();
        event.record(
            &mut |field: &tracing::field::Field, value: &dyn std::fmt::Debug| {
                if field.name() == "message" {
                    message = format!("{value:?}");
                }
            },
        );
        self.0.lock().unwrap().push(message);
    }
}

#[derive(Resource, Default)]
struct InvalidAttachments(Vec<(Entity, Entity)>);

fn observe_attachment(
    insert: On<Insert, ColliderOf>,
    relations: Query<&ColliderOf>,
    bodies: Query<(), With<RigidBody>>,
    mut invalid: ResMut<InvalidAttachments>,
) {
    if let Ok(relation) = relations.get(insert.entity)
        && !bodies.contains(relation.body)
    {
        invalid.0.push((insert.entity, relation.body));
    }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default(),
    ))
    .insert_resource(Gravity(Vec3::ZERO))
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ))
    .init_resource::<InvalidAttachments>()
    .register_instance_resource::<MapRuntimeScope>()
    .add_observer(physics::consolidate_static_bodies)
    .add_observer(physics::repair_collider_body)
    .add_observer(physics::consolidate_late_owned_bodies)
    .add_observer(observe_attachment);
    app.finish();
    app
}

fn source(app: &mut App) -> (Entity, Entity) {
    let batch = app
        .world_mut()
        .spawn((
            SubMapBatch,
            Transform::IDENTITY,
            RigidBody::Static,
            RigidBodyDisabled,
            bevy_tnua::TnuaNotPlatform,
        ))
        .id();
    let source = app
        .world_mut()
        .spawn((Transform::IDENTITY, ChildOf(batch)))
        .id();
    app.world_mut()
        .entity_mut(source)
        .insert(MapInstanceOwner(source));
    initialize_instance_context(app.world_mut(), source);
    (batch, source)
}

fn stable(app: &mut App) {
    for _ in 0..5 {
        app.update();
    }
}

fn assert_clean(app: &App, warnings: &HierarchyWarnings) {
    assert!(
        warnings.0.lock().unwrap().is_empty(),
        "自动绑定不能提交已移除的刚体地址"
    );
    assert!(
        app.world().resource::<InvalidAttachments>().0.is_empty(),
        "最终批次地址正确仍不足以证明中途未插入失效ColliderOf"
    );
}

fn floor_hit(query: SpatialQuery, active: feathrise_grounding::GroundSurfaceFilter) -> Option<Entity> {
    query
        .cast_ray_predicate(
            Vec3::new(0.0, 10.0, 0.0),
            Dir3::NEG_Y,
            20.0,
            true,
            &SpatialQueryFilter::default(),
            &|entity| active.is_active(entity),
        )
        .map(|hit| hit.entity)
}

#[test]
fn clipmap_sixty_four_scoped_static_chunks_bind_without_stale_self_and_keep_batch_gate() {
    let warnings = HierarchyWarnings::default();
    tracing::subscriber::with_default(warnings.clone(), || {
        let mut app = app();
        let (batch, source) = source(&mut app);
        app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(source);
        let leaves: Vec<_> = {
            let mut commands = app.world_mut().commands();
            (0..64)
                .map(|index| {
                    // 与Clipmap publication一致：先生成独立静态块，再补挂源根。
                    let leaf = commands
                        .spawn((
                            RigidBody::Static,
                            Collider::cuboid(2.0, 1.0, 2.0),
                            Transform::from_xyz(
                                (index % 8) as f32 * 3.0,
                                -0.5,
                                (index / 8) as f32 * 3.0,
                            ),
                        ))
                        .id();
                    commands.entity(source).add_child(leaf);
                    leaf
                })
                .collect()
        };
        app.world_mut().flush();
        app.world_mut().resource_mut::<MapRuntimeScope>().instance = None;
        stable(&mut app);
        for &leaf in &leaves {
            assert!(app.world().get::<RigidBody>(leaf).is_none());
            assert_eq!(app.world().get::<ColliderOf>(leaf).unwrap().body, batch);
        }
        assert_clean(&app, &warnings);
        assert_eq!(app.world_mut().run_system_once(floor_hit).unwrap(), None);
        app.world_mut()
            .entity_mut(batch)
            .remove::<(RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)>();
        stable(&mut app);
        assert_eq!(
            app.world_mut().run_system_once(floor_hit).unwrap(),
            Some(leaves[0])
        );
        app.world_mut()
            .entity_mut(batch)
            .insert((RigidBodyDisabled, bevy_tnua::TnuaNotPlatform));
        stable(&mut app);
        assert_eq!(app.world_mut().run_system_once(floor_hit).unwrap(), None);
        assert_clean(&app, &warnings);
    });
}

#[test]
fn late_owned_static_chunks_move_to_batch_while_nearest_dynamic_body_survives() {
    let warnings = HierarchyWarnings::default();
    tracing::subscriber::with_default(warnings.clone(), || {
        let mut app = app();
        let (batch, source) = source(&mut app);
        let late = app
            .world_mut()
            .spawn((
                Transform::from_xyz(30.0, 0.0, 0.0),
                Collider::cuboid(1.0, 1.0, 1.0),
                RigidBody::Static,
            ))
            .id();
        app.world_mut().flush();
        assert_eq!(app.world().get::<ColliderOf>(late).unwrap().body, late);
        // 模型已有独立刚体，异步完成后才补父级和Owner；此时仍要安全交给批次。
        app.world_mut().entity_mut(late).insert(ChildOf(source));
        app.world_mut().flush();
        stable(&mut app);
        assert!(app.world().get::<RigidBody>(late).is_none());
        assert_eq!(app.world().get::<ColliderOf>(late).unwrap().body, batch);

        let moving = app
            .world_mut()
            .spawn((
                Transform::from_xyz(40.0, 0.0, 0.0),
                ChildOf(source),
                RigidBody::Dynamic,
            ))
            .id();
        let nested = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                ChildOf(moving),
                RigidBody::Static,
                Collider::cuboid(1.0, 1.0, 1.0),
            ))
            .id();
        stable(&mut app);
        assert!(app.world().get::<RigidBody>(nested).is_none());
        assert_eq!(
            app.world().get::<RigidBody>(moving),
            Some(&RigidBody::Dynamic)
        );
        assert_eq!(app.world().get::<ColliderOf>(nested).unwrap().body, moving);
        assert_clean(&app, &warnings);
    });
}

#[test]
fn ordinary_nearest_bodies_and_retirement_keep_manual_invalid_relation_diagnostics() {
    let warnings = HierarchyWarnings::default();
    tracing::subscriber::with_default(warnings.clone(), || {
        let mut app = app();
        // 根地图没有实例scope：普通静态/动态嵌套仍按Avian最近祖先绑定。
        let root = app
            .world_mut()
            .spawn((Transform::IDENTITY, RigidBody::Static))
            .id();
        let nested = app
            .world_mut()
            .spawn((
                Transform::from_xyz(30.0, 0.0, 0.0),
                ChildOf(root),
                RigidBody::Static,
            ))
            .id();
        let leaf = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                ChildOf(nested),
                Collider::cuboid(1.0, 1.0, 1.0),
            ))
            .id();
        stable(&mut app);
        assert_eq!(app.world().get::<ColliderOf>(leaf).unwrap().body, nested);
        app.world_mut().entity_mut(leaf).insert(ChildOf(root));
        stable(&mut app);
        assert_eq!(app.world().get::<ColliderOf>(leaf).unwrap().body, root);
        assert_clean(&app, &warnings);

        // 真实坏显式关系必须继续报警，证明本修复没有静默关闭关系校验。
        let missing_body = app.world_mut().spawn(Transform::IDENTITY).id();
        app.world_mut()
            .entity_mut(leaf)
            .insert(ColliderOf { body: missing_body });
        app.world_mut().flush();
        assert_eq!(warnings.0.lock().unwrap().len(), 1);
        assert_eq!(
            app.world().resource::<InvalidAttachments>().0,
            [(leaf, missing_body)]
        );
        app.world_mut()
            .entity_mut(leaf)
            .insert(ColliderOf { body: root });
        warnings.0.lock().unwrap().clear();
        app.world_mut()
            .resource_mut::<InvalidAttachments>()
            .0
            .clear();

        app.world_mut().entity_mut(root).despawn();
        app.world_mut().entity_mut(missing_body).despawn();
        stable(&mut app);
        assert!(app.world().get_entity(root).is_err());
        assert!(app.world().get_entity(nested).is_err());
        assert!(app.world().get_entity(leaf).is_err());
        assert_clean(&app, &warnings);
    });
}

fn triangle_floor() -> Collider {
    Collider::trimesh(
        vec![
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(2.0, 0.0, -2.0),
            Vec3::new(2.0, 0.0, 2.0),
            Vec3::new(-2.0, 0.0, 2.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    )
}

fn proxy_count_for(app: &App, collider: Entity) -> usize {
    app.world()
        .resource::<avian3d::collider_tree::ColliderTrees>()
        .iter_trees()
        .map(|tree| {
            tree.proxies
                .iter()
                .filter(|(_, proxy)| proxy.collider == collider)
                .count()
        })
        .sum()
}

#[test]
fn detached_trimesh_updates_tree_identity_and_repeated_reattach_leaves_one_proxy() {
    use avian3d::collider_tree::{ColliderTreeProxyKey, ColliderTreeType, ColliderTrees};

    let mut app = app();
    let body = app.world_mut().spawn((Transform::IDENTITY, RigidBody::Static)).id();
    let collider = app
        .world_mut()
        .spawn((Transform::IDENTITY, ChildOf(body), triangle_floor()))
        .id();
    stable(&mut app);
    assert_eq!(app.world().get::<ColliderOf>(collider).unwrap().body, body);

    for _ in 0..16 {
        // 真正关系移除须同步更新代理身份，不能仅在重绑后检查最终ColliderOf正确。
        app.world_mut().entity_mut(collider).remove::<ColliderOf>();
        app.world_mut().flush();
        let detached = *app.world().get::<ColliderTreeProxyKey>(collider).unwrap();
        assert_eq!(detached.tree_type(), ColliderTreeType::Standalone);
        assert_eq!(app.world().resource::<ColliderTrees>().get_proxy(detached).unwrap().body, None);
        assert_eq!(proxy_count_for(&app, collider), 1);

        app.world_mut().entity_mut(collider).insert(ColliderOf { body });
        app.world_mut().flush();
        let attached = *app.world().get::<ColliderTreeProxyKey>(collider).unwrap();
        assert_eq!(attached.tree_type(), ColliderTreeType::Static);
        assert_eq!(app.world().resource::<ColliderTrees>().get_proxy(attached).unwrap().body, Some(body));
        assert_eq!(proxy_count_for(&app, collider), 1, "重绑必须撤销旧Standalone代理");
        stable(&mut app);
        assert!(!app.world().resource::<ContactGraph>().contains(collider, collider));
    }

    app.world_mut().entity_mut(collider).despawn();
    stable(&mut app);
    assert_eq!(proxy_count_for(&app, collider), 0, "退役不能遗留未被当前key管理的代理");
}

#[test]
fn broad_phase_rejects_duplicate_proxy_self_pair_but_keeps_distinct_dynamic_trimesh_contact() {
    use avian3d::collider_tree::{ColliderTreeProxyKey, ColliderTreeType, ColliderTrees, MovedProxies};

    let mut app = app();
    let ground = app
        .world_mut()
        .spawn((Transform::IDENTITY, RigidBody::Static, triangle_floor()))
        .id();
    app.world_mut().flush();
    let key = *app.world().get::<ColliderTreeProxyKey>(ground).unwrap();
    let (aabb, mut duplicate) = {
        let trees = app.world().resource::<ColliderTrees>();
        (
            trees.tree_for_type(key.tree_type()).get_proxy_aabb(key.id()).unwrap(),
            trees.get_proxy(key).unwrap().clone(),
        )
    };
    // 刻意模拟历史漏回写留下的独立树代理，证明宽相防线不依赖正确的当前树索引。
    duplicate.body = None;
    let duplicate_id = app.world_mut().resource_mut::<ColliderTrees>().standalone_tree.add_proxy(aabb, duplicate);
    let duplicate_key = ColliderTreeProxyKey::new(duplicate_id, ColliderTreeType::Standalone);
    app.world_mut().resource_mut::<MovedProxies>().insert(duplicate_key);

    let moving = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.0, 0.4, 0.0),
            RigidBody::Dynamic,
            LockedAxes::ALL_LOCKED,
            // 闭合四面体具有有效动态质量；下表面真实穿过平面，不用伪造ContactGraph。
            Collider::trimesh(
                vec![
                    Vec3::new(-0.5, -0.5, -0.5),
                    Vec3::new(0.5, -0.5, -0.5),
                    Vec3::new(0.0, -0.5, 0.5),
                    Vec3::new(0.0, 0.5, 0.0),
                ],
                vec![[0, 1, 2], [0, 3, 1], [1, 3, 2], [2, 3, 0]],
            ),
        ))
        .id();
    stable(&mut app);
    let graph = app.world().resource::<ContactGraph>();
    assert!(!graph.contains(ground, ground), "同一Collider的两个代理不能生成自身接触");
    assert!(graph.contact_pairs_with(moving).any(|pair| {
        (pair.collider1 == ground || pair.collider2 == ground)
            && pair.is_touching()
            && pair.manifolds.iter().any(|manifold| !manifold.points.is_empty())
    }), "不同Collider的动态Trimesh与静态Trimesh必须继续产生真实接触");
}
