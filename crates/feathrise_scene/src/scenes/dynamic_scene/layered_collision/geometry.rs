//! 平衡叶的闭合边界三角化；粗边补入细层权威采样点，不用裙边或重叠地面遮缝。
use super::{
    planning::chunk_bounds,
    types::{ChunkGeometry, ChunkPlan},
};
use bevy::prelude::{Vec2, Vec3, Vec3Swizzles}; // 世界平面采样与碰撞三维顶点。
use feathrise_scene_content::sparse_height::SparseHeightSpec; // 作者格坐标权威。
use std::collections::BTreeMap; // 二倍基础格整数键避免浮点顶点去重误差。 // 不接触ECS或渲染。

pub(super) fn cut_holes(
    spec: &SparseHeightSpec,
    mesh: &mut ChunkGeometry,
    holes: &[feathrise_scene_content::SceneTerrainCutout],
) -> Result<(), String> {
    if holes.is_empty() || mesh.triangles.is_empty() {
        return Ok(());
    }
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for point in &mesh.vertices {
        min = min.min(point.xz());
        max = max.max(point.xz());
    }
    let relevant: Vec<_> = holes
        .iter()
        .copied()
        .filter(|hole| hole.intersects(min, max))
        .collect();
    if relevant.is_empty() {
        return Ok(());
    }
    let mut vertices: BTreeMap<[u32; 3], u32> = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, p)| (p.to_array().map(f32::to_bits), i as u32))
        .collect();
    let mut triangles = Vec::new();
    for triangle in &mesh.triangles {
        let points = triangle.map(|i| mesh.vertices[i as usize]);
        for output in feathrise_scene_content::subtract_terrain_cutouts(points, &relevant) {
            let mut indices = [0; 3];
            for (to, p) in indices.iter_mut().zip(output) {
                *to = match vertices.get(&p.to_array().map(f32::to_bits)) {
                    Some(index) => *index,
                    None => {
                        let index = mesh.vertices.len() as u32;
                        mesh.vertices.push(p);
                        mesh.base_grid_points.push([
                            (f64::from(p.x) - f64::from(spec.world_min[0]))
                                / spec.actual_spacing_meters[0],
                            (f64::from(p.z) - f64::from(spec.world_min[1]))
                                / spec.actual_spacing_meters[1],
                        ]);
                        vertices.insert(p.to_array().map(f32::to_bits), index);
                        index
                    }
                };
            }
            triangles.push(indices);
            if mesh.vertices.len() > 65_536 || triangles.len() > 131_072 {
                return Err("洞口裁切超过单块几何预算".into());
            }
        }
    }
    mesh.triangles = triangles;
    Ok(()) // 空块由布局阶段排除，不制造填洞的托底平面。
}

pub(super) fn build_chunk(
    spec: &SparseHeightSpec,
    chunk: &ChunkPlan,
    mut sample: impl FnMut(Vec2) -> Option<f32>,
) -> Result<ChunkGeometry, String> {
    // 世界坐标采样便利接口。
    build_chunk_grid(spec, chunk, |point| sample(world_point(spec, point))) // 正式运行时宜使用精确基础格接口。
} // 同一世界点必须从同一版本高度源采样。

pub(super) fn world_point(spec: &SparseHeightSpec, point: [f64; 2]) -> Vec2 {
    // 各层统一映射一次。
    Vec2::new(
        (f64::from(spec.world_min[0]) + point[0] * spec.actual_spacing_meters[0]) as f32,
        (f64::from(spec.world_min[1]) + point[1] * spec.actual_spacing_meters[1]) as f32,
    ) // 边界两侧得到逐位相同坐标。
} // 不使用各块独立插值原点。

pub(super) fn build_chunk_grid(
    spec: &SparseHeightSpec,
    chunk: &ChunkPlan,
    mut sample: impl FnMut([f64; 2]) -> Option<f32>,
) -> Result<ChunkGeometry, String> {
    // 精确基础格采样接口。
    let (lo, hi) = chunk_bounds(spec, chunk.key); // 裁剪后的真实场景覆盖。
    if lo[0] >= hi[0] || lo[1] >= hi[1] {
        return Err("碰撞块在场景边界之外".into());
    } // 不创建零面积刚体。
    let step = 1_u64 << chunk.key.level; // 作者格距倍率。
    if step == 1 && chunk.finer_edges != 0 {
        return Err("最细碰撞层不能再插入更细接缝".into());
    } // 防止损坏计划生成重复顶点。
    let mut mesh = ChunkGeometry {
        vertices: Vec::new(),
        base_grid_points: Vec::new(),
        triangles: Vec::new(),
    }; // 后台独占网格。
    let mut indices = BTreeMap::<[u64; 2], u32>::new(); // 键是两倍基础格坐标，支持尾格半整数中心。
    let mut vertex = |point: [u64; 2], mesh: &mut ChunkGeometry| -> Result<u32, String> {
        // 去重保证同块内部拓扑连通。
        if let Some(index) = indices.get(&point) {
            return Ok(*index);
        } // 已有边界点直接复用。
        let grid = [point[0] as f64 * 0.5, point[1] as f64 * 0.5]; // 精确二进制半整数。
        let height = sample(grid)
            .filter(|height| height.is_finite())
            .ok_or_else(|| "碰撞高度页未就绪或含非有限高度".to_string())?; // 不用零值掩盖缺页。
        let world = world_point(spec, grid); // 所有层共享同一坐标映射。
        let index = mesh.vertices.len() as u32; // 每块不足数千点。
        mesh.vertices.push(Vec3::new(world.x, height, world.y)); // 世界空间物理顶点。
        mesh.base_grid_points.push(grid); // 页规划与正式采样使用精确源坐标。
        indices.insert(point, index); // 后续相邻单元复用。
        Ok(index) // 返回稳定局部索引。
    }; // 闭包只持有后台局部状态。
    let mut x = lo[0]; // 当前单元最小X。
    while x < hi[0] {
        // 最多32列。
        let nx = (x + step).min(hi[0]); // 尾格保持场景端点。
        let mut z = lo[1]; // 当前单元最小Z。
        while z < hi[1] {
            // 最多32行。
            let nz = (z + step).min(hi[1]); // 尾格可以短于完整步长。
            let edges = if let Some(cells) = &chunk.cells {
                // 新路径只处理被规划器选中的完整单元。
                match cells.binary_search_by_key(&[x, z], |cell| cell.origin) {
                    // 规划器按X/Z顺序归并。
                    Ok(index) => cells[index].finer_edges, // 只有实际细邻才能拆边。
                    Err(_) => {
                        z = nz;
                        continue;
                    } // 管理盒内的空洞不是物理地面。
                } // 完整格不需要裁剪。
            } else {
                // 保留完整块夹具的旧边界模板。
                (if x == lo[0] { chunk.finer_edges & 1 } else { 0 })
                    | (if nx == hi[0] {
                        chunk.finer_edges & 2
                    } else {
                        0
                    })
                    | (if z == lo[1] { chunk.finer_edges & 4 } else { 0 })
                    | (if nz == hi[1] {
                        chunk.finer_edges & 8
                    } else {
                        0
                    }) // 只在块外缘拆边。
            }; // 内部普通格固定两三角。
            let west = edges & 1 != 0 && z + step / 2 < nz; // 西侧细边中点。
            let east = edges & 2 != 0 && z + step / 2 < nz; // 东侧细边中点。
            let south = edges & 4 != 0 && x + step / 2 < nx; // 南侧细边中点。
            let north = edges & 8 != 0 && x + step / 2 < nx; // 北侧细边中点。
            let mut polygon = Vec::with_capacity(8); // 至多四角加四边中点。
            polygon.push([2 * x, 2 * z]); // 从西南角开始，XZ顺时针意味着法线朝上。
            if west {
                polygon.push([2 * x, 2 * z + step]);
            } // 西边细采样点。
            polygon.push([2 * x, 2 * nz]); // 西北角。
            if north {
                polygon.push([2 * x + step, 2 * nz]);
            } // 北边细采样点。
            polygon.push([2 * nx, 2 * nz]); // 东北角。
            if east {
                polygon.push([2 * nx, 2 * z + step]);
            } // 东边细采样点。
            polygon.push([2 * nx, 2 * z]); // 东南角。
            if south {
                polygon.push([2 * x + step, 2 * z]);
            } // 南边细采样点。
            let polygon = polygon
                .into_iter()
                .map(|point| vertex(point, &mut mesh))
                .collect::<Result<Vec<_>, _>>()?; // 所有边界高度从共同权威采样。
            if polygon.len() == 4 {
                // 普通格保持两三角形，不额外加密全块。
                mesh.triangles.push([polygon[0], polygon[1], polygon[2]]); // 与另一三角形共享对角线。
                mesh.triangles.push([polygon[0], polygon[2], polygon[3]]); // 完整覆盖矩形且不重叠。
            } else {
                // 过渡格使用严格内部中心，避免共线边的退化扇形。
                let center = vertex([x + nx, z + nz], &mut mesh)?; // 尾格中心允许半个作者格，通过统一采样插值。
                for edge in 0..polygon.len() {
                    mesh.triangles.push([
                        center,
                        polygon[edge],
                        polygon[(edge + 1) % polygon.len()],
                    ]);
                } // 每条细边均成为真实三角面边。
            } // 一个单元完整完成。
            z = nz; // 进入下一行。
        } // 当前列完成。
        x = nx; // 进入下一列。
    } // 所有最多1024个单元完成。
    Ok(mesh) // 完整候选供后台Collider cooking。
} // 没有渲染LOD依赖。
