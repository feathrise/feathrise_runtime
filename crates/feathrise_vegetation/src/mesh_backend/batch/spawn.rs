//! 一个空间块只生成批次根及每档每部件一个渲染节点。

use std::sync::Arc;

use bevy::camera::{
    primitives::Aabb,
    visibility::{NoAutoAabb, VisibilityRange},
};
use bevy::light::NotShadowCaster;
use bevy::math::Vec3A;
use bevy::prelude::*;
use bevy::render::batching::NoAutomaticBatching;

use crate::VegetationLayerId;

use super::components::{
    VegetationMeshBatch, VegetationMeshBatchInstances, VegetationMeshBatchNode,
    VegetationMeshBatchPlacement, VegetationMeshBatchPlacements,
};
use crate::mesh_backend::billboard::VegetationMeshBillboardMaterial;
use crate::mesh_backend::components::{
    VegetationMeshBillboardNode, VegetationMeshLodNode, VegetationMeshRenderIdentity,
    VegetationMeshWind,
};
use crate::mesh_backend::types::VegetationMeshPrototype;

/// 未提供显式保守边界的静态部件使用该半径估算分块总 AABB；它只影响粗块视锥剔除。
const FALLBACK_PART_HALF_EXTENT_METERS: f32 = 32.0;

/// 把一个局部 AABB 的八个角并入批次根局部空间总边界。
fn include_transformed_aabb(min: &mut Vec3, max: &mut Vec3, transform: Mat4, aabb: &Aabb) {
    let center = Vec3::from(aabb.center);
    let half = Vec3::from(aabb.half_extents);
    for x in [-half.x, half.x] {
        for y in [-half.y, half.y] {
            for z in [-half.z, half.z] {
                let point = transform.transform_point3(center + Vec3::new(x, y, z));
                *min = min.min(point);
                *max = max.max(point);
            }
        }
    }
}

fn batch_part_aabb(
    placements: &[VegetationMeshBatchPlacement],
    local_transform: Transform,
    authored: Option<Aabb>,
) -> Aabb {
    let authored = authored.unwrap_or(Aabb {
        center: Vec3A::ZERO,
        half_extents: Vec3A::splat(FALLBACK_PART_HALF_EXTENT_METERS),
    });
    let local = local_transform.to_matrix();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for placement in placements {
        include_transformed_aabb(
            &mut min,
            &mut max,
            placement.transform.to_matrix() * local,
            &authored,
        );
    }
    if placements.is_empty() {
        min = Vec3::ZERO;
        max = Vec3::ZERO;
    }
    Aabb::from_min_max(min, max)
}

/// 生成一个不设作者实例总量上限的通用 Mesh 批次。
///
/// `placements` 中每个值仍是独立作者实例；这里只把 ECS 与 draw 提交从逐株改为逐空间块。
/// 保持普通二维散布调用兼容。
pub fn spawn_vegetation_mesh_batch(
    commands: &mut Commands,
    layer: VegetationLayerId,
    prototype: &VegetationMeshPrototype,
    batch_id: u64,
    placements: Vec<VegetationMeshBatchPlacement>,
    root_transform: Transform,
) -> Entity {
    spawn_batch(
        commands,
        layer,
        prototype,
        batch_id,
        placements,
        root_transform,
        None,
    )
}
/// 复用完整模型、PBR、风、公告板与LOD；CPU只提交三维作者格，不生成逐株Transform。
pub fn spawn_volume_mesh_batch(
    commands: &mut Commands,
    layer: VegetationLayerId,
    prototype: &VegetationMeshPrototype,
    batch_id: u64,
    cells: super::components::VolumeMeshCells,
) -> Entity {
    spawn_batch(
        commands,
        layer,
        prototype,
        batch_id,
        Vec::new(),
        Transform::IDENTITY,
        Some(cells),
    )
}

fn spawn_batch(
    commands: &mut Commands,
    layer: VegetationLayerId,
    prototype: &VegetationMeshPrototype,
    batch_id: u64,
    placements: Vec<VegetationMeshBatchPlacement>,
    root_transform: Transform,
    volume: Option<super::components::VolumeMeshCells>,
) -> Entity {
    let placements: Arc<[VegetationMeshBatchPlacement]> = Arc::from(placements);
    let root = commands
        .spawn((
            Name::new(format!(
                "VegetationBatch_{}_{}_{}",
                layer.0, prototype.id.0, batch_id
            )),
            VegetationMeshBatch {
                layer,
                prototype: prototype.id,
                batch_id,
                instance_count: volume.as_ref().map_or(placements.len(), |v| v.cells.len()),
            },
            VegetationMeshBatchPlacements(placements.clone()),
            root_transform,
            Visibility::Inherited,
        ))
        .id();
    if placements.is_empty() && volume.is_none() {
        return root;
    }

    commands.entity(root).with_children(|children| {
        for (lod_index, lod) in prototype.lods.iter().enumerate() {
            for part in &lod.parts {
                let mut child = children.spawn((
                    Name::new(format!(
                        "VegetationBatch_{}_{}_{}_Lod{}_Part{}",
                        layer.0, prototype.id.0, batch_id, lod.level.0, part.part_id
                    )),
                    Mesh3d(part.mesh.clone()),
                    part.material.node_identity(),
                    VegetationMeshLodNode {
                        level: lod.level,
                        part_id: part.part_id,
                        billboard: part.billboard.is_some(),
                    },
                    VegetationMeshRenderIdentity {
                        layer,
                        prototype: prototype.id,
                        level: lod.level,
                        part_id: part.part_id,
                        billboard: part.billboard.is_some(),
                    },
                    VegetationMeshBatchNode {
                        authored_range: lod.visibility_range.clone(),
                        farthest_lod: lod_index + 1 == prototype.lods.len(),
                        local_transform: part.local_transform,
                        distance_offset: if lod.visibility_range.use_aabb {
                            part.local_transform.to_matrix().transform_point3(
                                part.culling_aabb
                                    .map(|aabb| Vec3::from(aabb.center))
                                    .unwrap_or(Vec3::ZERO),
                            ) // 测距锚点与旧逐实例节点的“部件局部变换后AABB中心”保持一致。
                        } else {
                            Vec3::ZERO
                        },
                        vertex_mode: part.batch_vertex_mode,
                    },
                    {
                        let mut input = VegetationMeshBatchInstances::empty(part.batch_vertex_mode);
                        input.volume = volume.clone();
                        input
                    },
                    volume
                        .as_ref()
                        .map_or(Transform::IDENTITY, |_| part.local_transform), // 逐株矩阵已包含部件局部变换，ECS节点自身保持批次原点。
                    VisibilityRange {
                        start_margin: 0.0..0.0,
                        end_margin: (f32::MAX * 0.5)..f32::MAX,
                        use_aabb: false,
                    }, // Bevy只给非abrupt范围启用LOD渐变Shader。节点保持全距离可见，真实范围由逐株实例流和当前GPU镜头决定。
                    volume.as_ref().map_or_else(
                        || batch_part_aabb(&placements, part.local_transform, part.culling_aabb),
                        |v| Aabb::from_min_max(v.min - Vec3::splat(32.), v.max + Vec3::splat(32.)),
                    ),
                    NoAutoAabb,
                    NoAutomaticBatching,
                ));
                part.material.insert_into(&mut child);
                if let Some(billboard) = part.billboard {
                    let gpu_material_ready = !billboard.requires_camera()
                        || part
                            .material
                            .is_material::<VegetationMeshBillboardMaterial>();
                    child.insert(VegetationMeshBillboardNode {
                        billboard,
                        rest_rotation: part.local_transform.rotation,
                        gpu_material_ready,
                    });
                }
                if !part.casts_shadow {
                    child.insert(NotShadowCaster);
                }
                if let Some(wind) = part.wind {
                    child.insert(VegetationMeshWind(wind));
                }
            }
        }
    });
    root
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::VegetationPrototypeId;
    use crate::mesh_backend::components::VegetationMeshInstance;
    use crate::mesh_backend::types::{
        VegetationMeshBatchVertexMode, VegetationMeshLod, VegetationMeshLodLevel,
        VegetationMeshPart,
    };

    fn part(part_id: u32) -> VegetationMeshPart {
        VegetationMeshPart {
            part_id,
            mesh: Handle::default(),
            material: Handle::<StandardMaterial>::default().into(),
            local_transform: Transform::IDENTITY,
            culling_aabb: Some(Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0))),
            casts_shadow: true,
            billboard: None,
            wind: None,
            batch_vertex_mode: VegetationMeshBatchVertexMode::Static,
        }
    }

    /// 锁定作者数量与 ECS 数量彻底解耦：超过旧 8192 数值也只生成固定批次节点。
    #[test]
    fn ten_thousand_authored_instances_create_one_batch_and_fixed_render_nodes() {
        const INSTANCE_COUNT: usize = 10_000;
        let mut farthest_part = part(2);
        farthest_part.local_transform = Transform::from_xyz(3.0, 0.0, 0.0);
        let mut farthest_range = VisibilityRange::abrupt(80.0, 300.0);
        farthest_range.use_aabb = true;
        let prototype = VegetationMeshPrototype {
            id: VegetationPrototypeId(71),
            lods: vec![
                VegetationMeshLod {
                    level: VegetationMeshLodLevel(0),
                    visibility_range: VisibilityRange::abrupt(0.0, 100.0),
                    parts: vec![part(0), part(1)],
                },
                VegetationMeshLod {
                    level: VegetationMeshLodLevel(1),
                    visibility_range: farthest_range,
                    parts: vec![farthest_part],
                },
            ],
        };
        let mut placements = (0..INSTANCE_COUNT)
            .map(|index| {
                VegetationMeshBatchPlacement::new(
                    index as u64,
                    Transform::from_xyz(index as f32 * 0.25, 0.0, 0.0),
                )
            })
            .collect();
        let mut app = App::new();
        let root = app
            .world_mut()
            .run_system_once(move |mut commands: Commands| {
                spawn_vegetation_mesh_batch(
                    &mut commands,
                    VegetationLayerId(70),
                    &prototype,
                    9,
                    std::mem::take(&mut placements),
                    Transform::IDENTITY,
                )
            })
            .unwrap();

        let batch = app
            .world()
            .entity(root)
            .get::<VegetationMeshBatch>()
            .unwrap();
        assert_eq!(batch.instance_count, INSTANCE_COUNT);
        assert_eq!(
            app.world()
                .entity(root)
                .get::<VegetationMeshBatchPlacements>()
                .unwrap()
                .0
                .len(),
            INSTANCE_COUNT,
        );
        let (node_count, farthest_node_count, farthest_offset, per_tree_root_count) = {
            let world = app.world_mut();
            let mut nodes = world.query::<&VegetationMeshBatchNode>();
            let (node_count, farthest_node_count, farthest_offset) = nodes.iter(world).fold(
                (0usize, 0usize, None),
                |(total, farthest, offset), node| {
                    (
                        total + 1,
                        farthest + usize::from(node.farthest_lod),
                        node.farthest_lod.then_some(node.distance_offset).or(offset),
                    )
                },
            );
            let mut per_tree_roots = world.query::<&VegetationMeshInstance>();
            (
                node_count,
                farthest_node_count,
                farthest_offset,
                per_tree_roots.iter(world).count(),
            )
        };
        assert_eq!(node_count, 3);
        assert_eq!(farthest_node_count, 1); // 最远身份取原型最后一档，不依赖档位编号必须等于3。
        assert_eq!(farthest_offset, Some(Vec3::new(3.0, 0.0, 0.0))); // AABB测距包含部件局部变换。
        assert_eq!(per_tree_root_count, 0);
        for range in app.world_mut().query::<&VisibilityRange>().iter(app.world()) {
            assert!(!range.is_abrupt(), "批次必须启用Bevy主通道、预通道与阴影的LOD渐变接口");
            assert!(range.is_visible_at_all(0.0) && range.is_visible_at_all(10_000.0),
                "整块范围不能先把近处或远处的逐株LOD候选裁掉");
        }
    }
}
