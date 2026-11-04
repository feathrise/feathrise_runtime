//! 固定景观实例生成helper。
//! 【功能设计】地图层提供离线稳定落点，本模块只生成共享Mesh/Material的四档节点并交给Bevy自动实例批处理。
//! 【模块所有权】旧根生成继续作为通用Mesh接管前及失败恢复时的安全基线。

use bevy::camera::visibility::VisibilityRange; // 每个渲染节点携带独立四档可见范围。
use bevy::light::NotShadowCaster; // 中远档可显式退出阴影视图限制固定最坏预算。
use bevy::prelude::*; // Commands、Mesh3d、MeshMaterial3d与Transform。
use feathrise_grounding::GroundSnap; // 根实体在正式地形碰撞就绪后一次性吸附真实高度。

use super::{LandscapeInstance, LandscapeLodMesh, LandscapePlacement, LandscapePrototype}; // 读取公共运行身份与稳定内容值。

/// 生成一个由三档现有资产和一档动态代理组成的世界稳定景观实例并返回根实体。
pub fn spawn_landscape_instance(
    commands: &mut Commands, // 地图OnEnter系统通过统一命令队列创建根和LOD子节点。
    prototype: &LandscapePrototype, // 当前类别共享的四档Mesh、Material、LOD与下扎规格。
    placement: LandscapePlacement, // 当前实例的稳定位置、朝向、缩放和碰撞资格。
) -> Entity {
    debug_assert_eq!(prototype.kind, placement.kind); // 开发构建阻止灌木清单误套岩石原型。
    let root_transform = Transform::from_translation(placement.position) // 先写入离线X/Z和等待吸附的占位Y。
        .with_rotation(Quat::from_rotation_y(placement.yaw_radians)); // 根节点统一承担稳定世界朝向。
    let root = commands
        .spawn((
            Name::new(format!(
                "HN01Landscape_{:?}_{:03}",
                placement.kind, placement.id
            )), // 名称保留类别和稳定编号。
            LandscapeInstance {
                id: placement.id,      // 保存离线清单编号供F3与碰撞代理关联。
                kind: placement.kind,  // 保存当前实例类别供查询无父级回溯。
                collision_proxy: None, // 玩家接近且地面吸附完成前不创建碰撞。
                collision_enabled: placement.collision_enabled, // 只让离线中大型岩石进入碰撞管理。
                uniform_scale: placement.uniform_scale, // 保存视觉同源体量供简化碰撞缩放。
            }, // 完成景观根运行状态初始化。
            root_transform, // 根变换保存世界位置和稳定朝向，不重复缩放物理代理。
            Visibility::Inherited, // 地图根可见时四档子节点按自己的范围参与渲染。
            GroundSnap::new(-prototype.ground_embed_depth.max(0.0)), // 正下扎深度转换为GroundSnap负偏移。
        ))
        .id(); // 保存根实体供地图层绑定DespawnOnExit和碰撞代理管理。
    commands.entity(root).with_children(|children| {
        for lod in &prototype.lods {
            let mut child = children.spawn((
                Name::new(format!(
                    "HN01Landscape_{:?}_{:03}_{:?}",
                    placement.kind, placement.id, lod.level
                )), // 渲染节点名保留类别、编号和LOD身份。
                Mesh3d(lod.mesh.clone()), // 全图同类别同档实例共享一个现有Mesh句柄。
                MeshMaterial3d(lod.material.clone()), // 全图同类别同档实例共享一个源PBR材质句柄。
                LandscapeLodMesh {
                    kind: placement.kind, // F3按灌木或岩石分开统计。
                    level: lod.level,     // F3按高、中、低、超低统计当前节点。
                }, // 完成LOD渲染身份。
                Transform::from_scale(Vec3::splat(placement.uniform_scale)), // 子节点只应用统一体量缩放。
                VisibilityRange {
                    start_margin: lod.visibility_range.start_margin.clone(), // 保留当前档进入宽域。
                    end_margin: lod.visibility_range.end_margin.clone(),     // 保留当前档退出宽域。
                    use_aabb: lod.visibility_range.use_aabb, // 四档统一按根位置而非不同AABB中心测距。
                }, // 完成Bevy原生距离LOD配置。
            )); // 生成当前高、中、低或超低档渲染节点。
            if !lod.casts_shadow {
                child.insert(NotShadowCaster); // 低价值中远档不进入方向光阴影视图。
            } // 完成当前档阴影预算分支。
        } // 完成一个实例的四档共享资产节点。
    }); // 完成单个固定景观实例层级。
    root // 返回根实体供地图层添加场景生命周期。
} // 完成固定景观实例生成。
