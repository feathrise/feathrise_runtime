//! 只在后台把有限 Mip/实时覆盖/浅坑采样为真实世界高度 Mesh，保留固定三角编号。

use super::{
    constants::MAX_PROXY_CELLS,
    types::{ProxyCapture, ProxyGrid},
};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use feathrise_map_content::{SparseHeightSpec, sample_crater_height};

#[cfg(test)]
pub(super) fn choose_grid(spec: &SparseHeightSpec) -> Result<ProxyGrid, String> {
    choose_grid_for_holes(spec, false, false)
}

pub(super) fn choose_grid_for_holes(
    spec: &SparseHeightSpec,
    holes: bool,
    stitches: bool,
) -> Result<ProxyGrid, String> {
    for level in 0..=spec.max_mip_level() {
        let cells = spec.mip_cells(level).ok_or("光追高度层级不存在")?;
        if cells[0].max(cells[1]) <= match (holes,stitches) {(true,true)=>128,(_,true)|(true,false)=>256,_=>MAX_PROXY_CELLS} {
            return Ok(ProxyGrid {
                level,
                cells: [cells[0] as u32, cells[1] as u32],
            });
        }
    }
    Err("稀疏高度规格超出光追代理格数预算".into())
}

fn height_at_level(input: &ProxyCapture, level: u8, x: u64, z: u64) -> Result<f32, String> {
    let base = input
        .spec
        .mip_grid_point_to_base(level, x, z)
        .ok_or("光追采样超出地图边界")?;
    let (base_key, local) = input
        .spec
        .tile_for_grid_point(base[0], base[1])
        .ok_or("光追格点无法定位作者页")?;
    let height = if let Some(tile) = input.live.get(&base_key) {
        input
            .spec
            .decode_height(tile.get_height(&input.spec, local).ok_or("光追实时页形状不符")?)
    } else {
        let (key, local) = input
            .spec
            .mip_tile_for_grid_point(level, x, z)
            .ok_or("光追格点无法定位持久父页")?;
        match input.formal.get(&key) {
            Some(tile) => input
                .spec
                .decode_height(tile.get_height(&input.spec, local).ok_or("光追父页形状不符")?),
            None => 0.0, // 仅清单未登记的合法空页解释为零；登记页缺失在 IO 阶段拒绝。
        }
    };
    let height = sample_crater_height(&input.spec, &input.craters, base, height);
    height
        .is_finite()
        .then_some(height)
        .ok_or_else(|| "光追地形出现非有限高度".into())
}

/// 洞口存在时选更粗的有限代理，为矩形边界预留格数与上传预算。
pub(super) fn build_proxy(input: &ProxyCapture) -> Result<super::types::PreparedProxy, String> {
    let mut grid = input.grid;
    let constrained_limit=match (!input.holes.is_empty(),input.stitches.active()) {(true,true)=>128,(_,true)|(true,false)=>256,_=>MAX_PROXY_CELLS as u32};
    if grid.cells[0].max(grid.cells[1])>constrained_limit {
        while grid.cells[0].max(grid.cells[1]) > constrained_limit {
            grid.level += 1;
            let cells = input
                .spec
                .mip_cells(grid.level)
                .ok_or("洞口光追代理层级无效")?;
            grid.cells = [cells[0] as u32, cells[1] as u32];
        }
    }
    let [nx, nz] = grid.cells;
    if nx == 0 || nz == 0 || u64::from(nx.max(nz)) > MAX_PROXY_CELLS || input.holes.len() > 32 {
        return Err("光追代理格数或洞口超过预算".into());
    }
    let regular_axis = |axis: usize, count: u32| -> Result<Vec<f32>, String> {
        (0..=count)
            .map(|i| {
                let base = input
                    .spec
                    .mip_grid_point_to_base(
                        grid.level,
                        if axis == 0 { u64::from(i) } else { 0 },
                        if axis == 1 { u64::from(i) } else { 0 },
                    )
                    .ok_or("光追代理轴超出地图")?;
                Ok(input
                    .spec
                    .grid_point_world(base[0], base[1])
                    .ok_or("光追代理轴超出地图")?[axis] as f32)
            })
            .collect()
    };
    let regular_x = regular_axis(0, nx)?;
    let regular_z = regular_axis(1, nz)?;
    let expand = |regular: &[f32], axis: usize| {
        let mut values = regular.to_vec();
        for hole in input.holes.iter() {
            values.extend([hole.min[axis], hole.max[axis]]);
        }
        // 共同曲线的全部折点进入固定代理轴；先给常规格降级预留容量，不按两侧Mip各自近似。
        for e in 0..4 {
            let info=input.stitches.edges[e];let tangent=if e<2 {1}else{0};
            if info.x>=2.0&&tangent==axis {
                values.extend((0..info.x as usize).map(|i|(input.spec.world_min[axis]+i as f32*info.y).min(input.spec.world_max[axis])));
            }
        }
        values.sort_by(f32::total_cmp);
        values.dedup();
        values
    };
    let x = expand(&regular_x, 0);
    let z = expand(&regular_z, 1);
    if x.len() > 513
        || z.len() > 513
        || x.iter().chain(&z).any(|v| !v.is_finite())
        || x[0] < input.spec.world_min[0]
        || *x.last().unwrap() > input.spec.world_max[0]
        || z[0] < input.spec.world_min[1]
        || *z.last().unwrap() > input.spec.world_max[1]
    {
        return Err("光追洞口轴无效或超预算".into());
    }
    let mut coarse = Vec::with_capacity((nx as usize + 1) * (nz as usize + 1));
    for iz in 0..=nz {
        for ix in 0..=nx {
            coarse.push(height_at_level(
                input,
                grid.level,
                u64::from(ix),
                u64::from(iz),
            )?);
        }
    }
    let locate = |axis: &[f32], v: f32| {
        axis.partition_point(|p| *p <= v)
            .saturating_sub(1)
            .min(axis.len() - 2)
    };
    let mut positions = Vec::with_capacity(x.len() * z.len());
    for wz in &z {
        for wx in &x {
            let ix = locate(&regular_x, *wx);
            let iz = locate(&regular_z, *wz);
            let u = (*wx - regular_x[ix]) / (regular_x[ix + 1] - regular_x[ix]);
            let v = (*wz - regular_z[iz]) / (regular_z[iz + 1] - regular_z[iz]);
            let a = iz * (nx as usize + 1) + ix;
            let h = if u + v <= 1.0 {
                coarse[a] * (1.0 - u - v) + coarse[a + 1] * u + coarse[a + nx as usize + 1] * v
            } else {
                coarse[a + 1] * (1.0 - v)
                    + coarse[a + nx as usize + 1] * (1.0 - u)
                    + coarse[a + nx as usize + 2] * (u + v - 1.0)
            };
            let h=input.stitches.height(Vec2::new(*wx,*wz),h,Vec2::from_array(input.spec.world_min),Vec2::from_array(input.spec.world_max));
            positions.push([*wx, h, *wz]);
        }
    }
    let cells = [x.len() as u32 - 1, z.len() as u32 - 1];
    if u64::from(cells[0]) * u64::from(cells[1]) * 2 > 524_288 {
        return Err("洞口代理超过19位三角编号预算".into());
    }
    let mut indices = Vec::with_capacity(cells[0] as usize * cells[1] as usize * 6);
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for iz in 0..cells[1] {
        for ix in 0..cells[0] {
            let a = iz * (cells[0] + 1) + ix;
            let b = a + 1;
            let c = a + cells[0] + 1;
            let d = c + 1;
            let center = [
                (x[ix as usize] + x[ix as usize + 1]) * 0.5,
                (z[iz as usize] + z[iz as usize + 1]) * 0.5,
            ];
            if input
                .holes
                .iter()
                .any(|hole| hole.contains(Vec2::from_array(center)))
            {
                // BLAS忽略零面积占位，索引槽保持原号；Deferred不会因洞口压缩编号而错指其他三角。
                indices.extend([a, a, a, a, a, a]);
                continue;
            }
            for triangle in [[a, c, b], [b, c, d]] {
                let p = triangle.map(|i| Vec3::from_array(positions[i as usize]));
                let normal = (p[1] - p[0]).cross(p[2] - p[0]);
                if !normal.is_finite() || normal.length_squared() <= f32::EPSILON * f32::EPSILON {
                    return Err("洞口外光追代理含退化三角形".into());
                }
                for index in triangle {
                    normals[index as usize] += normal;
                }
                indices.extend(triangle);
            }
        }
    }
    let normals = normals
        .into_iter()
        .map(|n| n.normalize_or(Vec3::Y).to_array())
        .collect::<Vec<_>>();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    Ok(super::types::PreparedProxy {
        mesh,
        grid: ProxyGrid {
            level: grid.level,
            cells,
        },
        axes: super::super::material::ClipmapProxyAxes::new(&x, &z),
    })
}

#[cfg(test)]
pub(super) fn build_mesh(input: &ProxyCapture) -> Result<Mesh, String> {
    Ok(build_proxy(input)?.mesh)
}

pub(super) fn validate_live_shape(
    input: &ProxyCapture,
    tile: &feathrise_map_content::SparseHeightTile,
) -> Result<(), String> {
    if input.spec.tile_shape(tile.key) != Some(tile.shape)
        || tile.height_codes(&input.spec).len() != usize::from(tile.shape[0]) * usize::from(tile.shape[1])
    {
        return Err("光追实时覆盖页与当前规格不一致".into());
    }
    Ok(())
}

/// 纯函数对应生产 WGSL，回归短尾格、对角线和地图最外端的索引合同。
#[cfg(test)]
pub(super) fn triangle_at(spec: &SparseHeightSpec, grid: ProxyGrid, point: Vec2) -> u32 {
    let logical = ((point - Vec2::from_array(spec.world_min))
        / Vec2::new(
            spec.actual_spacing_meters[0] as f32,
            spec.actual_spacing_meters[1] as f32,
        ))
    .clamp(
        Vec2::ZERO,
        Vec2::new(spec.cells[0] as f32, spec.cells[1] as f32),
    );
    let stride = (1u32 << grid.level) as f32;
    let cell = (logical / stride)
        .floor()
        .as_uvec2()
        .min(UVec2::from_array(grid.cells) - UVec2::ONE);
    let lower = cell.as_vec2() * stride;
    let upper = ((cell + UVec2::ONE).as_vec2() * stride)
        .min(Vec2::new(spec.cells[0] as f32, spec.cells[1] as f32));
    let fraction = (logical - lower) / (upper - lower);
    2 * (cell.y * grid.cells[0] + cell.x) + u32::from(fraction.x + fraction.y > 1.0)
}
