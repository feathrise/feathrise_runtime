//! 同步正式场景和玩家位置，规则库负责变量覆盖、地点边沿及读档基线。

use bevy::prelude::*;
use feathrise_project_rules::{LocationTracker, ProjectVariables};
use feathrise_scene::SceneRuntimeObserver;
use feathrise_scene_content::ActiveDynamicScene;
use feathrise_scene_core::{
    input::EditorInputState,
    state::{ScenePhase, SceneResidencyActivity},
};

pub(super) fn sync_scene_scope(
    variables: Option<Res<ProjectVariables>>,
    scene: Res<ActiveDynamicScene>,
    phase: Res<State<ScenePhase>>,
    residency: Res<SceneResidencyActivity>,
) {
    let Some(variables) = variables else {
        return;
    };
    if !(variables.is_changed()
        || scene.is_changed()
        || phase.is_changed()
        || residency.is_changed())
    {
        return;
    }
    // 正式场景的状态转换与驻留恢复已经应用，固定步、对话和位置检测可以读同一场景。
    let active = *phase.get() == ScenePhase::Active && !residency.switching;
    let scene = scene
        .scene_id
        .as_ref()
        .filter(|_| active)
        .map(|id| id.as_str());
    variables.set_active_scene(scene);
}

pub(super) fn configured(variables: Option<Res<ProjectVariables>>) -> bool {
    variables.is_some_and(|vars| vars.has_locations())
}

pub(super) fn observe(
    variables: Res<ProjectVariables>,
    scene: Res<ActiveDynamicScene>,
    editor: Option<Res<EditorInputState>>,
    player: Query<(Entity, &GlobalTransform), With<SceneRuntimeObserver>>,
    mut tracker: Local<LocationTracker>,
) {
    if editor.is_some_and(|input| input.scene_editor_open) {
        tracker.clear();
        return;
    }
    if let (Some(scene), Ok((observer, pose))) = (scene.scene_id.as_ref(), player.single()) {
        tracker.observe(
            &variables,
            scene.as_str(),
            observer.to_bits(),
            pose.translation().to_array(),
        );
    } else {
        tracker.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use feathrise_scene_content::SceneId;

    #[test]
    fn gameplay_uses_the_current_scene_and_drops_it_during_a_transition() {
        let variables = ProjectVariables::from_json(
            r#"{
            "version":1,"variables":{"a":{"default":1}},"scenes":{
                "MAP-town":{"variables":{"a":{"default":2}}},
                "MAP-road":{"variables":{"a":{"default":3}}}
            }
        }"#,
        )
        .unwrap();
        let mut app = App::new();
        app.insert_resource(variables.clone())
            .insert_resource(State::new(ScenePhase::Active))
            .insert_resource(ActiveDynamicScene {
                scene_id: Some(SceneId::parse("MAP-town").unwrap()),
            })
            .init_resource::<SceneResidencyActivity>()
            .add_systems(
                RunFixedMainLoop,
                sync_scene_scope.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
            );
        app.update();
        assert_eq!(variables.active_scene().as_deref(), Some("MAP-town"));
        let revision = variables.values_revision();
        app.update();
        assert_eq!(variables.values_revision(), revision);
        app.world_mut()
            .resource_mut::<SceneResidencyActivity>()
            .switching = true;
        app.world_mut()
            .resource_mut::<ActiveDynamicScene>()
            .scene_id = Some(SceneId::parse("MAP-road").unwrap());
        app.update();
        assert_eq!(variables.active_scene(), None);
        app.world_mut()
            .resource_mut::<SceneResidencyActivity>()
            .switching = false;
        app.update();
        assert_eq!(variables.active_scene().as_deref(), Some("MAP-road"));
        app.insert_resource(State::new(ScenePhase::Idle));
        app.update();
        assert_eq!(variables.active_scene(), None);
    }

    #[test]
    fn only_player_movement_in_the_bound_scene_emits_a_location_event() {
        let variables = ProjectVariables::from_json(
            r#"{
            "version":1,"locations":{"gate":{"scene":"MAP-town","min":[0,0,0],"max":[2,2,2]}},
            "rules":[{"id":"arrived","when":{"event":"location.entered","target":"gate"},
                "actions":[{"request":"notice","target":"gate"}]}]
        }"#,
        )
        .unwrap();
        let mut app = App::new();
        app.insert_resource(variables.clone())
            .insert_resource(ActiveDynamicScene {
                scene_id: Some(SceneId::parse("MAP-town").unwrap()),
            })
            .add_systems(PostUpdate, observe);
        app.world_mut()
            .spawn(GlobalTransform::from_translation(Vec3::ONE));
        let player = app
            .world_mut()
            .spawn((
                SceneRuntimeObserver,
                GlobalTransform::from_translation(Vec3::NEG_X),
            ))
            .id();
        app.update();
        assert!(variables.take_requests("notice").is_empty());
        app.world_mut()
            .entity_mut(player)
            .insert(GlobalTransform::from_translation(Vec3::ONE));
        app.update();
        app.update();
        assert_eq!(variables.take_requests("notice"), ["gate"]);
    }
}
