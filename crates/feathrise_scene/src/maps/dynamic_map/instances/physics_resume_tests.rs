//! 无窗口真实Avian暂停恢复回归：定位F12双时钟恢复后的窄相位、求解与分块换代。

use super::{components::SubMapBatch, physics};
use crate::shared::placement_data::StaticTerrainShape;
use avian3d::{
    collider_tree::{ColliderTreeProxyKey, ColliderTreeType, ColliderTrees},
    parry::shape::SharedShape,
    prelude::*,
    schedule::PhysicsTime,
};
use bevy::{asset::AssetPlugin, prelude::*};
use feathrise_scene_core::residency::{
    MapInstanceContextAppExt, MapInstanceOwner, MapRuntimeScope, initialize_instance_context,
};
use std::time::Duration;

const CELLS: usize = 32;
const CELL_SIZE: f32 = 0.125;
const CHUNK_SIZE: f32 = CELLS as f32 * CELL_SIZE;

#[derive(Resource)]
struct ResumeTrace {
    shape: &'static str,
    phase: &'static str,
    frame: usize,
    before_narrow: usize,
    after_narrow: usize,
    before_solver: usize,
    after_solver: usize,
}

impl ResumeTrace {
    fn log(&self, stage: &str) {
        eprintln!(
            "[terrain-physics-resume] shape={} phase={} frame={} stage={stage}",
            self.shape, self.phase, self.frame,
        );
    }
}

fn before_narrow(mut trace: ResMut<ResumeTrace>) {
    trace.before_narrow += 1;
    trace.log("before-narrow");
}

fn after_narrow(mut trace: ResMut<ResumeTrace>) {
    trace.after_narrow += 1;
    trace.log("after-narrow");
}

fn before_solver(mut trace: ResMut<ResumeTrace>) {
    trace.before_solver += 1;
    trace.log("before-solver");
}

fn after_solver(mut trace: ResMut<ResumeTrace>) {
    trace.after_solver += 1;
    trace.log("after-solver");
}

fn app(wrapped: bool) -> App {
    let mut app = App::new();
    let step = Duration::from_secs_f64(1.0 / 60.0);
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default(),
    ))
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(step))
    .insert_resource(Time::<Fixed>::from_duration(step))
    .insert_resource(ResumeTrace {
        shape: if wrapped { "production-wrapped" } else { "raw-trimesh" },
        phase: "initial-paused",
        frame: 0,
        before_narrow: 0,
        after_narrow: 0,
        before_solver: 0,
        after_solver: 0,
    })
    .register_instance_resource::<MapRuntimeScope>()
    .add_observer(physics::consolidate_static_bodies)
    .add_observer(physics::repair_collider_body)
    .add_observer(physics::consolidate_late_owned_bodies)
    .add_systems(
        PhysicsSchedule,
        (
            before_narrow
                .after(PhysicsStepSystems::BroadPhase)
                .before(PhysicsStepSystems::NarrowPhase),
            after_narrow
                .after(PhysicsStepSystems::NarrowPhase)
                .before(before_solver),
            before_solver.before(PhysicsStepSystems::Solver),
            after_solver
                .after(PhysicsStepSystems::Solver)
                .before(PhysicsStepSystems::Sleeping),
        ),
    );
    app.finish();
    pause(&mut app);
    // 先建立真实时钟基准，后续每次update精确推进一个固定步；不手动运行物理Schedule。
    app.update();
    app
}

fn pause(app: &mut App) {
    app.world_mut().resource_mut::<Time<Virtual>>().pause();
    app.world_mut().resource_mut::<Time<Physics>>().pause();
}

fn resume(app: &mut App, phase: &'static str) {
    app.world_mut().resource_mut::<ResumeTrace>().phase = phase;
    app.world_mut().resource_mut::<Time<Virtual>>().unpause();
    app.world_mut().resource_mut::<Time<Physics>>().unpause();
}

fn flat_chunk(wrapped: bool) -> Collider {
    let vertices = (0..=CELLS)
        .flat_map(|z| {
            (0..=CELLS).map(move |x| Vec3::new(x as f32 * CELL_SIZE, 0.0, z as f32 * CELL_SIZE))
        })
        .collect();
    let triangles = (0..CELLS)
        .flat_map(|z| {
            (0..CELLS).flat_map(move |x| {
                let a = (z * (CELLS + 1) + x) as u32;
                let b = a + 1;
                let c = a + (CELLS + 1) as u32;
                let d = c + 1;
                // 与地形网格一致的朝上绕序；边缘修正与正式碰撞构造保持一致。
                [[a, c, b], [b, c, d]]
            })
        })
        .collect();
    let raw = Collider::trimesh_with_config(vertices, triangles, TrimeshFlags::FIX_INTERNAL_EDGES);
    if wrapped {
        Collider::from(SharedShape::new(StaticTerrainShape::new(
            raw.shape().as_trimesh().unwrap().clone(),
        )))
    } else {
        raw
    }
}

fn spawn_chunk(app: &mut App, source: Entity, index: usize, shape: &Collider) -> Entity {
    let chunk = app
        .world_mut()
        .spawn((
            Transform::from_xyz(
                (index % 8) as f32 * CHUNK_SIZE - 4.0 * CHUNK_SIZE,
                0.0,
                (index / 8) as f32 * CHUNK_SIZE - 4.0 * CHUNK_SIZE,
            ),
            shape.clone(),
            RigidBody::Static,
        ))
        .id();
    // 正式publication先生成块，再挂源根；迟到Owner观察者须把叶刚体交给批次。
    app.world_mut().entity_mut(chunk).insert(ChildOf(source));
    app.world_mut()
        .entity_mut(chunk)
        .insert(MapInstanceOwner(source));
    chunk
}

fn fixed_count(app: &App) -> usize {
    app.world().resource::<ResumeTrace>().after_solver
}

fn advance(app: &mut App, frames: usize) {
    let before = fixed_count(app);
    for frame in 1..=frames {
        app.world_mut().resource_mut::<ResumeTrace>().frame = frame;
        app.update();
    }
    let trace = app.world().resource::<ResumeTrace>();
    assert_eq!(trace.after_solver, before + frames, "双恢复须逐帧运行固定物理步");
    assert_eq!(trace.before_narrow, trace.after_narrow, "窄相位须正常退出");
    assert_eq!(trace.after_narrow, trace.before_solver, "窄相位后须进入求解器");
    assert_eq!(trace.before_solver, trace.after_solver, "求解器须正常退出");
}

fn assert_support(app: &App, actor: Entity, chunks: &[Entity]) {
    let position = app.world().get::<Position>(actor).unwrap().0;
    eprintln!("[terrain-physics-resume] supported-position={position:?}");
    assert!(position.is_finite());
    assert!((0.8..1.2).contains(&position.y), "胶囊必须由真实地形承托：{position:?}");
    assert!(position.x.abs() < 0.1 && position.z.abs() < 0.1);
    let graph = app.world().resource::<ContactGraph>();
    assert!(
        graph.contact_pairs_with(actor).any(|pair| {
            pair.is_touching()
                && pair.generates_constraints()
                && (chunks.contains(&pair.collider1) || chunks.contains(&pair.collider2))
                && pair.manifolds.iter().any(|manifold| !manifold.points.is_empty())
        }),
        "恢复后必须存在角色与当前分块的实际接触和求解约束",
    );
}

fn exercise(wrapped: bool) {
    let mut app = app(wrapped);
    let batch = app
        .world_mut()
        .spawn((SubMapBatch, Transform::IDENTITY, RigidBody::Static, RigidBodyDisabled))
        .id();
    let source = app.world_mut().spawn((Transform::IDENTITY, ChildOf(batch))).id();
    app.world_mut().entity_mut(source).insert(MapInstanceOwner(source));
    initialize_instance_context(app.world_mut(), source);
    let shape = flat_chunk(wrapped);
    let mut chunks: Vec<_> = (0..64)
        .map(|index| spawn_chunk(&mut app, source, index, &shape))
        .collect();
    let actor = app
        .world_mut()
        .spawn((
            // (0,0)正好是四个细分块共用的角，覆盖窄相位多对接触的岛注册路径。
            Transform::from_xyz(0.0, 1.0, 0.0),
            RigidBody::Dynamic,
            Collider::capsule(0.5, 1.0),
            LockedAxes::ROTATION_LOCKED,
        ))
        .id();
    app.world_mut().entity_mut(batch).remove::<RigidBodyDisabled>();
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(fixed_count(&app), 0, "双暂停时不能偷偷运行FixedMain/物理");
    let paused_position = *app.world().get::<Transform>(actor).unwrap();
    assert_eq!(paused_position.translation, Vec3::new(0.0, 1.0, 0.0));
    for &chunk in &chunks {
        assert_eq!(app.world().get::<ColliderOf>(chunk).unwrap().body, batch);
        assert!(app.world().get::<RigidBody>(chunk).is_none());
        assert_eq!(app.world().get::<MapInstanceOwner>(chunk), Some(&MapInstanceOwner(source)));
    }

    resume(&mut app, "first-resume-16");
    advance(&mut app, 16);
    assert_support(&app, actor, &chunks);
    app.world_mut().resource_mut::<ResumeTrace>().phase = "first-resume-to-120";
    advance(&mut app, 104);
    assert_support(&app, actor, &chunks);

    pause(&mut app);
    app.world_mut().resource_mut::<ResumeTrace>().phase = "replacement-paused";
    let before = fixed_count(&app);
    let old_position = app.world().get::<Position>(actor).unwrap().0;
    // 四个当前承托块也在换代范围内；不只替换远处来回避接触图注销/重新登记。
    let replaced = [18, 19, 26, 27, 28, 35, 36, 37];
    let mut removed = Vec::new();
    for index in replaced {
        let old = chunks[index];
        removed.push(old);
        app.world_mut().entity_mut(old).despawn();
        chunks[index] = spawn_chunk(&mut app, source, index, &shape);
    }
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(fixed_count(&app), before, "换块期间物理时钟须保持暂停");
    assert_eq!(app.world().get::<Position>(actor).unwrap().0, old_position);
    for entity in removed {
        assert!(app.world().get_entity(entity).is_err());
    }
    resume(&mut app, "replacement-resume-16");
    advance(&mut app, 16);
    assert_support(&app, actor, &chunks);
    app.world_mut().resource_mut::<ResumeTrace>().phase = "replacement-resume-to-120";
    advance(&mut app, 104);
    assert_support(&app, actor, &chunks);
}

#[test]
fn raw_trimesh_dual_clock_resume_and_eight_chunk_replacement_keep_real_support() {
    exercise(false);
}

#[test]
fn wrapped_terrain_dual_clock_resume_and_eight_chunk_replacement_keep_real_support() {
    exercise(true);
}

fn assert_single_static_proxy(app: &App, collider: Entity, body: Entity, phase: &str) {
    let trees = app.world().resource::<ColliderTrees>();
    let copies: Vec<_> = trees
        .iter_trees()
        .flat_map(|tree| tree.proxies.iter().map(|(_, proxy)| proxy))
        .filter(|proxy| proxy.collider == collider)
        .collect();
    assert_eq!(copies.len(), 1, "{phase}：同一地形实体不能同时留在多个物理树中：{copies:?}");
    assert_eq!(copies[0].body, Some(body), "{phase}：物理树归属必须跟随正式刚体");
    assert_eq!(app.world().get::<ColliderOf>(collider).unwrap().body, body);
    assert_eq!(app.world().get::<RigidBody>(body), Some(&RigidBody::Static));
    let key = *app.world().get::<ColliderTreeProxyKey>(collider).unwrap();
    assert_eq!(key.tree_type(), ColliderTreeType::Static, "{phase}：地形不能残留Standalone资格");
    assert_eq!(trees.get_proxy(key).unwrap().collider, collider);
}

fn source_in_new_batch(app: &mut App) -> (Entity, Entity) {
    let batch = app
        .world_mut()
        .spawn((SubMapBatch, Transform::IDENTITY, RigidBody::Static))
        .id();
    let source = app.world_mut().spawn((Transform::IDENTITY, ChildOf(batch))).id();
    app.world_mut().entity_mut(source).insert(MapInstanceOwner(source));
    initialize_instance_context(app.world_mut(), source);
    (batch, source)
}

#[test]
fn late_terrain_instance_owner_and_source_reparent_keep_one_static_proxy_per_entity() {
    for wrapped in [false, true] {
        let mut app = app(wrapped);
        let shape = flat_chunk(wrapped);
        let (batch_a, source_a) = source_in_new_batch(&mut app);
        let (batch_b, source_b) = source_in_new_batch(&mut app);
        // 异步模型先作为独立静态碰撞登记，归属在后续采用才补齐；这是生产整合观察器的输入。
        let late = app
            .world_mut()
            .spawn((Transform::IDENTITY, RigidBody::Static, shape.clone()))
            .id();
        for _ in 0..3 {
            app.update();
        }
        assert_single_static_proxy(&app, late, late, "归属尚未采用");

        // 不手写ColliderOf：由ChildOf/Owner触发正式整合移除叶刚体、Avian注销再挂批次。
        app.world_mut().entity_mut(late).insert(ChildOf(source_a));
        app.world_mut().entity_mut(late).insert(MapInstanceOwner(source_a));
        app.world_mut().flush();
        assert!(app.world().get::<RigidBody>(late).is_none());
        assert_single_static_proxy(&app, late, batch_a, "迟到归属已采用");

        // 两个来源使用真实独立批次；相同位置的静态地形须靠刚体类型排除，而非删掉其承托。
        let other = spawn_chunk(&mut app, source_b, 36, &shape);
        app.world_mut().flush();
        assert_single_static_proxy(&app, other, batch_b, "另一来源的重叠地形");
        app.world_mut().entity_mut(late).insert(ChildOf(source_b));
        app.world_mut().entity_mut(late).insert(MapInstanceOwner(source_b));
        app.world_mut().flush();
        assert_single_static_proxy(&app, late, batch_b, "跨来源重挂");

        let actor = app
            .world_mut()
            .spawn((
                Transform::from_xyz(2.0, 1.0, 2.0),
                RigidBody::Dynamic,
                Collider::capsule(0.5, 1.0),
                LockedAxes::ROTATION_LOCKED,
            ))
            .id();
        resume(&mut app, "late-owner-source-reparent");
        advance(&mut app, 16);
        let graph = app.world().resource::<ContactGraph>();
        for pair in graph.active_pairs().iter().chain(graph.sleeping_pairs()) {
            assert_ne!(pair.collider1, pair.collider2, "真实地形不能与自己的Standalone副本产生接触");
            assert!(
                !([late, other].contains(&pair.collider1) && [late, other].contains(&pair.collider2)),
                "跨来源静态地形不得互相进入Composite窄相位",
            );
        }
        let position = app.world().get::<Position>(actor).unwrap().0;
        assert!((0.8..1.2).contains(&position.y), "真实承托仍须保留：{position:?}");
        assert!(graph.contact_pairs_with(actor).any(|pair| {
            pair.is_touching()
                && pair.generates_constraints()
                && ([late, other].contains(&pair.collider1) || [late, other].contains(&pair.collider2))
        }));
        assert_single_static_proxy(&app, late, batch_b, "恢复真实物理后");

        pause(&mut app);
        app.world_mut().entity_mut(late).despawn();
        app.world_mut().flush();
        assert!(
            app.world().resource::<ColliderTrees>().iter_trees().all(|tree| {
                tree.proxies.iter().all(|(_, proxy)| proxy.collider != late)
            }),
            "实体退役须同步注销全部物理资格，不能留副本参与下次恢复",
        );
        assert_single_static_proxy(&app, other, batch_b, "另一来源继续保留");
    }
}
