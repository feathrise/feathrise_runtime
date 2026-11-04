//! 动态 Mesh 植被的玩家近场碰撞代理。
//!
//! 【功能设计】远景实例继续只参与共享渲染批次；本模块只在玩家跨越固定小网格或空间块
//! 新常驻时，扫描与碰撞外圈相交的少量块，并按稳定实例 ID 增删简化静态代理。

use avian3d::prelude::{Collider, RigidBody};
use bevy::prelude::*;
use feathrise_scene_core::residency::MapSourceFrame;
use feathrise_map_content::{
    MapVegetationInstanceTransform, VegetationPresetCollision, VegetationPresetRegistry,
    VegetationPresetRuntime,
};

use super::resources::{DynamicVegetationBuild, DynamicVegetationState};

/// 玩家跨越该水平网格后才重新核对近场代理；预设进入距离远大于该值，移动时不会临门才创建。
pub(super) const DYNAMIC_COLLISION_FOCUS_CELL_METERS: f32 = 16.0;

/// 玩家跨世界16m小格、网格姿态改变或块采用后才重新核对，稳定帧不扫描逐株数据。
pub(super) fn update_dynamic_mesh_collision_focus(
    commands: &mut Commands,
    player_world: Vec3,
    frame: MapSourceFrame,
    source: &DynamicVegetationBuild,
    presets: &VegetationPresetRegistry,
    state: &mut DynamicVegetationState,
    resident_changed: bool,
) -> Result<bool, String> {
    let cell = [
        (player_world.x / DYNAMIC_COLLISION_FOCUS_CELL_METERS).floor() as i32,
        (player_world.z / DYNAMIC_COLLISION_FOCUS_CELL_METERS).floor() as i32,
    ];
    if state.collision_focus_cell == Some(cell) && state.collision_frame == Some(frame) && !resident_changed {
        return Ok(false);
    }
    let changed = sync_dynamic_mesh_collisions(commands, player_world.xz(), frame,
        source, presets, state)?;
    state.collision_focus_cell = Some(cell);
    state.collision_frame = Some(frame);
    Ok(changed)
}

/// 一份 Mesh 预设中渲染贴地与物理形状共同使用的解析结果。
pub(super) struct MeshPresetPhysics<'a> {
    pub(super) ground_embed_depth: f32,
    pub(super) collision: &'a VegetationPresetCollision,
}

/// 从地图保存的稳定预设 ID 解析当前目录中的物理规格；历史别名也会归一到同一规范项。
pub(super) fn resolve_mesh_preset_physics<'a>(
    presets: &'a VegetationPresetRegistry,
    profile: &str,
) -> Result<MeshPresetPhysics<'a>, String> {
    let preset = presets
        .resolve(profile)
        .ok_or_else(|| format!("开发者植被预设{profile}不存在"))?;
    match &preset.runtime {
        VegetationPresetRuntime::Mesh {
            ground_embed_depth,
            collision,
            ..
        } => Ok(MeshPresetPhysics {
            ground_embed_depth: *ground_embed_depth,
            collision,
        }),
        _ => Err(format!("植被预设{}不是Mesh后端", preset.id)),
    }
}

/// 后台三维根范围固定成本变换到世界XZ；只有与近场圆相交的块才扫描逐株变换。
fn chunk_intersects_radius(
    source_root_bounds: Option<[Vec3; 2]>,
    frame: MapSourceFrame,
    focus_world: Vec2,
    radius: f32,
) -> bool {
    let Some([minimum, maximum]) = source_root_bounds else {
        return false;
    };
    let center = frame.to_world((minimum + maximum) * 0.5).xz();
    let half_extent = (maximum - minimum) * 0.5;
    // 绝对矩阵乘半尺寸等价于变换八角取界，完整计入源Y、嵌套旋转与非均匀缩放。
    let world_extent = frame.source_to_world.x_axis.xz().abs() * half_extent.x
        + frame.source_to_world.y_axis.xz().abs() * half_extent.y
        + frame.source_to_world.z_axis.xz().abs() * half_extent.z;
    let nearest = focus_world.clamp(center - world_extent, center + world_extent);
    nearest.distance_squared(focus_world) <= radius * radius
}

/// 把预设局部碰撞规格和稳定实例姿态转换成不继承视觉层级缩放的世界静态代理。
fn collision_parts(
    collision: &VegetationPresetCollision,
    instance: &MapVegetationInstanceTransform,
    ground_embed_depth: f32,
) -> Option<(Collider, Transform)> {
    let yaw = Quat::from_rotation_y(instance.yaw_radians);
    let mut root = Vec3::from_array(instance.translation);
    root.y -= ground_embed_depth;
    match collision {
        VegetationPresetCollision::None => None,
        VegetationPresetCollision::Cylinder {
            radius,
            height,
            center_y,
            scale_with_instance,
            ..
        } => {
            let scale = if *scale_with_instance {
                instance.scale
            } else {
                1.0
            };
            Some((
                Collider::cylinder(*radius * scale, *height * scale),
                Transform::from_translation(root + Vec3::Y * (*center_y * scale))
                    .with_rotation(yaw),
            ))
        }
        VegetationPresetCollision::Cuboid {
            size,
            center,
            scale_with_instance,
            ..
        } => {
            let scale = if *scale_with_instance {
                instance.scale
            } else {
                1.0
            };
            let size = Vec3::from_array(*size) * scale;
            let center = yaw * (Vec3::from_array(*center) * scale);
            Some((
                Collider::cuboid(size.x, size.y, size.z),
                Transform::from_translation(root + center).with_rotation(yaw),
            ))
        }
    }
}

fn clear_chunk_collisions(
    commands: &mut Commands,
    chunk: &mut super::resources::DynamicMeshResidentChunk,
) -> bool {
    if chunk.collision_proxies.is_empty() {
        return false;
    }
    for (_, proxy) in std::mem::take(&mut chunk.collision_proxies) {
        commands.entity(proxy).try_despawn();
    }
    true
}

/// 同步玩家附近的精确逐株代理；目录、地图和作者实例数量均不使用固定容量。
fn sync_dynamic_mesh_collisions(
    commands: &mut Commands,
    focus_world: Vec2,
    frame: MapSourceFrame,
    source: &DynamicVegetationBuild,
    presets: &VegetationPresetRegistry,
    state: &mut DynamicVegetationState,
) -> Result<bool, String> {
    let mut changed = false;
    for (key, chunk) in &mut state.resident_mesh_chunks {
        let layer = source
            .mesh_layers
            .iter()
            .find(|layer| layer.layer_index == key.layer_index)
            .ok_or_else(|| "动态Mesh碰撞引用了不存在的正式层".to_owned())?;
        let profile = layer
            .layer
            .runtime_profile
            .as_deref()
            .ok_or_else(|| format!("Mesh植被层{}缺少正式预设ID", layer.layer.id))?;
        let physics = resolve_mesh_preset_physics(presets, profile)?;
        let Some(enter_distance) = physics.collision.enter_distance() else {
            changed |= clear_chunk_collisions(commands, chunk);
            continue;
        };
        let exit_distance = physics
            .collision
            .exit_distance()
            .expect("有创建距离的碰撞规格必有回收距离");
        if !chunk_intersects_radius(chunk.source_root_bounds, frame, focus_world, exit_distance) {
            changed |= clear_chunk_collisions(commands, chunk);
            continue; // 远块不扫描逐株变换，远景渲染与物理生命周期完全解耦。
        }
        let enter_squared = enter_distance * enter_distance;
        let exit_squared = exit_distance * exit_distance;
        for instance in chunk.transforms.iter() {
            // 【功能设计】距离合同仍是世界XZ米制；倾斜网格必须先完整变换三维作者根，
            // 再投影到世界XZ，不能先丢弃玩家或树根的源Y而让近场代理提前出现或漏建。
            let root_world = frame.to_world(Vec3::from_array(instance.translation)).xz();
            let distance_squared = root_world.distance_squared(focus_world);
            if let Some(proxy) = chunk.collision_proxies.get(&instance.instance_id).copied() {
                if distance_squared > exit_squared {
                    commands.entity(proxy).try_despawn();
                    chunk.collision_proxies.remove(&instance.instance_id);
                    changed = true;
                }
                continue;
            }
            if distance_squared > enter_squared {
                continue;
            }
            let Some((collider, transform)) =
                collision_parts(physics.collision, instance, physics.ground_embed_depth)
            else {
                continue;
            };
            let proxy = commands
                .spawn((
                    Name::new(format!(
                        "动态植被碰撞_{}_{}",
                        layer.layer.id, instance.instance_id
                    )),
                    RigidBody::Static,
                    collider,
                    transform,
                    ChildOf(chunk.root), // 空间块或整份植被修订回收时自动带走代理。
                ))
                .id();
            chunk.collision_proxies.insert(instance.instance_id, proxy);
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
#[path = "collision_frame_tests.rs"]
mod frame_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_scaling_can_follow_or_ignore_instance_scale() {
        let instance = MapVegetationInstanceTransform {
            instance_id: 1,
            translation: [10.0, 2.0, 20.0],
            yaw_radians: 0.0,
            scale: 2.0,
        };
        let scaled = VegetationPresetCollision::Cylinder {
            radius: 1.0,
            height: 4.0,
            center_y: 2.0,
            scale_with_instance: true,
            enter_distance: 10.0,
            exit_distance: 12.0,
        };
        let (_, scaled_transform) = collision_parts(&scaled, &instance, 0.5).unwrap();
        assert_eq!(scaled_transform.translation, Vec3::new(10.0, 5.5, 20.0));

        let world_sized = VegetationPresetCollision::Cylinder {
            radius: 1.0,
            height: 4.0,
            center_y: 2.0,
            scale_with_instance: false,
            enter_distance: 10.0,
            exit_distance: 12.0,
        };
        let (_, world_transform) = collision_parts(&world_sized, &instance, 0.5).unwrap();
        assert_eq!(world_transform.translation, Vec3::new(10.0, 3.5, 20.0));
    }
}
