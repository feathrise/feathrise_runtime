//! 共享原形状弱票据在后台提取真实三角、减面和烘焙，保持缩放与内边缘约束。
use super::super::resources::{CACHE_BYTES, SOURCE_BYTES};
use super::types::{Built, Level};
use crate::shared::placement_data::StaticTerrainShape;
use avian3d::{
    parry::shape::{SharedShape, TriMesh},
    prelude::*,
};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
};
use std::sync::Arc;
pub(super) fn source_mesh(collider: &Collider) -> Option<&TriMesh> {
    collider.shape().as_trimesh().or_else(|| {
        collider
            .shape()
            .as_shape::<StaticTerrainShape>()
            .map(StaticTerrainShape::mesh)
    })
}

pub(super) fn cook(original: Collider) -> Result<Built, String> {
    let lease = Arc::downgrade(&original.shape().0); // 弱租约阻止地址ABA，不强留已卸载场景的原始BVH。
    let source =
        source_mesh(&original).ok_or("碰撞LOD只适配真实静态Trimesh；其他形状继续原始碰撞")?;
    if source
        .vertices()
        .len()
        .saturating_mul(12)
        .saturating_add(source.indices().len().saturating_mul(12))
        > SOURCE_BYTES
    {
        return Err("静态碰撞源超过32MiB后台工作信用，保留原始碰撞".into());
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        source
            .vertices()
            .iter()
            .map(|point| point.to_array())
            .collect::<Vec<_>>(),
    );
    mesh.insert_indices(Indices::U32(
        source
            .indices()
            .iter()
            .flat_map(|triangle| triangle.iter().copied())
            .collect(),
    ));
    let mut bytes = 0usize;
    let mut levels = Vec::new();
    for lod in feathrise_terrain::build_static_mesh_lods(mesh)? {
        let Some(VertexAttributeValues::Float32x3(positions)) =
            lod.mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return Err("碰撞LOD缺少真实位置属性".into());
        };
        let indices = lod
            .mesh
            .indices()
            .ok_or("碰撞LOD缺少真实三角索引")?
            .iter()
            .map(|index| index as u32)
            .collect::<Vec<_>>();
        // BVH和法线约束也占用CPU信用；真实非单位Collider还保存独立scaled_shape，必须为两套几何预留信用。
        // 该预算只拒绝派生，不删除原始承托。
        let copies = if original.scale() == Vec3::ONE { 1 } else { 2 };
        bytes = bytes
            .saturating_add(positions.len().saturating_mul(32).saturating_mul(copies))
            .saturating_add(
                (indices.len() / 3)
                    .saturating_mul(128)
                    .saturating_mul(copies),
            );
        if bytes > CACHE_BYTES {
            return Err("碰撞LOD派生超过64MiB工作信用，保留原始碰撞".into());
        }
        let mesh = TriMesh::with_flags(
            positions.iter().copied().map(Vec3::from_array).collect(),
            indices
                .chunks_exact(3)
                .map(|triangle| [triangle[0], triangle[1], triangle[2]])
                .collect(),
            source.flags(),
        )
        .map_err(|error| format!("碰撞LOD三角结构无效：{error}"))?;
        let mut collider = Collider::from(SharedShape::new(StaticTerrainShape::new(mesh)));
        collider.set_scale(original.scale(), 10); // 缩放大BVH与包围球只在worker执行，采用帧使用缓存元数据。
        levels.push(Level {
            collider,
            error: lod.max_error * original.scale().abs().max_element(),
        });
    }
    Ok(Built {
        levels: levels.into(),
        bytes,
        source: lease,
    })
}
