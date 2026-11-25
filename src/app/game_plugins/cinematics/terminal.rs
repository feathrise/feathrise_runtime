//! 终态按结算顺序从后向前采用。相同实体只取最近一次演出的姿态，切槽可撤回到第一个接管前。
use super::{
    FrameIssue, Session,
    bindings::{self, Cast, PlacementIndex},
    clips::Clips,
    poses::Leases,
    prepare_target,
};
use bevy::prelude::*;
use feathrise_cinematic::{CinematicWorldState, FrameData, MotionTrack};
use feathrise_scene_content::ActiveDynamicScene;
use feathrise_scene_core::cinematic::{CinematicActor, ResumeCinematicActor};
use std::{
    collections::{BTreeMap, VecDeque},
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Applied {
    signature: Option<Signature>,
    epoch: u64,
    leases: Leases,
}

#[derive(PartialEq)]
struct Signature {
    ticket: u64,
    document: usize,
    track: usize,
    basis: Mat4,
    player: Option<Entity>,
}

#[derive(Default, Resource)]
pub(super) struct Terminal {
    revisions: Option<[u32; 4]>,
    epoch: u64,
    cursor: Option<(usize, usize)>,
    cleanup: Option<Option<Entity>>,
    waiting: VecDeque<(usize, usize)>,
    applied: BTreeMap<Entity, Applied>,
    clips: Clips,
}

impl Terminal {
    pub fn occupied(&self) -> bool {
        !self.applied.is_empty()
            || self.cursor.is_some()
            || self.cleanup.is_some()
            || !self.waiting.is_empty()
    }
}

pub(super) fn install(app: &mut App) {
    app.add_observer(|event: On<ResumeCinematicActor>, mut commands: Commands| {
        let root = event.entity;
        commands.queue(move |world: &mut World| {
            if world.resource::<Session>().leases.owns_player(root) {
                return;
            }
            world.resource_scope(|world, mut state: Mut<Terminal>| {
                if let Some(applied) = state.applied.get_mut(&root) {
                    applied.leases.release_player(world, root);
                }
            });
        });
    });
}

fn revision<R: Resource>(world: &World) -> u32 {
    world
        .get_resource_ref::<R>()
        .map_or(0, |r| r.last_changed().get())
}

fn track(frame: &FrameData, index: usize) -> Option<(bool, &MotionTrack)> {
    let doc = &frame.document;
    if index < doc.actors.len() {
        Some((true, &doc.actors[index]))
    } else {
        doc.objects
            .get(index - doc.actors.len())
            .map(|t| (false, t))
    }
}

fn binding(
    world: &World,
    frame: &FrameData,
    index: usize,
    actor: bool,
    track: &MotionTrack,
) -> Result<(Entity, Signature), FrameIssue> {
    let scope = bindings::scope(
        world,
        track.instance.as_deref().or(frame.instance.as_deref()),
    )?;
    let id = scope.target(world, &track.target, actor)?;
    // 同一演员实体也会更换骨架；子地图重定位时，仍须按新的源坐标重投终态。
    Ok((
        id,
        Signature {
            ticket: frame.ticket,
            document: Arc::as_ptr(&frame.document) as usize,
            track: index,
            basis: scope.frame.source_to_world,
            player: world
                .get::<CinematicActor>(id)
                .and_then(|actor| actor.player),
        },
    ))
}

pub(super) fn adopt(world: &mut World, frame: &FrameData, session: &mut Leases) {
    world.resource_scope(|world, mut state: Mut<Terminal>| {
        for index in 0..frame.document.actors.len() + frame.document.objects.len() {
            let (actor, track) = track(frame, index).unwrap();
            let Ok((id, signature)) = binding(world, frame, index, actor, track) else {
                continue;
            };
            let applied = state.applied.entry(id).or_default();
            applied.leases.adopt(id, session);
            applied.signature = Some(signature);
            if actor {
                applied.leases.return_actor_movement(world, id);
            }
        }
    });
}

fn apply_track(
    world: &mut World,
    state: &mut Terminal,
    doc: usize,
    index: usize,
    retry: bool,
    pass: u64,
) {
    let frame = world.resource::<CinematicWorldState>().completed[doc].clone();
    let (actor, track) = track(&frame, index).unwrap();
    let (id, sig) = match binding(world, &frame, index, actor, track) {
        Ok(bound) => bound,
        Err(FrameIssue::Waiting) => {
            state.waiting.push_back((doc, index));
            return;
        }
        Err(FrameIssue::Error(_)) => return,
    };
    let epoch = state.epoch;
    let applied = state.applied.entry(id).or_default();
    if !retry && applied.epoch == epoch {
        return;
    }
    applied.epoch = epoch;
    if applied.signature.as_ref() == Some(&sig) {
        return;
    }
    match prepare_target(
        world,
        &frame,
        track.sample(frame.document.duration),
        actor,
        &mut state.clips,
        pass,
    ) {
        Ok(Some(plan)) => {
            applied.leases.apply(world, &plan);
            if actor {
                applied.leases.return_actor_movement(world, id);
            }
            applied.signature = Some(sig);
        }
        Err(FrameIssue::Waiting) => state.waiting.push_back((doc, index)),
        Err(FrameIssue::Error(_)) | Ok(None) => {
            applied.leases.release(world, false);
        }
    }
}

pub(super) fn restore(world: &mut World, pass: u64) {
    let revisions = [
        revision::<CinematicWorldState>(world),
        revision::<Cast>(world),
        revision::<PlacementIndex>(world),
        revision::<ActiveDynamicScene>(world),
    ];
    world.resource_scope(|world, mut state: Mut<Terminal>| {
        if state.revisions != Some(revisions) {
            state.revisions = Some(revisions);
            state.epoch = state.epoch.wrapping_add(1);
            state.cursor = Some((world.resource::<CinematicWorldState>().completed.len(), 0));
            state.cleanup = None;
            state.waiting.clear();
        }
        let start = Instant::now();
        let mut retries = state.waiting.len();
        // 待加载演员独立重试。每次只推进一个轨道或一个待回收目标，长存档的空文档也计入预算。
        while start.elapsed() < Duration::from_micros(450) {
            if let Some((remaining, index)) = state.cursor {
                if remaining == 0 {
                    state.cursor = None;
                    state.cleanup = Some(None);
                    continue;
                }
                let doc = remaining - 1;
                let asset = &world.resource::<CinematicWorldState>().completed[doc].document;
                if index == asset.actors.len() + asset.objects.len() {
                    state.cursor = Some((doc, 0));
                    continue;
                }
                state.cursor = Some((remaining, index + 1));
                apply_track(world, &mut state, doc, index, false, pass);
            } else if let Some(last) = state.cleanup {
                let next = match last {
                    Some(id) => state.applied.range((Excluded(id), Unbounded)).next(),
                    None => state.applied.first_key_value(),
                }
                .map(|(&e, a)| (e, a.epoch != state.epoch));
                let Some((id, stale)) = next else {
                    state.cleanup = None;
                    continue;
                };
                state.cleanup = Some(Some(id));
                if stale {
                    state
                        .applied
                        .remove(&id)
                        .unwrap()
                        .leases
                        .release(world, false);
                    state.clips.forget(id);
                }
            } else if retries > 0
                && let Some((doc, index)) = state.waiting.pop_front()
            {
                retries -= 1;
                apply_track(world, &mut state, doc, index, true, pass);
            } else {
                break;
            }
        }
    });
}
