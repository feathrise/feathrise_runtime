//! 八个内部块共享样本及全局边交点；等值面保持洞底/洞顶且不产生分块裂缝。
use super::types::*;
use crate::TerrainMeshInput;
use bevy::math::Vec3;
use std::collections::HashMap;

const CORNERS: [[usize; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [0, 1, 0],
    [1, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [0, 1, 1],
    [1, 1, 1],
];
// 所有立方体使用000→111体对角线；邻块共享面的对角线完全相同。
const TETS: [[usize; 4]; 6] = [
    [0, 1, 3, 7],
    [0, 3, 2, 7],
    [0, 2, 6, 7],
    [0, 6, 4, 7],
    [0, 4, 5, 7],
    [0, 5, 1, 7],
];

pub fn build_volume_mesh(data: &VolumeData) -> Result<TerrainMeshInput, String> {
    data.validate()?;
    extract(data, Vec3::ZERO, false, false)
}

/// 分块只持有共享密度角点；基础高度可能远在本块上方，不能套用旧盒子的高度容纳门。
pub fn build_density_mesh(
    cells: u32,
    step: f32,
    density: Vec<i16>,
    origin: Vec3,
) -> Result<TerrainMeshInput, String> {
    let data = VolumeData {
        cells,
        cell_size: step,
        density,
        base_heights: vec![cells as f32 * step / 2.0; (cells as usize + 1).pow(2)],
    };
    data.validate_lattice()?;
    if !origin.is_finite() { return Err("三维采样块原点必须有限".into()); }
    extract(&data, origin, true, true)
}

/// 【功能设计】LOD交界的约束插值不能重新量化成i16，否则相邻曲面会出现微缝。
/// 仍使用同一四面体、端点排序和容量合同；浮点入口只用于派生窗口，不是作者格式。
pub fn build_density_mesh_fractional(
    cells: u32,
    step: f32,
    density: Vec<f32>,
    origin: Vec3,
) -> Result<TerrainMeshInput, String> {
    let data = VolumeData {
        cells,
        cell_size: step,
        density: vec![0; (cells as usize + 1).pow(3)],
        base_heights: vec![cells as f32 * step / 2.0; (cells as usize + 1).pow(2)],
    };
    data.validate_lattice()?;
    if !origin.is_finite() || density.len() != data.density.len()
        || density.iter().any(|v| !v.is_finite()) {
        return Err("三维派生采样长度、原点或密度无效".into());
    }
    extract_samples(&data, origin, true, true, Some(&density))
}

fn extract(
    data: &VolumeData,
    origin: Vec3,
    canonical: bool,
    allow_empty: bool,
) -> Result<TerrainMeshInput, String> {
    extract_samples(data, origin, canonical, allow_empty, None)
}
fn extract_samples(
    data: &VolumeData,
    origin: Vec3,
    canonical: bool,
    allow_empty: bool,
    fractional: Option<&[f32]>,
) -> Result<TerrainMeshInput, String> {
    let mut builder = Builder {
        data,
        canonical,
        fractional,
        mesh: TerrainMeshInput {
            positions: Vec::new(),
            indices: Vec::new(),
        },
        edges: HashMap::new(),
    };
    let half = data.cells as usize / 2;
    let lattice_origin = (origin / data.cell_size).round();
    for bz in 0..2 {
        for by in 0..2 {
            for bx in 0..2 {
                for z in bz * half..(bz + 1) * half {
                    for y in by * half..(by + 1) * half {
                        for x in bx * half..(bx + 1) * half {
                            let ids = std::array::from_fn::<_, 8, _>(|i| {
                                data.index(x + CORNERS[i][0], y + CORNERS[i][1], z + CORNERS[i][2])
                                    as u32
                            });
                            let p = std::array::from_fn::<_, 8, _>(|i| {
                                let corner = Vec3::new(
                                    (x + CORNERS[i][0]) as f32,
                                    (y + CORNERS[i][1]) as f32,
                                    (z + CORNERS[i][2]) as f32,
                                );
                                if canonical {
                                    (lattice_origin + corner) * data.cell_size
                                } else {
                                    corner * data.cell_size + origin
                                }
                            });
                            if ids.iter().all(|i| builder.value(*i) >= 0.0)
                                || ids.iter().all(|i| builder.value(*i) < 0.0)
                            {
                                continue;
                            }
                            for tet in TETS {
                                builder.tetrahedron(tet.map(|i| ids[i]), tet.map(|i| p[i]))?;
                            }
                        }
                    }
                }
            }
        }
    }
    if builder.mesh.indices.is_empty() && !allow_empty {
        return Err("三维区域没有可用表面".into());
    }
    // 孤立零密度点只产生零面积候选，不是一个曲面；压紧其未引用顶点，保持GLB完整拓扑契约。
    let mut remap = vec![u32::MAX; builder.mesh.positions.len()];
    for &index in &builder.mesh.indices {
        remap[index as usize] = 0;
    }
    let mut positions = Vec::with_capacity(builder.mesh.positions.len());
    for (old, point) in builder.mesh.positions.into_iter().enumerate() {
        if remap[old] != u32::MAX {
            remap[old] = positions.len() as u32;
            positions.push(point);
        }
    }
    for index in &mut builder.mesh.indices {
        *index = remap[*index as usize];
    }
    builder.mesh.positions = positions;
    Ok(builder.mesh)
}

struct Builder<'a> {
    data: &'a VolumeData,
    canonical: bool,
    fractional: Option<&'a [f32]>,
    mesh: TerrainMeshInput,
    edges: HashMap<(u32, u32), u32>,
}
impl Builder<'_> {
    fn value(&self, index: u32) -> f32 {
        self.fractional.map_or_else(|| self.data.density[index as usize] as f32, |values| values[index as usize])
    }
    fn vertex(&mut self, a: u32, b: u32, pa: Vec3, pb: Vec3) -> Result<u32, String> {
        // 新分块在同一世界坐标中按同一端点顺序插值，避免邻块反向计算产生浮点裂缝。
        // v1保持原顺序，以便旧GLB仍能通过源/网格摘要校验。
        let (a, b, pa, pb) = if self.canonical
            && pa
                .to_array()
                .iter()
                .zip(pb.to_array())
                .find_map(|(a, b)| (*a != b).then(|| a.total_cmp(&b)))
                .is_some_and(|order| order.is_gt())
        {
            (b, a, pb, pa)
        } else {
            (a, b, pa, pb)
        };
        let (va, vb) = (
            self.value(a),
            self.value(b),
        );
        // 零点端点共享单一样本ID，避免平地穿过格点时产生重合但不焊接的裂缝。
        let key = if va == 0.0 {
            (a, a)
        } else if vb == 0.0 {
            (b, b)
        } else {
            (a.min(b), a.max(b))
        };
        if let Some(index) = self.edges.get(&key) {
            return Ok(*index);
        }
        if self.mesh.positions.len() >= MAX_VOLUME_VERTICES {
            return Err("三维生成超过65536顶点原型预算".into());
        }
        let p = pa.lerp(pb, va / (va - vb));
        let index = self.mesh.positions.len() as u32;
        self.mesh.positions.push(p.to_array());
        self.edges.insert(key, index);
        Ok(index)
    }
    fn triangle(&mut self, mut indices: [u32; 3], toward_air: Vec3) -> Result<(), String> {
        let p = indices.map(|i| Vec3::from_array(self.mesh.positions[i as usize]));
        let normal = (p[1] - p[0]).cross(p[2] - p[0]);
        if normal.length_squared() <= 1e-16 {
            return Ok(());
        }
        if normal.dot(toward_air) < 0.0 {
            indices.swap(1, 2);
        }
        if self.mesh.indices.len() / 3 >= MAX_VOLUME_TRIANGLES {
            return Err("三维生成超过131072三角原型预算".into());
        }
        self.mesh.indices.extend(indices);
        Ok(())
    }
    fn tetrahedron(&mut self, ids: [u32; 4], p: [Vec3; 4]) -> Result<(), String> {
        let (mut inside, mut outside) = ([0; 4], [0; 4]);
        let (mut ni, mut no) = (0, 0);
        for i in 0..4 {
            if self.value(ids[i]) < 0.0 {
                inside[ni] = i;
                ni += 1;
            } else {
                outside[no] = i;
                no += 1;
            }
        }
        if ni == 0 || no == 0 {
            return Ok(());
        }
        let toward_air = outside[..no].iter().map(|&i| p[i]).sum::<Vec3>() / no as f32
            - inside[..ni].iter().map(|&i| p[i]).sum::<Vec3>() / ni as f32;
        let mut edge = |i: usize, o: usize| self.vertex(ids[i], ids[o], p[i], p[o]);
        if ni == 1 {
            let a = edge(inside[0], outside[0])?;
            let b = edge(inside[0], outside[1])?;
            let c = edge(inside[0], outside[2])?;
            self.triangle([a, b, c], toward_air)?;
        } else if no == 1 {
            let a = edge(inside[0], outside[0])?;
            let b = edge(inside[1], outside[0])?;
            let c = edge(inside[2], outside[0])?;
            self.triangle([a, b, c], toward_air)?;
        } else {
            let a = edge(inside[0], outside[0])?;
            let b = edge(inside[0], outside[1])?;
            let c = edge(inside[1], outside[1])?;
            let d = edge(inside[1], outside[0])?;
            self.triangle([a, b, c], toward_air)?;
            self.triangle([a, c, d], toward_air)?;
        }
        Ok(())
    }
}
