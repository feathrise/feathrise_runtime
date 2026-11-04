//! 验证纯地图、显式应用领域与子地图完成门各自提供真实证据，不互相冒领。
use super::*;
use feathrise_scene_core::state::{ScenePhase, LoadingProgress, LoadingStage, SceneStartupGate};

#[derive(Resource)]
struct Proof { scene: ScenePhase, complete: bool, error: Option<String> }
fn hooks() -> SceneApplicationHooks {
    SceneApplicationHooks {
        startup: |world, gate| world.get_resource::<Proof>()
            .filter(|proof| proof.scene == gate.target)
            .map_or(SceneApplicationStartupStatus { completed: 0, total: 1, error: None },
                |proof| SceneApplicationStartupStatus { completed: usize::from(proof.complete), total: 1, error: proof.error.clone() }),
        configure_instance: |_| {}, instance_ready: |_, _| Ok(false),
        publish_instance: |_, _, _| {}, retire_instance: |_, _, _| None,
        reset_instances: |_, _| {}, expect_instances: |_, _| {},
    }
}
fn app() -> App {
    let mut app = App::new();
    app.insert_resource(State::new(ScenePhase::Active))
        .insert_resource(SceneStartupGate::waiting_for_collision(ScenePhase::Active))
        .init_resource::<LoadingProgress>()
        .add_systems(Update, crate::shared::scene_runtime::complete_scene_startup);
    app
}
#[test]
fn map_without_application_waits_for_geometry_then_completes_without_actor_entities() {
    let mut app = app();
    app.update();
    assert!(!app.world().resource::<SceneStartupGate>().ready);
    // 首次调度会登记Bevy自身的系统元数据实体；完成场景门不能额外创建应用实体。
    let entities_before_completion = app.world().entities().len();
    app.world_mut().resource_mut::<SceneStartupGate>().collision_ready = true;
    app.update();
    assert!(app.world().resource::<SceneStartupGate>().ready);
    assert_eq!(app.world().entities().len(), entities_before_completion);
}
#[test]
fn registered_domain_rejects_missing_or_other_scene_proof_and_recovers_after_failure() {
    let mut app = app();
    app.insert_resource(hooks());
    app.world_mut().resource_mut::<SceneStartupGate>().collision_ready = true;
    app.update();
    assert!(!app.world().resource::<SceneStartupGate>().ready);
    app.insert_resource(Proof { scene: ScenePhase::Idle, complete: true, error: None });
    app.update();
    assert!(!app.world().resource::<SceneStartupGate>().ready);
    app.insert_resource(Proof { scene: ScenePhase::Active, complete: false, error: Some("领域资源损坏".into()) });
    app.update();
    assert_eq!(app.world().resource::<LoadingProgress>().stage, LoadingStage::Failed);
    app.insert_resource(Proof { scene: ScenePhase::Active, complete: true, error: None });
    app.update();
    assert!(app.world().resource::<SceneStartupGate>().ready);
    assert!(app.world().resource::<SceneStartupGate>().application_error.is_none());
}
#[test]
fn application_success_cannot_override_pending_or_failed_sub_map() {
    let mut app = app();
    app.insert_resource(hooks())
        .insert_resource(Proof { scene: ScenePhase::Active, complete: true, error: None });
    {
        let mut gate = app.world_mut().resource_mut::<SceneStartupGate>();
        gate.collision_ready = true; gate.sub_maps_ready = false;
    }
    app.update();
    assert!(!app.world().resource::<SceneStartupGate>().ready);
    app.world_mut().resource_mut::<SceneStartupGate>().sub_maps_error = Some("子地图资源损坏".into());
    app.update();
    assert_eq!(app.world().resource::<LoadingProgress>().stage, LoadingStage::Failed);
    {
        let mut gate = app.world_mut().resource_mut::<SceneStartupGate>();
        gate.sub_maps_ready = true; gate.sub_maps_error = None;
    }
    app.update();
    assert!(app.world().resource::<SceneStartupGate>().ready);
}
