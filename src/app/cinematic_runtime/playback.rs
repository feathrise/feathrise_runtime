//! 没有自定义导演的工程也可以通过工程规则播放镜头。RPG 在同一接口上提供自己的存档、条件和 Yarn。
use super::super::game_plugins::cinematics::CinematicPresentation;
use bevy::prelude::*;
use feathrise_cinematic::{
    CinematicCatalog, CinematicFrame, CinematicWorldState, FinishAction, FrameData, FrameMode,
    GameDirectorProvided, PlaybackCondition, RepeatPolicy,
};
use feathrise_project_rules::ProjectVariables;
use std::collections::HashSet;

#[derive(Resource, Default)]
struct Playback {
    seen: HashSet<String>,
    observed: Option<(u64, u64, u64)>,
    session: Option<(u64, u64)>,
    scene: Option<String>,
    ticket: u64,
    next_cue: usize,
    waiting: bool,
}

#[cfg(test)]
mod tests {
    use super::super::super::game_plugins::cinematics::CinematicPresentationPlugin;
    use super::*;
    use bevy::{time::TimeUpdateStrategy, transform::TransformPlugin};
    use feathrise_cinematic::{CameraKey, Cinematic, Pose};

    fn fixture(variables: &ProjectVariables, catalog: CinematicCatalog) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin, CinematicPresentationPlugin))
            .insert_resource(variables.clone())
            .insert_resource(catalog)
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(50),
            ));
        install(&mut app);
        app
    }

    #[test]
    fn missing_asset_and_seen_condition_keep_the_request_until_playback_finishes() {
        let variables = ProjectVariables::from_json(
            r#"{"version":1,"rules":[{
            "id":"play","once":true,"when":{"event":"start"},
            "actions":[{"request":"cinematic.play","target":"shot"}]},
            {"id":"prior","when":{"event":"prior"},
            "actions":[{"request":"cinematic.play","target":"prior"}]}]}"#,
        )
        .unwrap();
        let mut app = fixture(&variables, CinematicCatalog::from_documents([]).unwrap());
        variables.event("start", "");
        app.update();
        assert!(app.world().resource::<CinematicFrame>().active.is_none());
        let mut shot = Cinematic::new("shot", "等待前段");
        shot.duration = 0.1;
        shot.conditions.push(PlaybackCondition::Seen {
            cinematic: "prior".into(),
            seen: true,
        });
        let mut prior = Cinematic::new("prior", "先行演出");
        prior.duration = 0.1;
        app.insert_resource(CinematicCatalog::from_documents([shot, prior]).unwrap());
        app.update();
        assert!(app.world().resource::<CinematicFrame>().active.is_none());
        variables.event("prior", "");
        for _ in 0..8 {
            app.update();
            if app
                .world()
                .resource::<CinematicFrame>()
                .active
                .as_ref()
                .is_some_and(|active| active.document.id == "shot")
            {
                break;
            }
        }
        let active = app
            .world()
            .resource::<CinematicFrame>()
            .active
            .as_ref()
            .unwrap();
        assert_eq!(active.document.id, "shot");
        assert!(active.instance.is_none());
        // 接手播放还没有完成收尾，规则快照仍须保留这一份待办。
        let snapshot = serde_json::to_value(variables.snapshot()).unwrap();
        assert_eq!(snapshot["requests"]["cinematic.play"][0], "shot");
        for _ in 0..8 {
            app.update();
        }
        assert!(app.world().resource::<CinematicFrame>().active.is_none());
        assert!(variables.take_requests("cinematic.play").is_empty());
        assert_eq!(
            app.world()
                .resource::<CinematicWorldState>()
                .completed
                .len(),
            2
        );
    }

    #[test]
    fn presentation_failure_retains_work_and_restore_cannot_confirm_with_an_old_frame() {
        let variables = ProjectVariables::from_json(
            r#"{"version":1,"rules":[{
            "id":"play","once":true,"when":{"event":"start"},
            "actions":[{"request":"cinematic.play","target":"shot"}]}]}"#,
        )
        .unwrap();
        let mut shot = Cinematic::new("shot", "缺少相机");
        shot.duration = 0.1;
        shot.camera.push(CameraKey {
            time: 0.,
            pose: Pose::default(),
            fov_y_degrees: 45.,
            smooth: false,
        });
        let mut app = fixture(
            &variables,
            CinematicCatalog::from_documents([shot]).unwrap(),
        );
        variables.event("start", "");
        for _ in 0..5 {
            app.update();
        }
        assert!(app.world().resource::<CinematicFrame>().active.is_none());
        let snapshot = variables.snapshot();
        let saved = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            saved["requests"]["cinematic.play"],
            serde_json::json!(["shot"])
        );
        let ticket = app.world().resource::<Playback>().ticket;
        app.world_mut().spawn((
            Camera3d::default(),
            feathrise_scene_core::view::PrimarySceneView,
            Transform::default(),
        ));
        variables.event("camera.ready", "");
        app.update();
        assert!(app.world().resource::<CinematicFrame>().active.is_some());
        variables.restore(snapshot);
        app.update();
        let active = app
            .world()
            .resource::<CinematicFrame>()
            .active
            .as_ref()
            .unwrap();
        assert!(active.ticket > ticket + 1);
        assert_eq!(active.time, 0.);
        assert_eq!(
            serde_json::to_value(variables.snapshot()).unwrap()["requests"]["cinematic.play"],
            serde_json::json!(["shot"])
        );
        for _ in 0..8 {
            app.update();
        }
        assert!(variables.take_requests("cinematic.play").is_empty());
        assert_eq!(
            app.world()
                .resource::<CinematicWorldState>()
                .completed
                .len(),
            1
        );
    }
    #[test]
    fn foreign_requests_wait_while_same_scene_transfer_restarts_the_current_request() {
        let variables = ProjectVariables::from_json(
            r#"{"version":1,"rules":[{"id":"play","when":{"event":"start"},
            "actions":[{"request":"cinematic.play","target":"shot"}]}]}"#,
        )
        .unwrap();
        variables.set_active_scene(Some("town"));
        let mut shot = Cinematic::new("shot", "当前场景");
        shot.duration = 0.1;
        let mut app = fixture(
            &variables,
            CinematicCatalog::from_documents([shot]).unwrap(),
        );
        variables.event_in_instance("start", "", "other/a");
        variables.event_in_instance("start", "", "town/a");
        app.update();
        let frame = app
            .world()
            .resource::<CinematicFrame>()
            .active
            .as_ref()
            .unwrap();
        assert_eq!(frame.instance.as_deref(), Some("town/a"));
        let ticket = frame.ticket;
        variables.set_active_scene(None);
        app.update();
        assert!(app.world().resource::<CinematicFrame>().active.is_none());
        variables.set_active_scene(Some("town"));
        app.update();
        let frame = app
            .world()
            .resource::<CinematicFrame>()
            .active
            .as_ref()
            .unwrap();
        assert!(frame.ticket > ticket);
        assert_eq!(frame.time, 0.);
        for _ in 0..8 {
            app.update();
        }
        assert_eq!(
            variables.take_requests_with_instances("cinematic.play"),
            [("shot".into(), Some("other/a".into()))]
        );
        assert_eq!(
            app.world()
                .resource::<CinematicWorldState>()
                .completed
                .len(),
            1
        );
    }
}
#[derive(Component)]
struct Subtitle;
pub(super) fn install(app: &mut App) {
    app.init_resource::<Playback>().add_systems(
        Update,
        tick.run_if(not(resource_exists::<GameDirectorProvided>)),
    );
}
fn tick(
    mut commands: Commands,
    time: Res<Time>,
    inputs: Option<Res<ButtonInput<KeyCode>>>,
    catalog: Res<CinematicCatalog>,
    variables: Option<Res<ProjectVariables>>,
    presentation: Res<CinematicPresentation>,
    mut state: ResMut<Playback>,
    mut frame: ResMut<CinematicFrame>,
    mut completed: ResMut<CinematicWorldState>,
    mut subtitles: Query<(Entity, &mut Text), With<Subtitle>>,
) {
    let Some(variables) = variables else { return };
    let session = variables.session_revision();
    if state.session != Some(session) {
        if let Some(previous) = state.session {
            // 读档与换图先撤回旧镜头；相同目标和实例也不能用旧播放去确认新一代待办。
            if previous.0 != session.0 {
                state.seen.clear();
                completed.completed.clear();
            }
            if frame
                .active
                .as_ref()
                .is_some_and(|f| f.mode == FrameMode::Game)
            {
                frame.active = None;
                for (entity, _) in &mut subtitles {
                    commands.entity(entity).despawn();
                }
            }
        }
        state.session = Some(session);
        state.observed = None;
        state.scene = variables.active_scene();
    }
    // 自建世界无需场景 ID；曾进入正式场景后，加载间隙等待下一次活动范围。
    if (session.1 != 0 && state.scene.is_none())
        || !catalog.ready
        || frame
            .active
            .as_ref()
            .is_some_and(|f| f.mode != FrameMode::Game)
    {
        return;
    }
    if frame.active.is_none() {
        let revision = (
            variables.requests_revision(),
            variables.values_revision(),
            variables.events_revision(),
        );
        if state.observed == Some(revision) && !catalog.is_changed() {
            return;
        }
        state.observed = Some(revision);
        // 默认导演没有独立存档队列，直到收尾成功才确认原请求，缺资产、条件不符和播放失败均可恢复。
        variables.handle_requests_with_instances("cinematic.play", |id, instance| {
            if frame.active.is_some() {
                return false;
            }
            // 外图待办保留到返回原图，不能抢占当前场景后续已就绪的演出。
            if let (Some(scene), Some(instance)) = (&state.scene, instance)
                && instance.split('/').next() != Some(scene.as_str())
            {
                return false;
            }
            let Some(document) = catalog.get(id) else {
                warn!("未找到剧情演出 {id}，已保留待播请求");
                return false;
            };
            if document.repeat == RepeatPolicy::OncePerSave && state.seen.contains(id) {
                return true;
            }
            if matches!(document.repeat, RepeatPolicy::OncePerQuest { .. })
                || document.dialogue.iter().any(|c| c.yarn.is_some())
                || document
                    .on_finish
                    .iter()
                    .any(|a| matches!(a, FinishAction::QuestObjective { .. }))
                || document
                    .conditions
                    .iter()
                    .any(|c| !matches!(c, PlaybackCondition::Seen { .. }))
            {
                warn!("演出 {id} 需要工程提供任务、背包或 Yarn 导演，已保留待播请求");
                return false;
            }
            if document.conditions.iter().any(|condition| match condition {
                PlaybackCondition::Seen { cinematic, seen } => {
                    state.seen.contains(cinematic) != *seen
                }
                _ => unreachable!(),
            }) {
                return false;
            }
            state.ticket += 1;
            state.next_cue = 0;
            state.waiting = false;
            frame.active = Some(FrameData {
                document,
                time: 0.,
                ticket: state.ticket,
                instance: instance.map(str::to_owned),
                mode: FrameMode::Game,
            });
            false
        });
        return;
    }
    let active = frame.active.as_mut().unwrap();
    if let Some(error) = presentation.error(active.ticket, active.time) {
        warn!("演出 {}：{error}", active.document.id);
        frame.active = None;
        for (e, _) in &mut subtitles {
            commands.entity(e).despawn()
        }
        return;
    }
    if !presentation.ready(active.ticket, active.time) {
        return;
    }
    let pressed = |key| inputs.as_ref().is_some_and(|i| i.just_pressed(key));
    if state.waiting
        && !pressed(KeyCode::Space)
        && !pressed(KeyCode::Enter)
        && !pressed(KeyCode::Escape)
    {
        return;
    }
    state.waiting = false;
    if active.time >= active.document.duration {
        let active = frame.active.take().unwrap();
        let changes = active
            .document
            .on_finish
            .iter()
            .filter_map(|a| match a {
                FinishAction::SetVariable { key, value } => {
                    Some(serde_json::from_value(value.clone()).map(|v| (key.clone(), v)))
                }
                _ => None,
            })
            .collect::<Result<_, _>>();
        if let Err(error) =
            changes
                .map_err(|e| e.to_string())
                .and_then(|values| match &active.instance {
                    Some(instance) => variables.set_batch_in_instance(values, instance),
                    None => variables.set_batch(values),
                })
        {
            warn!("演出 {} 结算失败：{error}", active.document.id);
        } else {
            // 同名请求可以有多份；本次成功只确认与在播实例匹配的第一份。
            let mut confirmed = false;
            variables.handle_requests_with_instances("cinematic.play", |id, instance| {
                let matches = !confirmed
                    && variables.session_revision() == session
                    && id == active.document.id
                    && instance == active.instance.as_deref();
                confirmed |= matches;
                matches
            });
            state.seen.insert(active.document.id.clone());
            state.observed = None;
            if let Some(instance) = &active.instance {
                variables.event_in_instance("cinematic.finished", &active.document.id, instance);
            } else {
                variables.event("cinematic.finished", &active.document.id);
            }
            completed.record_completion(active);
        }
        for (e, _) in &mut subtitles {
            commands.entity(e).despawn()
        }
        return;
    }
    let skip = pressed(KeyCode::Escape);
    let mut next = if skip {
        active.document.duration
    } else {
        (active.time + time.delta_secs()).min(active.document.duration)
    };
    let mut held = None;
    while let Some(cue) = active.document.dialogue.get(state.next_cue) {
        if cue.time > next {
            break;
        }
        state.next_cue += 1;
        if cue.wait && !skip {
            next = cue.time;
            state.waiting = true;
            held = cue.text.as_deref();
            break;
        }
    }
    active.time = next;
    let text = held.map(str::to_owned).unwrap_or_else(|| {
        active
            .document
            .dialogue_at(next)
            .filter_map(|c| c.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    });
    if let Some((_, mut old)) = subtitles.iter_mut().next() {
        if old.0 != text {
            old.0 = text
        }
    } else if !text.is_empty() {
        commands.spawn((
            Subtitle,
            Text::new(text),
            TextLayout::new(Justify::Center, bevy::text::LineBreak::WordBoundary),
            Node {
                position_type: PositionType::Absolute,
                bottom: percent(7.),
                width: percent(100.),
                ..default()
            },
        ));
    }
}
