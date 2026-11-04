//! 通用多部件Mesh实例的共享层级生成入口。
//! 【功能设计】手动调用与自动生命周期复用同一装配函数；本模块不决定布局换版、回退或项目内容类别。

use bevy::camera::visibility::{NoFrustumCulling, VisibilityRange}; // GPU转向节点禁用不再可信的作者姿态CPU视锥边界。
use bevy::light::NotShadowCaster; // 每个部件可独立退出阴影视图。
use bevy::prelude::{Commands, Entity, Mesh3d, Name, Transform, Visibility}; // 通用实例组合共享Mesh与类型擦除材质绑定。

use crate::{
    ActiveVegetationLayout, VegetationCandidate, VegetationLayerId, resolved_candidate_enabled,
}; // 实例身份不包含树、灌木或岩石枚举。

use super::billboard::VegetationMeshBillboardMaterial; // 动态模式必须绑定该PBR扩展材质才会在GPU转向。
use super::components::{
    VegetationMeshBillboardNode, VegetationMeshInstance, VegetationMeshLodNode,
    VegetationMeshRenderIdentity, VegetationMeshWind,
}; // 根与子节点只写显式技术身份和内容无关公告板配置。
use super::types::VegetationMeshPrototype; // 原型提供任意LOD和任意部件共享资产。

/// 生成一个由任意LOD与部件组成的世界稳定Mesh实例。
pub fn spawn_vegetation_mesh_instance(
    commands: &mut Commands,
    layer: VegetationLayerId,
    prototype: &VegetationMeshPrototype,
    instance_id: u64,
    transform: Transform,
) -> Entity {
    let root = commands
        .spawn((
            Name::new(format!(
                "Vegetation_{}_{}_{}",
                layer.0, prototype.id.0, instance_id
            )), // 名称只包含技术身份，不写入树、灌木或岩石语义。
            VegetationMeshInstance {
                layer,                   // 保留所属无限布局层。
                prototype: prototype.id, // 保留共享原型身份。
                instance_id,             // 保留世界稳定候选ID。
            },
            transform,             // 根节点承载世界平移、旋转和缩放。
            Visibility::Inherited, // 各LOD子节点继续按自己的范围参与渲染。
        ))
        .id(); // 返回根实体供地图绑定场景生命周期或碰撞代理。
    commands.entity(root).with_children(|children| {
        for lod in &prototype.lods {
            for part in &lod.parts {
                let mut child = children.spawn((
                    Name::new(format!(
                        "Vegetation_{}_{}_{}_Lod{}_Part{}",
                        layer.0, prototype.id.0, instance_id, lod.level.0, part.part_id
                    )), // 子节点名称只保留通用技术身份。
                    Mesh3d(part.mesh.clone()), // 同原型同档全部实例共享Mesh以触发硬件实例化。
                    part.material.node_identity(), // 异构标准/扩展材质共享统一诊断身份。
                    VegetationMeshLodNode {
                        level: lod.level,                    // 诊断读取档位而不解析名称。
                        part_id: part.part_id,               // 多部件原型保持稳定部件身份。
                        billboard: part.billboard.is_some(), // 诊断继续提供无需解释具体模式的能力位。
                    },
                    VegetationMeshRenderIdentity {
                        layer,                               // RenderApp按真实通用层过滤，旧专业节点不会获得该身份。
                        prototype: prototype.id, // 同层不同原型或热替换继续可被精确区分。
                        level: lod.level,        // 分箱快照保留当前通用LOD数值。
                        part_id: part.part_id,   // 分箱快照保留当前稳定部件编号。
                        billboard: part.billboard.is_some(), // 最远公告板不依赖材质AlphaMode猜测。
                    },
                    part.local_transform, // 原型部件可相对同一实例根偏移、旋转或缩放。
                    VisibilityRange {
                        start_margin: lod.visibility_range.start_margin.clone(), // 保留进入宽域。
                        end_margin: lod.visibility_range.end_margin.clone(),     // 保留退出宽域。
                        use_aabb: lod.visibility_range.use_aabb, // 消费者选择按根或各部件AABB测距。
                    },
                )); // 生成当前LOD当前部件节点。
                part.material.insert_into(&mut child); // 恢复唯一正确的MeshMaterial3d<M>具体组件。
                if let Some(billboard) = part.billboard {
                    let gpu_material_ready = !billboard.requires_camera()
                        || part
                            .material
                            .is_material::<VegetationMeshBillboardMaterial>(); // 固定模式无需扩展；动态模式必须显式使用GPU材质。
                    child.insert(VegetationMeshBillboardNode {
                        billboard,                                    // 保存固定、绕Y轴或全向朝向模式。
                        rest_rotation: part.local_transform.rotation, // 保存GPU材质缺失时的作者局部回退姿态。
                        gpu_material_ready, // 诊断只把真实GPU材质节点计为已朝向。
                    }); // 只有显式公告板部件才进入朝向与统一诊断查询。
                    if billboard.requires_camera()
                        && gpu_material_ready
                        && part.culling_aabb.is_none()
                    {
                        child.insert(NoFrustumCulling); // 没有显式全朝向保守AABB时，禁止静态薄平面被错误提前剔除。
                    } // 固定、材质回退或已提供保守AABB的节点继续使用Bevy正常视锥裁剪。
                } // 普通实体LOD不承担任何公告板逐帧成本。
                if let Some(culling_aabb) = part.culling_aabb {
                    child.insert(culling_aabb); // GPU顶点变形部件使用调用方已计算的保守剔除边界。
                } // 无覆盖时继续等待Bevy按Mesh自动计算AABB。
                if !part.casts_shadow {
                    child.insert(NotShadowCaster); // 远档或公告板可退出阴影图。
                }
                if let Some(wind) = part.wind {
                    child.insert(VegetationMeshWind(wind)); // 材质适配器按高度权重驱动风，不绑定树类别。
                }
            }
        }
    });
    root // 返回完整通用实例根。
}

/// 仅当当前活动分配场选择了候选时生成通用Mesh实例。
///
/// 调用方可在布局换版或脏区生命周期系统中使用本入口；竞争已经由核心解析完成，
/// 因此这里不会遍历其他层，也不会把内容类别耦合进Mesh后端。
pub fn spawn_resolved_vegetation_mesh_instance(
    commands: &mut Commands,
    layout: &ActiveVegetationLayout,
    layer: VegetationLayerId,
    prototype: &VegetationMeshPrototype,
    candidate: &VegetationCandidate,
) -> Option<Entity> {
    resolved_candidate_enabled(layout, layer, candidate).then(|| {
        spawn_vegetation_mesh_instance(
            commands,
            layer,
            prototype,
            candidate.instance_id,
            candidate.transform,
        )
    }) // 稳定候选身份和Transform在远场、近场代理与Mesh之间保持一致。
}
