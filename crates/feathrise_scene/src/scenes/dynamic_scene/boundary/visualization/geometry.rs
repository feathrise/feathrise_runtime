//! 把已采用的空气墙碰撞规格转为有界调试网格；不读取作者草稿或创建物理组件。
use super::super::derivation::{Chunk, ChunkKey}; // 显示与实际碰撞使用同一份完整规格。
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
}; // CPU构造后仅保留GPU载荷。
use std::{collections::BTreeMap, ops::Bound}; // 按空间键继续，不扫描已处理前缀。
pub(super) const PARTS_PER_BATCH: usize = 256; // 每次后台工作与上传均限制在约0.25MiB。
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Cursor {
    pub key: Option<ChunkKey>,
    pub part: usize,
} // 空间块及块内碰撞段游标。
pub(super) struct Batch {
    pub mesh: Option<Mesh>,
    pub next: Option<Cursor>,
} // 一个完整可上传批次。
pub(super) fn build(chunks: &BTreeMap<ChunkKey, std::sync::Arc<Chunk>>, cursor: Cursor) -> Batch {
    let cube = Mesh::from(Cuboid::from_size(Vec3::ONE)); // 使用Bevy标准面顺序和法线，避免自制三角绕序错误。
    let positions = cube
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(VertexAttributeValues::as_float3)
        .expect("Cuboid位置");
    let normals = cube
        .attribute(Mesh::ATTRIBUTE_NORMAL)
        .and_then(VertexAttributeValues::as_float3)
        .expect("Cuboid法线");
    let cube_indices = cube.indices().expect("Cuboid索引"); // 每个盒体24顶点、36索引。
    let mut vertices = Vec::<[f32; 3]>::new();
    let mut directions = Vec::<[f32; 3]>::new();
    let mut indices = Vec::<u32>::new(); // 仅分配当前批次。
    let mut count = 0;
    let mut next = None; // 下一个未处理段，不以墙数据规模分配集合。
    let lower = cursor.key.map_or(Bound::Unbounded, Bound::Included);
    'chunks: for (key, chunk) in chunks.range((lower, Bound::Unbounded)) {
        let start = if cursor.key == Some(*key) {
            cursor.part
        } else {
            0
        }; // 新块从首段开始。
        for (part, spec) in chunk.specs.iter().enumerate().skip(start) {
            if count == PARTS_PER_BATCH {
                next = Some(Cursor {
                    key: Some(*key),
                    part,
                });
                break 'chunks;
            } // 完整段边界暂停。
            count += 1; // 隐藏段也占用批次预算，避免一次扫描整个场景。

            let base = vertices.len() as u32; // 批次固定预算保证u32索引安全。
            for (position, normal) in positions.iter().zip(normals) {
                vertices.push(
                    (spec.transform.rotation * (Vec3::from_array(*position) * spec.size)
                        + spec.transform.translation)
                        .to_array(),
                ); // 与碰撞盒相同的宽高、转角与封缝。
                directions.push((spec.transform.rotation * Vec3::from_array(*normal)).to_array()); // 世界朝向，避免非均匀缩放错误影响法线。
            }
            indices.extend(cube_indices.iter().map(|index| base + index as u32)); // 每段保留标准盒体拓扑。
        }
    }
    let mesh = (!vertices.is_empty()).then(|| {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vertices)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, directions)
        .with_inserted_indices(Indices::U32(indices))
    }); // 无纹理的unlit材质不需要UV。
    Batch { mesh, next } // 不包含Collider，显示开关无法改变角色阻挡。
}
#[cfg(test)]
mod tests {
    use super::super::super::helpers::BoundaryColliderSpec;
    use super::*;
    #[test]
    fn mesh_matches_collision_extents_and_batches_without_dropping_parts() {
        let spec = BoundaryColliderSpec {
            name: "wall".into(),
            size: Vec3::new(8.0, 10.0, 2.0),
            transform: Transform::from_xyz(20.0, 5.0, -4.0),
        }; // 已生效碰撞范围。
        let chunk = Chunk {
            specs: vec![spec; PARTS_PER_BATCH + 1],
            collider: avian3d::prelude::Collider::cuboid(1.0, 1.0, 1.0),
        }; // 测试只读取spec。
        let chunks = BTreeMap::from([((0, 0), std::sync::Arc::new(chunk))]);
        let first = build(&chunks, Cursor::default());
        let mesh = first.mesh.unwrap();
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .unwrap();
        assert_eq!(positions.len(), PARTS_PER_BATCH * 24); // 固定上传预算。
        let (min, max) = positions.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), p| (min.min(Vec3::from_array(*p)), max.max(Vec3::from_array(*p))),
        );
        assert_eq!(min, Vec3::new(16.0, 0.0, -5.0));
        assert_eq!(max, Vec3::new(24.0, 10.0, -3.0)); // 高度与厚度均真实显示。
        let second = build(&chunks, first.next.unwrap());
        assert_eq!(second.mesh.unwrap().count_vertices(), 24);
        assert!(second.next.is_none()); // 最后一段不遗漏。
        assert!(build(&BTreeMap::new(), Cursor::default()).mesh.is_none()); // 无墙不生成空Mesh。
    }
}
