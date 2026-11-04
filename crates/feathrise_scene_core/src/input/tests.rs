//! 直接运行共享输入条件，验证暂停原因、加载和驻留切换不能相互提前放行。
use super::*;
use bevy::ecs::system::RunSystemOnce;

fn gates(world: &mut World) -> (bool, bool) {
    (
        world.run_system_once(in_game_mode).unwrap(),
        world.run_system_once(world_active).unwrap(),
    )
}

#[test]
fn application_input_and_simulation_permissions_are_independent() {
    let mut world = World::new();
    world.init_resource::<InputGateState>();
    assert_eq!(gates(&mut world), (true, true));
    world.resource_mut::<InputGateState>().input_allowed = false;
    assert_eq!(gates(&mut world), (false, true));
    world.resource_mut::<InputGateState>().world_active = false;
    assert_eq!(gates(&mut world), (false, false));
}

#[test]
fn missing_permission_resource_is_not_treated_as_an_implicit_allow() {
    let mut world = World::new();
    // 与旧GlobalResource/UiBlocksGameInput的必需资源语义相同：缺少装配时不能静默放行。
    assert!(world.run_system_once(in_game_mode).is_err());
    assert!(world.run_system_once(world_active).is_err());
}

#[test]
fn editor_and_collision_transactions_cannot_release_each_other() {
    let mut world = World::new();
    world.init_resource::<InputGateState>();
    world.insert_resource(EditorInputState { map_editor_open: true, ..default() });
    world.insert_resource(WorldPauseBlockers { map_collision_reload: true });
    assert_eq!(gates(&mut world), (false, false));
    world.resource_mut::<EditorInputState>().map_editor_open = false;
    assert_eq!(gates(&mut world), (false, false));
    world.resource_mut::<WorldPauseBlockers>().map_collision_reload = false;
    assert_eq!(gates(&mut world), (true, true));
}

#[test]
fn loading_and_residency_require_actual_completion_after_permissions_reopen() {
    let mut world = World::new();
    world.init_resource::<InputGateState>();
    world.insert_resource(State::new(ScenePhase::Active));
    world.insert_resource(SceneStartupGate::waiting_for_collision(ScenePhase::Active));
    assert_eq!(gates(&mut world), (false, false));
    world.resource_mut::<SceneStartupGate>().ready = true;
    world.insert_resource(LoadingProgress::default());
    assert_eq!(gates(&mut world), (false, false));
    world.remove_resource::<LoadingProgress>();
    world.insert_resource(crate::state::SceneResidencyActivity { switching: true, ..default() });
    assert_eq!(gates(&mut world), (false, false));
    world.resource_mut::<crate::state::SceneResidencyActivity>().switching = false;
    assert_eq!(gates(&mut world), (true, true));
}
