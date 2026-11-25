//! 游戏侧只提供真实玩家的位置。地点的边沿、场景身份及读档基线由工程规则负责。

use bevy::prelude::*;
use feathrise_project_rules::{LocationTracker, ProjectVariables};
use feathrise_scene::SceneRuntimeObserver;
use feathrise_scene_content::ActiveDynamicScene;
use feathrise_scene_core::input::EditorInputState;

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
