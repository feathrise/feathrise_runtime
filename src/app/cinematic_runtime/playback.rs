//! 没有自定义导演的工程也可以通过工程规则播放镜头。RPG 在同一接口上提供自己的存档、条件和 Yarn。
use super::super::game_plugins::cinematics::CinematicPresentation;
use bevy::prelude::*;
use feathrise_cinematic::{
    CinematicCatalog, CinematicFrame, CinematicWorldState, FinishAction, FrameData, FrameMode,
    GameDirectorProvided, PlaybackCondition, RepeatPolicy,
};
use feathrise_project_rules::ProjectVariables;
use std::collections::{HashSet, VecDeque};

#[derive(Resource, Default)]
struct Playback {
    queue: VecDeque<(String, Option<String>)>,
    seen: HashSet<String>,
    ticket: u64,
    next_cue: usize,
    waiting: bool,
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
    if !catalog.ready
        || frame
            .active
            .as_ref()
            .is_some_and(|f| f.mode != FrameMode::Game)
    {
        return;
    }
    state
        .queue
        .extend(variables.take_requests_with_instances("cinematic.play"));
    if frame.active.is_none() {
        let Some((id, instance)) = state.queue.pop_front() else {
            return;
        };
        let Some(document) = catalog.get(&id) else {
            warn!("未找到剧情演出 {id}");
            return;
        };
        if document.conditions.iter().any(|condition| match condition {
            PlaybackCondition::Seen { cinematic, seen } => state.seen.contains(cinematic) != *seen,
            _ => {
                warn!("演出 {id} 需要工程提供任务或背包导演");
                true
            }
        }) || (document.repeat == RepeatPolicy::OncePerSave && state.seen.contains(&id))
        {
            return;
        }
        if matches!(document.repeat, RepeatPolicy::OncePerQuest { .. })
            || document.dialogue.iter().any(|c| c.yarn.is_some())
            || document
                .on_finish
                .iter()
                .any(|a| matches!(a, FinishAction::QuestObjective { .. }))
        {
            warn!("演出 {id} 需要工程提供任务或 Yarn 导演");
            return;
        }
        state.ticket += 1;
        state.next_cue = 0;
        state.waiting = false;
        frame.active = Some(FrameData {
            document,
            time: 0.,
            ticket: state.ticket,
            instance,
            mode: FrameMode::Game,
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
            state.seen.insert(active.document.id.clone());
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
