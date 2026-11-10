//! 树木实例生成 helper。
//! 【功能设计】场景层提供离线稳定树位，本模块只生成共享Mesh/Material句柄的四档节点，
//! 让Bevy自动硬件实例化相同网格与材质；不会在运行时随机散布或复制树木资产。
//! 【模块所有权】旧树根生成继续作为通用Mesh接管前及失败恢复时的安全基线。

use bevy::camera::visibility::VisibilityRange; // 每个渲染节点携带自己的四档可见范围。
use bevy::light::NotShadowCaster; // 最远公告板不进入阴影图，保持固定阴影预算。
use bevy::prelude::*; // 引入Commands、Mesh3d、MeshMaterial3d与Transform。
use feathrise_grounding::GroundSnap; // 树根在正式地形碰撞就绪后一次性吸附真实地表。

use super::components::{TreeInstance, TreeLodMesh}; // 生成树根与每档渲染节点的稳定身份。
use super::types::{TreeLodLevel, TreeMeshPart, TreePlacement, TreePrototype}; // 读取共享原型和离线树位。

/// 生成一棵由四档现有资产组成的世界稳定树木并返回根实体。
pub fn spawn_tree_instance(
    commands: &mut Commands, // 场景OnEnter系统通过统一命令队列创建根和渲染子节点。
    prototype: &TreePrototype, // 全部树位共享的Mesh、Material、LOD与根部下扎规格。
    placement: TreePlacement, // 当前树的稳定位置、朝向和缩放。
) -> Entity {
    let root_transform = Transform::from_translation(placement.position) // 先写入离线X/Z和等待吸附的占位Y。
        .with_rotation(Quat::from_rotation_y(placement.yaw_radians)); // 只旋转根节点，全部LOD保持完全同位。
    let root = commands
        .spawn((
            Name::new(format!("HN01Tree_{:03}", placement.id)), // 稳定编号方便世界检查器定位单树。
            TreeInstance {
                id: placement.id,      // 保存内容表编号供F3与碰撞代理关联。
                collision_proxy: None, // 玩家接近且地面吸附完成前不创建碰撞实体。
            }, // 完成根组件初始化。
            root_transform,        // 根变换只承载世界位置和稳定朝向，不缩放碰撞体。
            Visibility::Inherited, // 场景根可见时所有LOD子节点按各自范围参与渲染。
            GroundSnap::new(tree_ground_offset(prototype)), // 树根原点轻微下扎，让旧资产负Y根须自然埋入地面。
        ))
        .id(); // 保存根实体供场景生命周期标记和碰撞代理系统使用。
    commands.entity(root).with_children(|tree| {
        for lod in &prototype.lods {
            let visual_transform = tree_visual_transform(placement); // 全部LOD保持原资产地面原点，不再把负Y根须错误抬到地表。
            if let Some(trunk_mesh) = lod.trunk_mesh.as_ref() {
                let mut trunk = tree.spawn((
                    Name::new(format!(
                        "HN01Tree_{:03}_{:?}_Trunk",
                        placement.id, lod.level
                    )), // 树干节点名包含树号与LOD。
                    Mesh3d(trunk_mesh.clone()), // 复用当前LOD现有树干primitive句柄。
                    MeshMaterial3d(prototype.trunk_material.clone()), // 三个实体LOD共享同一树干材质以自动合批。
                    TreeLodMesh {
                        level: lod.level,          // F3按高、中、低统计可见树干节点。
                        part: TreeMeshPart::Trunk, // 明确该节点属于不透明树干。
                    }, // 完成树干渲染身份。
                    visual_transform, // 与树冠、公告板保持完全相同的根位置、朝向和缩放。
                    VisibilityRange {
                        start_margin: lod.visibility_range.start_margin.clone(), // 保留当前LOD进入交叉区。
                        end_margin: lod.visibility_range.end_margin.clone(), // 保留当前LOD退出交叉区。
                        use_aabb: lod.visibility_range.use_aabb, // 全部档位按同一树根计算距离。
                    }, // 完成Bevy原生距离LOD配置。
                )); // 生成当前实体LOD树干节点；材质就绪后由场景层原子替换为根部固定的GPU顶点风材质。
                if !lod.casts_shadow {
                    trunk.insert(NotShadowCaster); // 低档树干退出阴影图，限制全图林带远景投影成本。
                } // 树干与同档树冠共享同一投影策略。
            } // 公告板没有单独树干节点。
            let canopy_material = if lod.level == TreeLodLevel::Billboard {
                prototype.billboard_material.clone() // 最远档使用现有公告板图集材质。
            } else {
                prototype.canopy_material.clone() // 三个实体档复用现有树冠材质。
            }; // 完成当前档位材质选择。
            let canopy_part = if lod.level == TreeLodLevel::Billboard {
                TreeMeshPart::Billboard // 公告板同时表达整棵树。
            } else {
                TreeMeshPart::Canopy // 实体LOD节点只表达树冠簇。
            }; // 完成渲染部件身份选择。
            let mut canopy = tree.spawn((
                Name::new(format!(
                    "HN01Tree_{:03}_{:?}_{:?}",
                    placement.id, lod.level, canopy_part
                )), // 节点名保留树号、LOD和部件。
                Mesh3d(lod.canopy_mesh.clone()), // 实体档读取树冠primitive，最远档读取公告板primitive。
                MeshMaterial3d(canopy_material), // 复用当前档共享材质以触发Bevy自动硬件实例化。
                TreeLodMesh {
                    level: lod.level,  // F3按四档统计当前可见树冠或公告板。
                    part: canopy_part, // 区分实体树冠与完整公告板。
                }, // 完成树冠/公告板身份。
                visual_transform,                // 与同档树干保持严格同位，避免交叉淡化时树冠漂移。
                VisibilityRange {
                    start_margin: lod.visibility_range.start_margin.clone(), // 相邻档共享一致进入交叉区。
                    end_margin: lod.visibility_range.end_margin.clone(), // 相邻档共享一致退出交叉区。
                    use_aabb: lod.visibility_range.use_aabb, // 全部档按树根原点比较距离。
                }, // 完成当前档位可见范围。
            )); // 生成当前档位树冠或公告板。
            if !lod.casts_shadow {
                canopy.insert(NotShadowCaster); // 低档与公告板不重复渲染远景阴影，高中档继续随GPU风同步投影。
            } // 公告板保持静止且不投影；三个实体LOD稍后通过共享GPU材质获得随树高增加的顶点风。
        } // 完成高、中、低、公告板四档节点生成。
    }); // 完成单棵树的共享资产子树。
    root // 返回根实体供场景层绑定DespawnOnExit。
} // 完成单棵树实例生成。

/// 把正的下扎深度转换为GroundSnap使用的负地表偏移。
fn tree_ground_offset(prototype: &TreePrototype) -> f32 {
    -prototype.ground_embed_depth.max(0.0) // 非法负深度钳为零，合法值让树根原点进入地表。
} // 完成树根吸附偏移换算。

/// 构造不含额外Y抬升的LOD视觉变换。
fn tree_visual_transform(placement: TreePlacement) -> Transform {
    Transform::from_scale(Vec3::splat(placement.uniform_scale)) // 旧GLB的负Y根须本来就用于埋土，只应用统一体量缩放。
} // 完成树木视觉变换构造。

#[cfg(test)] // 纯变换回归无需启动渲染器或物理世界。
mod tests {
    use bevy::camera::visibility::VisibilityRange; // 构造最小四档原型。
    use bevy::prelude::*; // 构造弱资产句柄、树位和变换。

    use super::{tree_ground_offset, tree_visual_transform}; // 直接验证贴地纯函数，GPU风分级由wind模块独立测试。
    use crate::tree::{TreeLodLevel, TreeLodPrototype, TreePlacement, TreePrototype}; // 构造公共技术原型。

    #[test] // 旧资产负Y根须不得再次被抬到地表上方。
    fn tree_visual_transform_keeps_asset_ground_origin_unlifted() {
        let placement = TreePlacement {
            id: 0,                // 稳定编号不参与视觉变换。
            position: Vec3::ZERO, // 纯视觉子节点只保存局部缩放。
            yaw_radians: 0.0,     // 根实体承担朝向，子节点不重复旋转。
            uniform_scale: 0.4,   // 使用HN-01实际缩放范围中的代表值。
        }; // 完成测试树位。
        let transform = tree_visual_transform(placement); // 生成生产路径使用的LOD局部变换。
        assert_eq!(transform.translation.y, 0.0); // 禁止按Mesh最低点再次加入正Y抬升。
        assert_eq!(transform.scale, Vec3::splat(0.4)); // 贴地修复不得改变用户已看到的树木体量。
    } // 完成视觉根部回归。

    #[test] // 正下扎深度必须转换为负GroundSnap偏移。
    fn tree_ground_offset_embeds_root_below_surface() {
        let lod = TreeLodPrototype {
            level: TreeLodLevel::High,           // 测试只需要一份可克隆的合法LOD模板。
            trunk_mesh: Some(Handle::default()), // 弱句柄足以构造纯数据原型。
            canopy_mesh: Handle::default(),      // 测试不访问真实网格资产。
            visibility_range: VisibilityRange::default(), // 距离范围不参与贴地偏移计算。
            casts_shadow: true,                  // 贴地纯函数不读取阴影策略，使用高档生产默认值。
        }; // 完成LOD模板。
        let prototype = TreePrototype {
            lods: [lod.clone(), lod.clone(), lod.clone(), lod], // 固定四档满足公共原型结构。
            trunk_material: Handle::default(),                  // 测试不访问真实树干材质。
            canopy_material: Handle::default(),                 // 测试不访问真实树冠材质。
            billboard_material: Handle::default(),              // 测试不访问真实公告板材质。
            ground_embed_depth: 0.08,                           // 使用HN-01修复后的八厘米下扎值。
        }; // 完成共享树木原型。
        assert_eq!(tree_ground_offset(&prototype), -0.08); // GroundSnap必须把树根原点解算到地表下方。
    } // 完成树根下扎回归。
} // 结束树木贴地纯函数测试模块。
