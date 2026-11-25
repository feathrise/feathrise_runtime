//! 空模板的自由相机同时代表观察点。原生输入始终在玩法姿态上计算，演出镜头只写渲染时的姿态。
use super::{CinematicControl, PrimarySceneView};
use bevy::{
    app::RunFixedMainLoop,
    camera_controller::free_camera::{
        FreeCameraState, rotate_freecam_to, run_freecamera_controller,
    },
    prelude::*,
};

#[derive(Default, Resource)]
struct FreeRig(Option<Controller>);

struct Controller {
    entity: Entity,
    pose: Transform,
    enabled: bool,
    yaw: f32,
    pitch: f32,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<FreeRig>()
        .add_systems(
            RunFixedMainLoop,
            before
                .before(run_freecamera_controller)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .run_if(occupied),
        )
        .add_systems(
            RunFixedMainLoop,
            after
                .after(rotate_freecam_to)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .run_if(occupied),
        )
        .add_systems(Last, release.run_if(occupied));
}

fn occupied(control: Res<CinematicControl>, rig: Res<FreeRig>) -> bool {
    control.camera || control.player || rig.0.is_some()
}

pub(super) fn gameplay_pose(world: &World, id: Entity) -> Option<Transform> {
    world
        .get_resource::<FreeRig>()?
        .0
        .as_ref()
        .filter(|s| s.entity == id)
        .map(|s| s.pose)
}

fn before(
    control: Res<CinematicControl>,
    session: Res<super::Session>,
    mut rig: ResMut<FreeRig>,
    mut query: Query<(Entity, &mut Transform, &mut FreeCameraState), With<PrimarySceneView>>,
) {
    let Ok((entity, mut pose, mut state)) = query.single_mut() else {
        rig.0 = None;
        return;
    };
    // 导演可能在 Update 才发第一帧：此时屏幕已经是演出机位，必须从拍摄前的玩法姿态开始读输入。
    let prior = session
        .camera
        .as_ref()
        .filter(|(id, _, _)| *id == entity)
        .map_or(*pose, |(_, t, _)| *t);
    let slot = rig.0.get_or_insert_with(|| Controller {
        entity,
        pose: prior,
        enabled: state.enabled,
        yaw: state.yaw,
        pitch: state.pitch,
    });
    if slot.entity != entity {
        slot.entity = entity;
        slot.pose = *pose;
    }
    *pose = slot.pose;
    slot.enabled = state.enabled;
    let (yaw, pitch, _) = slot.pose.rotation.to_euler(EulerRot::YXZ);
    slot.yaw = yaw;
    slot.pitch = pitch;
    if control.camera && control.player {
        state.enabled = false;
    }
}

fn after(
    control: Res<CinematicControl>,
    mut rig: ResMut<FreeRig>,
    mut query: Query<(&mut Transform, &mut FreeCameraState), With<PrimarySceneView>>,
) {
    let Some(slot) = rig.0.as_mut() else { return };
    let Ok((mut pose, mut state)) = query.get_mut(slot.entity) else {
        return;
    };
    state.enabled = slot.enabled;
    if control.camera {
        pose.rotation = slot.pose.rotation;
        state.yaw = slot.yaw;
        state.pitch = slot.pitch;
        state.rotation_curve = None;
    }
    if control.player {
        pose.translation = slot.pose.translation;
        state.velocity = Vec3::ZERO;
    }
    slot.pose = *pose;
}

fn release(control: Res<CinematicControl>, mut rig: ResMut<FreeRig>) {
    if !control.camera && !control.player {
        rig.0 = None;
    }
}
