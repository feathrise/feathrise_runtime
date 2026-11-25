//! 将同一演出时间采样应用于真实主相机、演员骨架和摆放实例。播放、拖帧和导出共用此路径。
pub(crate) mod bindings;
mod clips;
mod free_camera;
mod poses;
mod subtitles;
mod terminal;
#[cfg(test)]
mod tests;
#[cfg(all(test, target_os = "windows"))]
mod window_tests;

use bevy::{app::AnimationSystems, prelude::*};
use feathrise_cinematic::{
    CinematicFrame, CinematicWorldState, FrameData, FrameMode, Pose, TargetSample,
};
pub use feathrise_scene_core::cinematic::{CinematicActor, CinematicControl};
use feathrise_scene_core::{input::GamePauseSystemSet, view::PrimarySceneView};
use poses::{Leases, Plan};
use std::collections::HashSet;

/// 捕获方在此集合后读取回执；此时骨骼求值和全局变换传播均已完成。
#[derive(SystemSet, Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct CinematicPresentationSet;

#[derive(Resource, Default)]
pub struct CinematicPresentation {
    pub camera: Option<Entity>,
    presented: Option<(u64, u32)>,
    failure: Option<(u64, u32, String)>,
}
impl CinematicPresentation {
    pub fn ready(&self, ticket: u64, time: f32) -> bool {
        self.presented == Some((ticket, time.to_bits()))
    }
    pub fn error(&self, ticket: u64, time: f32) -> Option<&str> {
        self.failure
            .as_ref()
            .filter(|(t, f, _)| (*t, *f) == (ticket, time.to_bits()))
            .map(|(_, _, s)| s.as_str())
    }
}

pub struct CinematicPresentationPlugin;
impl Plugin for CinematicPresentationPlugin {
    fn build(&self, app: &mut App) {
        bindings::install(app);
        terminal::install(app);
        free_camera::install(app);
        app.init_resource::<CinematicFrame>()
            .init_resource::<CinematicWorldState>()
            .init_resource::<CinematicPresentation>()
            .init_resource::<CinematicControl>()
            .init_resource::<Session>()
            .init_resource::<terminal::Terminal>()
            .add_systems(PreUpdate, synchronize_control)
            .add_systems(
                Update,
                synchronize_control.before(GamePauseSystemSet::PublishGateFacts),
            )
            .add_systems(
                PostUpdate,
                (present, subtitles::update)
                    .chain()
                    .before(AnimationSystems)
                    .before(TransformSystems::Propagate),
            )
            .configure_sets(
                PostUpdate,
                CinematicPresentationSet
                    .after(AnimationSystems)
                    .after(TransformSystems::Propagate),
            );
    }
}

fn requested_control(f: &FrameData) -> CinematicControl {
    CinematicControl {
        camera: f.document.control_camera || f.mode != FrameMode::Game,
        player: f.document.control_player || f.mode != FrameMode::Game,
    }
}

fn synchronize_control(frame: Res<CinematicFrame>, mut control: ResMut<CinematicControl>) {
    let next = frame
        .active
        .as_ref()
        .map_or_else(CinematicControl::default, requested_control);
    control.set_if_neq(next);
}

#[derive(Default, Resource)]
struct Session {
    pass: u64,
    ticket: Option<u64>,
    final_frame: Option<FrameData>,
    leases: Leases,
    clips: clips::Clips,
    camera: Option<(Entity, Transform, Projection)>,
}

enum FrameIssue {
    Waiting,
    Error(String),
}
impl From<String> for FrameIssue {
    fn from(value: String) -> Self {
        Self::Error(value)
    }
}

fn pose_matrix(pose: Pose) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::from_array(pose.scale),
        Quat::from_array(pose.rotation).normalize(),
        Vec3::from_array(pose.translation),
    )
}

fn prepare_target(
    world: &mut World,
    frame: &FrameData,
    target: TargetSample<'_>,
    actor: bool,
    clips: &mut clips::Clips,
    pass: u64,
) -> Result<Option<Plan>, FrameIssue> {
    if target.pose.is_none() && target.clip.is_none() {
        return Ok(None);
    }
    let scope = bindings::scope(world, target.instance.or(frame.instance.as_deref()))?;
    let entity = scope.target(world, target.target, actor)?;
    if world.get::<Transform>(entity).is_none() {
        return Err(format!("演出目标 {} 缺少变换", target.target).into());
    }
    let mut pose = target
        .pose
        .map(|p| scope.frame.source_to_world * pose_matrix(p));
    let mut shot_clip = None;
    if let Some(clip) = target.clip {
        if actor {
            shot_clip = Some(clips.prepare(world, entity, clip.name, clip.time, pass)?);
        } else {
            let (origin, sampled) =
                feathrise_scene::sample_placement_clip(world, entity, clip.name, clip.time)?;
            pose = Some(match pose {
                Some(authored) => authored * origin.to_matrix().inverse() * sampled.to_matrix(),
                None => poses::local_to_world(world, entity, sampled),
            });
        }
    }
    Ok(Some(Plan {
        entity,
        pose,
        clip: shot_clip,
    }))
}

fn restore_camera(world: &mut World, session: &mut Session) {
    if let Some((id, pose, projection)) = session.camera.take() {
        let pose = free_camera::gameplay_pose(world, id).unwrap_or(pose);
        if let Ok(mut camera) = world.get_entity_mut(id) {
            camera.insert((pose, projection));
        }
    }
}

fn apply(
    world: &mut World,
    frame: &FrameData,
    session: &mut Session,
) -> Result<Option<Entity>, FrameIssue> {
    // 在 Update 末尾抵达的镜头须等控制发布，确保首帧已采用角色近景材质和游戏输入门。
    if *world.resource::<CinematicControl>() != requested_control(frame) {
        return Err(FrameIssue::Waiting);
    }
    let sample = frame.sample();
    let mut plans = Vec::with_capacity(sample.actors.len() + sample.objects.len());
    for (actor, tracks) in [(true, sample.actors), (false, sample.objects)] {
        for target in tracks {
            if let Some(plan) = prepare_target(
                world,
                frame,
                target,
                actor,
                &mut session.clips,
                session.pass,
            )? {
                plans.push(plan);
            }
        }
    }
    // 预校验完整帧之后才动场景；缺人、缺片段或异步资源未到时不会露出只改了一半的镜头。
    let view = world
        .query_filtered::<Entity, (With<PrimarySceneView>, With<Camera>)>()
        .single(world)
        .ok();
    let take_camera = frame.document.control_camera || frame.mode != FrameMode::Game;
    if let Some(shot) = sample.camera.filter(|_| take_camera) {
        let camera = view.ok_or_else(|| FrameIssue::Error("演出需要唯一的主场景相机".into()))?;
        // 独立工程可自己生成世界和主相机。无实例的镜头以世界为坐标系，有地址时必须解析具体实例。
        let basis = if let Some(path) = frame.instance.as_deref() {
            bindings::scope(world, Some(path))?.frame.source_to_world
        } else {
            Mat4::IDENTITY
        };
        let projection = world
            .get::<Projection>(camera)
            .ok_or_else(|| FrameIssue::Error("主相机缺少投影".into()))?;
        if session.camera.as_ref().is_none_or(|(e, _, _)| *e != camera) {
            let original = (*world.get::<Transform>(camera).unwrap(), projection.clone());
            restore_camera(world, session);
            session.camera = Some((camera, original.0, original.1));
        }
        poses::set_world_pose(world, camera, basis * pose_matrix(shot.pose));
        let mut projection = world.get_mut::<Projection>(camera).unwrap();
        if !matches!(*projection, Projection::Perspective(_)) {
            *projection = Projection::Perspective(PerspectiveProjection::default());
        }
        if let Projection::Perspective(projection) = &mut *projection {
            projection.fov = shot.fov_y_degrees.to_radians();
        }
    } else {
        restore_camera(world, session);
    }
    let current_poses: HashSet<_> = plans
        .iter()
        .filter(|p| p.pose.is_some())
        .map(|p| p.entity)
        .collect();
    let current_players: HashSet<_> = plans
        .iter()
        .filter(|p| p.clip.is_some())
        .map(|p| p.entity)
        .collect();
    session
        .leases
        .remove_missing(world, &current_poses, &current_players);
    for plan in &plans {
        session.leases.apply(world, plan);
    }
    Ok(view)
}

fn present(world: &mut World) {
    let frame = world.resource::<CinematicFrame>().active.clone();
    let ticket = frame.as_ref().map(|f| f.ticket);
    // 空工程常态只检查资源，无需查询相机、角色、资产或地图。
    if ticket.is_none()
        && world.resource::<Session>().ticket.is_none()
        && world.resource::<CinematicWorldState>().completed.is_empty()
        && !world.resource::<terminal::Terminal>().occupied()
    {
        return;
    }
    world.resource_scope(|world, mut session: Mut<Session>| {
        session.pass = session.pass.wrapping_add(1);
        if session.ticket != ticket {
            if let Some(final_frame) = session.final_frame.take()
                && world
                    .resource::<CinematicWorldState>()
                    .completed
                    .last()
                    .is_some_and(|record| {
                        record.ticket == final_frame.ticket
                            && record.instance == final_frame.instance
                            && std::sync::Arc::ptr_eq(&record.document, &final_frame.document)
                    })
            {
                terminal::adopt(world, &final_frame, &mut session.leases);
            }
            session.leases.release(world, false);
            restore_camera(world, &mut session);
            session.clips = clips::Clips::default();
            session.ticket = ticket;
            *world.resource_mut::<CinematicPresentation>() = CinematicPresentation::default();
        }
        if let Some(frame) = frame {
            let result = apply(world, &frame, &mut session);
            let mut receipt = world.resource_mut::<CinematicPresentation>();
            receipt.presented = None;
            receipt.failure = None;
            let time = frame.time.to_bits();
            match result {
                Ok(camera) => {
                    receipt.presented = Some((frame.ticket, time));
                    receipt.camera = camera;
                    session.final_frame = (frame.mode == FrameMode::Game
                        && frame.time >= frame.document.duration)
                        .then_some(frame);
                }
                Err(FrameIssue::Waiting) => {}
                Err(FrameIssue::Error(message)) => {
                    receipt.failure = Some((frame.ticket, time, message))
                }
            }
        } else {
            terminal::restore(world, session.pass);
        }
    });
}
