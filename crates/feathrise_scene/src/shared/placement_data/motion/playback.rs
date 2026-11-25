//! 已采用场景批次的共享时间轴，并按完整实例路径定向处理游戏播放命令。

use avian3d::prelude::{Physics, PhysicsTime};
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_content::{
    SceneId, SceneInstancePath, SceneMotionClip, SceneMotionClips, SceneMotionPlacementTrack, SceneMotionPlayback,
};
use feathrise_scene_core::{
    input::EditorInputState,
    residency::{InstanceResourceTable, SceneInstanceOwner, SceneSourceFrame},
};
use std::{cmp::Reverse, collections::BTreeMap, sync::Arc};

use super::super::{components::PlacementInstanceGate, resources::ScenePlacementRuntimeState};

/// 控制已就绪实例的片段。暂停保留当前姿势，停止回到片段的第一帧。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneMotionAction {
    Play,
    Pause,
    Reverse,
    Stop,
}

/// 每个已采用实例拥有自己的时钟。顶层场景可用 `SceneInstancePath::root` 定位。
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct SceneMotionCommand {
    pub instance: SceneInstancePath,
    pub clip_id: String,
    pub action: SceneMotionAction,
}

/// 单次片段自然到达端点的方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneMotionDirection {
    Forward,
    Backward,
}

/// 单次片段在固定步自然到达对应端点时发布一次，暂停或停止只改变播放状态。
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct SceneMotionFinished {
    pub instance: SceneInstancePath,
    pub clip_id: String,
    pub direction: SceneMotionDirection,
}

/// 工程试走可订阅最后一次实际执行的区域动作；普通游戏无需安装此资源。
#[derive(Resource, Default)]
pub struct SceneMotionTriggerTrace {
    pub last: Option<SceneMotionTriggerHit>,
}

pub struct SceneMotionTriggerHit {
    pub clip_name: String,
    pub entering: bool,
    pub action: SceneMotionAction,
}

#[derive(Clone, Copy)]
struct Playhead {
    priority: u64,
    phase: f64,
    backward: bool,
    playing: bool,
    first_sample: bool,
    discontinuity: bool,
}

impl Playhead {
    fn start(clip: &SceneMotionClip, backward: bool) -> Self {
        Self {
            priority: 0,
            phase: if backward { clip.duration } else { 0.0 },
            backward,
            playing: true,
            first_sample: true,
            discontinuity: false,
        }
    }

    fn advance(&mut self, clip: &SceneMotionClip, delta: f64) -> bool {
        self.discontinuity = std::mem::take(&mut self.first_sample);
        // 首次采用直接命中对应端点，不把初始姿势差变成乘客承托速度。
        if self.discontinuity || !self.playing {
            return false;
        }
        let duration = clip.duration;
        let next = self.phase + if self.backward { -delta } else { delta };
        match clip.playback {
            SceneMotionPlayback::Once => {
                self.phase = next.clamp(0.0, duration);
                self.playing = if self.backward { next > 0.0 } else { next < duration };
            }
            SceneMotionPlayback::Loop if duration > 0.0 => {
                self.discontinuity = next < 0.0 || next >= duration;
                self.phase = next.rem_euclid(duration);
            }
            SceneMotionPlayback::PingPong if duration > 0.0 => self.phase = next.rem_euclid(2.0 * duration),
            _ => self.playing = false,
        }
        clip.playback == SceneMotionPlayback::Once && !self.playing
    }
}

/// 每个活动摆放批次一份状态，休眠的手动片段不进入每步推进集合。
#[derive(Component)]
pub(in crate::shared::placement_data) struct MotionController {
    scene_id: SceneId,
    clips: Arc<SceneMotionClips>,
    gate: PlacementInstanceGate,
    playheads: BTreeMap<usize, Playhead>,
    routing_revision: u64,
    occupied: Vec<(usize, usize)>,
}

impl MotionController {
    pub(in crate::shared::placement_data) fn new(
        scene_id: SceneId,
        clips: Arc<SceneMotionClips>,
        gate: PlacementInstanceGate,
    ) -> Self {
        let playheads = clips.automatic().iter().map(|&id| {
            (id, Playhead::start(&clips.clips()[id], false))
        }).collect();
        Self { scene_id, clips, gate, playheads, routing_revision: 0, occupied: Vec::new() }
    }

    fn apply(&mut self, id: &str, action: SceneMotionAction) {
        let Some(index) = self.clips.clip_index(id) else { return; };
        self.apply_index(index, action);
    }

    fn apply_index(&mut self, index: usize, action: SceneMotionAction) {
        if matches!(action, SceneMotionAction::Pause | SceneMotionAction::Stop) {
            if let Some(playhead) = self.playheads.get_mut(&index) {
                playhead.playing = false;
                if action == SceneMotionAction::Stop {
                    playhead.backward = false;
                    playhead.phase = 0.0;
                    playhead.first_sample = true;
                }
            }
            return;
        }
        let clip = &self.clips.clips()[index];
        let backward = action == SceneMotionAction::Reverse;
        let playhead = self.playheads.entry(index).or_insert_with(|| Playhead::start(clip, backward));
        if !playhead.playing && clip.playback == SceneMotionPlayback::Once
            && if backward { playhead.phase <= 0.0 } else { playhead.phase >= clip.duration }
        {
            *playhead = Playhead::start(clip, backward);
        }
        playhead.playing = true;
        playhead.backward = backward;
        self.routing_revision += 1;
        playhead.priority = self.routing_revision;
    }

    pub(super) fn routing_revision(&self) -> u64 { self.routing_revision }

    fn update_regions(&mut self, hits: &mut Vec<(usize, usize)>, mut trace: Option<&mut SceneMotionTriggerTrace>) {
        use feathrise_scene_content::MotionRegionAction;
        let previous = std::mem::take(&mut self.occupied);
        // 先处理离开，再处理进入；同一门的相邻触发区切换时以新区域的动作收尾。
        for (entering, from, other) in [(false, &previous, &*hits), (true, &*hits, &previous)] {
            for &(clip, region) in from.iter().filter(|id| other.binary_search(id).is_err()) {
                let area = &self.clips.clips()[clip].regions[region];
                let action = match if entering { area.enter } else { area.exit } {
                    MotionRegionAction::None => continue,
                    MotionRegionAction::Play => SceneMotionAction::Play,
                    MotionRegionAction::Reverse => SceneMotionAction::Reverse,
                    MotionRegionAction::Pause => SceneMotionAction::Pause,
                    MotionRegionAction::Stop => SceneMotionAction::Stop,
                };
                self.apply_index(clip, action);
                if let Some(trace) = trace.as_deref_mut() {
                    trace.last = Some(SceneMotionTriggerHit {
                        clip_name: self.clips.clips()[clip].name.clone(), entering, action,
                    });
                }
            }
        }
        self.occupied = std::mem::take(hits);
        *hits = previous;
    }

    pub(super) fn selected_binding(&self, bindings: &[SceneMotionPlacementTrack]) -> Option<usize> {
        // 明确开始/反向命令接管重叠对象。默认自动片段同优先级时沿用最前的 glTF 顺序。
        bindings.iter().enumerate().filter_map(|(index, binding)| {
            self.playheads.get(&binding.clip_index).map(|playhead| (playhead.priority, Reverse(index)))
        }).max().map(|(_, index)| index.0)
    }

    pub(super) fn sample(&self, clip_index: usize) -> Option<(f64, bool)> {
        self.playheads.get(&clip_index).map(|playhead| (playhead.phase, playhead.discontinuity))
    }
}

/// 同一动画资产下的普通摆放修订沿用时钟，热更动画后新批次使用新片段定义。
pub(in crate::shared::placement_data) fn carry(commands: &mut Commands, old: Entity, new: Entity) {
    commands.queue(move |world: &mut World| {
        let same = world.get::<MotionController>(old).zip(world.get::<MotionController>(new))
            .is_some_and(|(old, new)| Arc::ptr_eq(&old.clips, &new.clips));
        if same && let Some(old) = world.entity_mut(old).take::<MotionController>()
            && let Some(mut new) = world.get_mut::<MotionController>(new)
        {
            new.playheads = old.playheads;
            new.occupied = old.occupied;
            new.routing_revision = old.routing_revision;
        }
    });
}

#[derive(SystemParam)]
pub(in crate::shared::placement_data) struct MotionOwners<'w> {
    root: Option<Res<'w, ScenePlacementRuntimeState>>,
    nested: Option<Res<'w, InstanceResourceTable<ScenePlacementRuntimeState>>>,
}

impl MotionOwners<'_> {
    pub(super) fn active_batch(&self, owner: Option<Entity>, scene_id: &SceneId) -> Option<Entity> {
        let state = match owner {
            Some(owner) => self.nested.as_deref()?.values.get(&owner)?,
            None => self.root.as_deref()?,
        };
        (state.active_scene_id.as_ref()? == scene_id).then_some(state.active.as_ref()?.root)
    }
}

pub(in crate::shared::placement_data) fn apply_commands(
    mut commands: MessageReader<SceneMotionCommand>,
    owners: MotionOwners,
    scene: crate::scenes::dynamic_scene::TerrainObjectQueries,
    subscenes: Query<&crate::SubsceneInstance>,
    editor: Option<Res<EditorInputState>>,
    mut controllers: Query<&mut MotionController>,
) {
    // 每个游戏帧收取消息。渲染帧跑得更快时，消息不会因等下一个固定步而过期。
    for command in commands.read() {
        if command.instance.terrain_id.is_some() || editor.as_deref().is_some_and(|input| input.scene_editor_open) {
            continue;
        }
        let (owner, scene_id) = if command.instance.is_root() {
            (None, &command.instance.root_scene_id)
        } else {
            let Some(owner) = scene.active_subscene_at(&command.instance) else { continue; };
            let Ok(instance) = subscenes.get(owner) else { continue; };
            (Some(owner), &instance.source_scene_id)
        };
        let Some(root) = owners.active_batch(owner, scene_id) else { continue; };
        if let Ok(mut controller) = controllers.get_mut(root)
            && controller.gate.is_active()
        {
            controller.apply(&command.clip_id, command.action);
        }
    }
}

pub(in crate::shared::placement_data) fn advance(
    time: Res<Time<Fixed>>,
    physics: Option<Res<Time<Physics>>>,
    editor: Option<Res<EditorInputState>>,
    mut controllers: Query<(&InheritedVisibility, Option<&SceneInstanceOwner>, &mut MotionController)>,
    subscenes: Query<&crate::SubsceneInstance>,
    frames: Query<&SceneSourceFrame>,
    observers: Query<&GlobalTransform, With<crate::SceneRuntimeObserver>>,
    mut hits: Local<Vec<(usize, usize)>>,
    mut finished: MessageWriter<SceneMotionFinished>,
    mut trace: Option<ResMut<SceneMotionTriggerTrace>>,
) {
    let Some(physics) = physics else { return; };
    let delta = time.delta_secs_f64() * physics.relative_speed_f64();
    if physics.is_paused() || editor.as_deref().is_some_and(|input| input.scene_editor_open) || delta <= 0.0 {
        return;
    }
    for (visibility, owner, mut controller) in &mut controllers {
        if !visibility.get() || !controller.gate.is_active() { continue; }
        if controller.clips.has_regions()
            && let Some(frame) = owner.map_or_else(|| Some(SceneSourceFrame::default()), |owner| frames.get(owner.0).ok().copied()) {
            hits.clear();
            for pose in &observers { controller.clips.regions_at(frame.to_local(pose.translation()), &mut hits); }
            // 复用两次查询的命中数组，多个观察者共用同一区域时只提交一次边沿。
            hits.sort_unstable();
            hits.dedup();
            if controller.occupied != *hits { controller.update_regions(&mut hits, trace.as_deref_mut()); }
        }
        let MotionController { scene_id, clips, playheads, .. } = &mut *controller;
        // 同一片段的所有摆放只读以上一次固定步为基准推进的同一状态。
        for (&index, playhead) in playheads {
            let clip = &clips.clips()[index];
            if !playhead.advance(clip, delta) { continue; }
            let instance = if let Some(owner) = owner {
                let Ok(subscene) = subscenes.get(owner.0) else { continue; };
                subscene.path.clone()
            } else {
                SceneInstancePath::root(scene_id.clone())
            };
            let direction = if playhead.backward { SceneMotionDirection::Backward } else { SceneMotionDirection::Forward };
            finished.write(SceneMotionFinished { instance, clip_id: clip.id.clone(), direction });
        }
    }
}
