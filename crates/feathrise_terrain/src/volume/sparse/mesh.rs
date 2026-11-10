//! 不变区块复用网格缓存；仅后台聚合一个地形primitive，避免每块占用一个光追实例。
use super::types::*;
use crate::{TerrainMeshInput, volume::build_volume_mesh};
use std::collections::HashMap;

pub fn build_author_volume_mesh(source: &VolumeSource) -> Result<TerrainMeshInput, String> {
    if matches!(source, VolumeSource::Paged(_)) {
        return Err("流送作者版本必须按驻留窗口读取网格，不能聚合整层".into());
    }
    let VolumeSource::Sparse(data) = source else {
        let VolumeSource::Dense(data) = source else {
            unreachable!()
        };
        return build_volume_mesh(data);
    };
    data.validate()?;
    let mut mesh = TerrainMeshInput {
        positions: Vec::new(),
        indices: Vec::new(),
    };
    let mut vertices = HashMap::new();
    for block in data.meshes.values() {
        let mut remap = Vec::with_capacity(block.positions.len());
        for &p in &block.positions {
            let key = p.map(|v| if v == 0. { 0 } else { v.to_bits() });
            let index = if let Some(&i) = vertices.get(&key) {
                i
            } else {
                if mesh.positions.len() == MAX_SPARSE_VERTICES {
                    return Err("活动三维地形超过262144顶点碰撞预算，保留上一完整地形".into());
                }
                let i = mesh.positions.len() as u32;
                mesh.positions.push(p);
                vertices.insert(key, i);
                i
            };
            remap.push(index);
        }
        if mesh.indices.len() + block.indices.len() > MAX_SPARSE_TRIANGLES * 3 {
            return Err("活动三维地形超过262144三角碰撞预算，保留上一完整地形".into());
        }
        mesh.indices
            .extend(block.indices.iter().map(|i| remap[*i as usize]));
    }
    if mesh.indices.is_empty() {
        return Err("分块三维地形没有可用表面".into());
    }
    Ok(mesh)
}
