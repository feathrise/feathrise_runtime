//! 三维Mesh种植沿用原预设碰撞；CPU只派生有界玩家近场代理，不能生成渲染散布。
use super::*;
use avian3d::prelude::{Collider, RigidBody};
use feathrise_scene_content::VegetationPresetCollision;

#[derive(Clone)]
pub(super) struct Source {
    pub cells: feathrise_environment::vegetation::VolumeMeshCells,
    pub collision: VegetationPresetCollision,
}
#[derive(Resource, Default)]
pub(super) struct Physics {
    task: Option<Task<Vec<(Collider, Transform)>>>,
    building: Option<(Entity, [i32; 3])>,
    active: Option<(Entity, [i32; 3])>,
    root: Option<Entity>,
}

pub(super) fn install(app: &mut App) {
    app.init_instance_resource::<Physics>()
        .add_systems(
            Update,
            drive
                .after(super::drive)
                .run_if(in_state(feathrise_scene_core::state::ScenePhase::Active)),
        )
        .add_systems(
            OnExit(feathrise_scene_core::state::ScenePhase::Active),
            |mut state: InstanceResMut<Physics>| {
                let old = std::mem::take(&mut *state);
                AsyncComputeTaskPool::get()
                    .spawn(async move { drop(old) })
                    .detach();
            },
        );
}

fn build(sources: Arc<Vec<Source>>, focus: Vec3) -> Vec<(Collider, Transform)> {
    let mut candidates = Vec::new();
    for source in sources.iter() {
        let Some(radius) = source.collision.exit_distance() else {
            continue;
        };
        for cell in source.cells.cells.iter() {
            let point = Vec3::from_array([cell[0][0], cell[0][1], cell[0][2]]);
            let distance = point.distance_squared(focus);
            if distance > radius * radius {
                continue;
            }
            let Some(pose) = source.cells.physics_pose(cell) else {
                continue;
            };
            candidates.push((distance, source.collision.clone(), pose));
            // 中间候选也有硬内存上界；保留最近代理，作者与GPU显示数据不截断。
            if candidates.len() >= 256 {
                candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
                candidates.truncate(128);
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    candidates.truncate(128);
    candidates
        .into_iter()
        .filter_map(|(_, collision, pose)| {
            let (collider, center) = match collision {
                VegetationPresetCollision::None => return None,
                VegetationPresetCollision::Cylinder {
                    radius,
                    height,
                    center_y,
                    scale_with_instance,
                    ..
                } => {
                    let scale = if scale_with_instance {
                        pose.scale.x
                    } else {
                        1.
                    };
                    (
                        Collider::cylinder(radius * scale, height * scale),
                        Vec3::Y * center_y * scale,
                    )
                }
                VegetationPresetCollision::Cuboid {
                    size,
                    center,
                    scale_with_instance,
                    ..
                } => {
                    let scale = if scale_with_instance {
                        pose.scale.x
                    } else {
                        1.
                    };
                    let size = Vec3::from_array(size) * scale;
                    (
                        Collider::cuboid(size.x, size.y, size.z),
                        Vec3::from_array(center) * scale,
                    )
                }
            };
            Some((
                collider,
                Transform::from_translation(pose.translation + pose.rotation * center)
                    .with_rotation(pose.rotation),
            ))
        })
        .collect()
}

pub(super) fn drive(
    frame: feathrise_scene_core::residency::InstanceFrame,
    mut commands: Commands,
    source: InstanceRes<super::State>,
    mut state: InstanceResMut<Physics>,
    players: Query<&Transform, With<crate::SceneRuntimeObserver>>,
    owners: InstanceQuery<(), With<super::VolumeVegetationRoot>>,
) {
    let (Some(owner), Ok(player)) = (
        source.root.filter(|e| source.valid && owners.contains(*e)),
        players.single(),
    ) else {
        if let Some(root) = state.root.take() {
            commands.entity(root).try_despawn();
        }
        state.active = None;
        return;
    };
    let identity = (
        owner,
        (frame.to_local(player.translation) / 4.).floor().as_ivec3().to_array(),
    );
    if let Some(ready) = state.task.as_mut().and_then(check_ready) {
        let built = state.building.take().unwrap();
        state.task = None;
        if built == identity {
            let root = commands
                .spawn((
                    Name::new("三维植被近场碰撞"),
                    Transform::IDENTITY,
                    ChildOf(owner),
                ))
                .id();
            for (collider, transform) in ready {
                commands.spawn((RigidBody::Static, collider, transform, ChildOf(root)));
            }
            if let Some(old) = state.root.replace(root) {
                commands.entity(old).try_despawn();
            }
            state.active = Some(identity);
        }
    }
    if state.task.is_none() && state.active != Some(identity) {
        let sources = source.collision_sources.clone();
        let focus = frame.to_local(player.translation);
        state.building = Some(identity);
        state.task = Some(AsyncComputeTaskPool::get().spawn(async move { build(sources, focus) }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wall_tree_physics_uses_the_same_surface_pose_and_embed_as_gpu() {
        let cells = feathrise_environment::vegetation::VolumeMeshCells {
            cells: Arc::from(vec![[
                [1., 2., 3., 1.],
                [1., 0., 0., 0.25],
                [1., 2., 3., 42.],
            ]]),
            seed: 12,
            spacing: 0.25,
            scale_range: [2., 2.],
            ground_embed_depth: 0.1,
            min: Vec3::ZERO,
            max: Vec3::splat(4.),
        };
        let pose = cells.physics_pose(&cells.cells[0]).unwrap();
        assert!(pose.rotation.mul_vec3(Vec3::Y).distance(Vec3::X) < 1e-5);
        assert!(pose.translation.distance(Vec3::new(0.9, 2., 3.)) < 1e-5);
        let source = Source {
            cells,
            collision: VegetationPresetCollision::Cylinder {
                radius: 0.2,
                height: 2.,
                center_y: 1.,
                scale_with_instance: true,
                enter_distance: 8.,
                exit_distance: 12.,
            },
        };
        let ready = build(Arc::new(vec![source]), Vec3::new(1., 2., 3.));
        assert_eq!(ready.len(), 1);
        assert!(ready[0].1.translation.distance(Vec3::new(2.9, 2., 3.)) < 1e-5);
    }
}
