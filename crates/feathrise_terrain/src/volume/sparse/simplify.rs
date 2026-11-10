//! 每个脏块在后台简化一次；锁定开边和共享接缝，所有消费者使用同一份紧凑网格。
use crate::TerrainMeshInput;
use bevy::math::Vec3;
use meshopt::{SimplifyOptions, VertexDataAdapter};
use std::collections::HashMap;

pub(super) fn simplify_chunk_mesh(
    mesh: TerrainMeshInput,
    cell_size: f32,
) -> Result<TerrainMeshInput, String> {
    simplify_mesh(mesh, cell_size / 16., false)
}

/// 远处代理也保持开边和洞口/洞底高度极值；不能用删除组件或丢三角来换预算。
pub fn simplify_display_mesh(
    mesh: TerrainMeshInput,
    error: f32,
) -> Result<TerrainMeshInput, String> {
    simplify_mesh(mesh, error, true)
}

fn simplify_mesh(
    mesh: TerrainMeshInput,
    target_error: f32,
    preserve_features: bool,
) -> Result<TerrainMeshInput, String> {
    if mesh.indices.is_empty() {
        return Ok(mesh);
    }
    // 体素精度仍由原密度决定。作者块配方仍使用格距1/16；显示代理使用自己的版本化误差额度。
    // 不使用Sloppy/Prune，不删除洞穴组件。实际开边的一环保留全部原顶点及三角关系，
    // 避免两侧独立简化新增相同边界弦，聚合焊接后形成四面共用一条边的非流形结果。
    // 作者块固定meshopt 0.6.2及参数，修改需增加MeshRecipe；代理配方另由显示缓存版本约束。
    let vertices = VertexDataAdapter::new(bytemuck::cast_slice(&mesh.positions), 12, 0)
        .map_err(|e| format!("三维网格简化输入无效：{e}"))?;
    let original_edges = edge_counts(&mesh.indices);
    let mut border = vec![false; mesh.positions.len()];
    for (&[a, b], &count) in &original_edges {
        if count == 1 {
            border[a as usize] = true;
            border[b as usize] = true;
        }
    }
    let mut locks = border.clone();
    if preserve_features {
        let mut lower = vec![false; mesh.positions.len()];
        let mut higher = lower.clone();
        for tri in mesh.indices.chunks_exact(3) {
            for &a in tri {
                for &b in tri {
                    let delta = mesh.positions[b as usize][1] - mesh.positions[a as usize][1];
                    lower[a as usize] |= delta < -1e-6;
                    higher[a as usize] |= delta > 1e-6;
                }
            }
        }
        for i in 0..locks.len() {
            locks[i] |= lower[i] != higher[i];
        }
    }
    for tri in mesh.indices.chunks_exact(3) {
        if tri.iter().any(|i| border[*i as usize]) {
            for &i in tri {
                locks[i as usize] = true;
            }
        }
    }
    let mut error = 0.;
    let indices = meshopt::simplify_with_locks(
        &mesh.indices,
        &vertices,
        &locks,
        0,
        target_error,
        SimplifyOptions::LockBorder | SimplifyOptions::ErrorAbsolute,
        Some(&mut error),
    );
    if !error.is_finite()
        || error > target_error + 1e-7
        || indices.is_empty()
        || !indices.len().is_multiple_of(3)
        || indices.len() > mesh.indices.len()
        || indices.iter().any(|i| *i as usize >= mesh.positions.len())
    {
        return Err("三维网格简化结果或误差指标无效，保留上一完整地形".into());
    }
    // 近乎相切的洞壁可能使通用简化器产生重复边；这类块复用原等值面，不影响其他块减面。
    // 保持原开边集合和每条内部边二面相邻，不能为了数量改善把非流形几何交给碰撞/光追。
    if !preserves_edges(&original_edges, &indices)
        || indices.chunks_exact(3).any(|tri| {
            let p = [tri[0], tri[1], tri[2]].map(|i| Vec3::from_array(mesh.positions[i as usize]));
            (p[1] - p[0]).cross(p[2] - p[0]).length_squared() <= 1e-16
        })
    {
        return Ok(mesh);
    }
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    for &i in &indices {
        remap[i as usize] = 0;
    }
    let mut positions = Vec::with_capacity(remap.iter().filter(|i| **i == 0).count());
    for (i, p) in mesh.positions.into_iter().enumerate() {
        if remap[i] == 0 {
            remap[i] = positions.len() as u32;
            positions.push(p);
        }
    }
    // simplify保留原索引容量，不能把已经移除的三角形空间继续记入64MiB缓存/历史。
    let mut indices = indices.into_boxed_slice().into_vec();
    for i in &mut indices {
        *i = remap[*i as usize];
    }
    Ok(TerrainMeshInput { positions, indices })
}

fn edge_counts(indices: &[u32]) -> HashMap<[u32; 2], usize> {
    let mut edges = HashMap::new();
    for tri in indices.chunks_exact(3) {
        for [a, b] in [[tri[0], tri[1]], [tri[1], tri[2]], [tri[2], tri[0]]] {
            *edges.entry([a.min(b), a.max(b)]).or_insert(0) += 1;
        }
    }
    edges
}

fn preserves_edges(original: &HashMap<[u32; 2], usize>, indices: &[u32]) -> bool {
    if indices
        .chunks_exact(3)
        .any(|t| t[0] == t[1] || t[1] == t[2] || t[2] == t[0])
    {
        return false;
    }
    let edges = edge_counts(indices);
    edges
        .iter()
        .all(|(edge, n)| *n == 2 || (*n == 1 && original.get(edge) == Some(&1)))
        && original
            .iter()
            .all(|(edge, n)| *n != 1 || edges.get(edge) == Some(&1))
}
