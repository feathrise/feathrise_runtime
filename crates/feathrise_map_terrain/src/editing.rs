//! F12 任意地形拓扑的局部顶点查询与平滑邻接索引。

mod surface; // 吸附仅查询局部BVH候选，再按真实共享边追踪所选地面。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque}; // 稳定焊接、洞穴分层BFS与局部笔刷查询。

use bevy::math::Vec3; // BVH与Möller–Trumbore射线测试使用稳定Float32向量运算。

use crate::{
    // 编辑索引继续服从共享网格校验与固定桶数。
    MAX_TERRAIN_RASTER_BINS_PER_AXIS,
    TerrainMeshInput,
    derive::{RectilinearGrid, infer_rectilinear_grid, validate_mesh},
}; // 完成共享入口导入。

/// 一份 GLB 拓扑修订对应的不可变笔刷加速数据。
#[derive(Clone, Debug)]
pub struct TerrainEditingIndex {
    grid: Option<RectilinearGrid>, // 严格规则网格仅需行列与格距，不为每个顶点建立通用索引。
    vertex_count: usize,           // 两条路径共用的拓扑身份检查。
    triangle_count: usize,         // 拒绝与构建时不一致的三角表长度。
    bin_counts: [u32; 2],          // X/Z两轴固定桶数。
    world_min: [f32; 2],           // 实际顶点投影范围下界。
    world_max: [f32; 2],           // 实际顶点投影范围上界。
    bin_offsets: Vec<u32>,         // 每个桶在扁平顶点表中的起止偏移。
    bin_vertices: Vec<u32>,        // 每个顶点只登记一次，不产生重复笔刷访问。
    vertex_groups: Vec<u32>,       // 每个GLB属性顶点所属的同XYZ逻辑顶点组。
    group_representatives: Vec<u32>, // 每个逻辑组使用源顺序最前的顶点作为稳定身份。
    group_offsets: Vec<u32>,       // 每个逻辑组在同位置顶点表中的起止偏移。
    group_vertices: Vec<u32>,      // GLB因法线等属性拆分后仍代表同一地形点的全部顶点。
    adjacency_offsets: Vec<u32>,   // 每个逻辑顶点组在邻接表中的起止偏移。
    adjacency_vertices: Vec<u32>,  // 排序去重后的逻辑一环邻居代表顶点。
    triangle_order: Vec<u32>,      // BVH叶节点引用的原始三角序号。
    bvh_nodes: Vec<TerrainBvhNode>, // 后台构建的实际三角形包围层级，光标不扫描整张网格。
} // 结束编辑索引定义。

/// 光标射线与真实GLB三角形的最近命中。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainRayHit {
    pub position: [f32; 3], // 世界空间命中点。
    pub distance: f32,      // 沿调用方向向量的非负参数。
    pub triangle: u32,      // 原始索引表中的三角序号。
}

/// 一个平衡BVH节点；count非零表示叶节点，first指向triangle_order。
#[derive(Clone, Copy, Debug, Default)]
struct TerrainBvhNode {
    minimum: [f32; 3], // 三维AABB下界。
    maximum: [f32; 3], // 三维AABB上界。
    first: u32,        // 叶：triangle_order首偏移；枝：左子节点。
    second: u32,       // 叶：未使用；枝：右子节点。
    count: u16,        // 叶三角数；零表示枝节点。
}

impl TerrainEditingIndex {
    /// 遍历圆形笔刷包围盒内各桶，并对真实落入圆内的顶点调用一次访问器。
    pub fn for_each_vertex_in_circle(
        &self,                             // 当前GLB修订的不可变索引。
        positions: &[[f32; 3]],            // F12当前顶点高度；X/Z在F12中保持不变。
        center: [f32; 2],                  // 世界X/Z笔刷圆心。
        radius: f32,                       // 世界米制笔刷半径。
        mut visit: impl FnMut(usize, f32), // 回传顶点索引和到圆心距离。
    ) {
        if !radius.is_finite()
            || radius <= 0.0
            || !center.iter().all(|value| value.is_finite())
            || positions.len() != self.vertex_count
        {
            return; // 损坏调用输入不得越界访问索引。
        } // 完成访问入口门。
        if let Some(grid) = self.grid {
            let Some([first_x, last_x]) = grid_axis_vertex_range(
                positions,
                center[0] - radius,
                center[0] + radius,
                grid.width,
                1,
                0,
            ) else {
                return; // 圆形笔刷与规则网格 X 顶点列没有交集。
            };
            let Some([first_z, last_z]) = grid_axis_vertex_range(
                positions,
                center[1] - radius,
                center[1] + radius,
                grid.height,
                grid.width,
                2,
            ) else {
                return; // 圆形笔刷与规则网格 Z 顶点行没有交集。
            };
            let radius_squared = radius * radius; // 规则路径也对真实圆而非矩形执行访问。
            for row in first_z..=last_z {
                for column in first_x..=last_x {
                    let vertex = row * grid.width + column; // 行优先网格直接得到稳定源顶点身份。
                    let delta_x = positions[vertex][0] - center[0]; // 保留构建时真实 X 浮点坐标。
                    let delta_z = positions[vertex][2] - center[1]; // 保留构建时真实 Z 浮点坐标。
                    let distance_squared = delta_x * delta_x + delta_z * delta_z; // 精确过滤矩形角落。
                    if distance_squared <= radius_squared {
                        visit(vertex, distance_squared.sqrt()); // 无桶表、无全图扫描，圆内顶点只访问一次。
                    }
                }
            }
            return; // 规则路径不读取通用空间桶。
        }
        let Some([first_x, last_x]) =
            self.axis_bin_range(center[0] - radius, center[0] + radius, 0)
        else {
            return; // 笔刷X范围与地形实际顶点范围无交集。
        }; // 完成X桶裁剪。
        let Some([first_z, last_z]) =
            self.axis_bin_range(center[1] - radius, center[1] + radius, 1)
        else {
            return; // 笔刷Z范围与地形实际顶点范围无交集。
        }; // 完成Z桶裁剪。
        let radius_squared = radius * radius; // 只计算一次真实圆门。
        for bin_z in first_z..=last_z {
            for bin_x in first_x..=last_x {
                let bin = (bin_z * self.bin_counts[0] + bin_x) as usize; // 定位当前扁平桶。
                let start = self.bin_offsets[bin] as usize; // 取得桶顶点首偏移。
                let end = self.bin_offsets[bin + 1] as usize; // 取得桶顶点尾偏移。
                for &vertex in &self.bin_vertices[start..end] {
                    let vertex = vertex as usize; // 转为位置切片索引。
                    let delta_x = positions[vertex][0] - center[0]; // 世界X米制差。
                    let delta_z = positions[vertex][2] - center[1]; // 世界Z米制差。
                    let distance_squared = delta_x * delta_x + delta_z * delta_z; // 不在圆内时避免开方。
                    if distance_squared <= radius_squared {
                        visit(vertex, distance_squared.sqrt()); // 圆内顶点只访问一次。
                    } // 完成真实圆命中门。
                } // 完成当前桶顶点访问。
            } // 完成当前Z桶行。
        } // 完成全部相交桶访问。
    } // 结束圆形局部顶点遍历。

    /// 收集圆内与实际命中点属于同一局部拓扑分量的逻辑代表，避免笔刷穿透洞顶和洞底。
    pub fn connected_representatives_in_circle(
        &self,                     // 当前GLB拓扑修订的不可变编辑索引。
        positions: &[[f32; 3]],    // 当前世界位置；F12只允许Y在既有拓扑上变化。
        indices: &[u32],           // 保持源三角顺序的完整TriangleList。
        center: [f32; 2],          // 世界X/Z笔刷圆心。
        radius: f32,               // 世界米制笔刷半径。
        hit_position: [f32; 3],    // 实际射线命中或水笔刷插值后的三维表面点。
        hit_triangle: Option<u32>, // 有真实命中时优先用其三个逻辑角作为层种子。
    ) -> Vec<(usize, f32)> {
        if self.grid.is_some() {
            let mut affected = Vec::new(); // 规则网格按行优先直接产出升序唯一顶点，不建立有序树。
            self.for_each_vertex_in_circle(positions, center, radius, |vertex, distance| {
                affected.push((vertex, distance)); // 单层完整规则网格没有洞顶/洞底混叠。
            });
            return affected; // 圆内候选直接构成当前规则地形层，无需再建候选树或 BFS。
        }
        let mut affected = BTreeMap::<usize, f32>::new(); // 属性拆点先合并为稳定逻辑代表。
        self.for_each_vertex_in_circle(positions, center, radius, |vertex, distance| {
            if let Some(representative) = self.representative(vertex) {
                affected.entry(representative).or_insert(distance); // 同XYZ副本平面距离相同且只保留一次。
            } // 损坏成员不会进入局部作者写侧。
        }); // 先由固定空间桶限制候选，不扫描整张网格。
        if affected.len() <= 1 {
            return affected.into_iter().collect(); // 零或单点不可能跨越洞穴投影层。
        } // 完成最小集合快速路径。
        let candidates = affected.keys().copied().collect::<BTreeSet<_>>(); // BFS只允许在当前圆内扩展。
        let mut queue = VecDeque::<usize>::new(); // 局部队列容量随真实圆内候选而非整图增长。
        if let Some(hit_triangle) = hit_triangle {
            let base = hit_triangle as usize * 3; // TriangleList中每个面固定占三个索引。
            if let Some(triangle) = indices.get(base..base + 3) {
                for &vertex in triangle {
                    if let Some(representative) = self.representative(vertex as usize)
                        && candidates.contains(&representative)
                    {
                        queue.push_back(representative); // 命中面圆内角是最精确的层种子。
                    } // 完成当前命中角种子门。
                } // 完成三个面角扫描。
            } // 损坏三角身份由最近三维点回退安全收敛。
        } // 没有三角身份的水笔刷直接使用最近三维逻辑点。
        if queue.is_empty() && hit_triangle.is_some() {
            return Vec::new(); // 命中大三角三个角都在笔刷外时宁可不改，也不能猜到投影重合的另一层。
        } // 真实地形雕刻必须以命中面圆内角建立明确局部连通证据。
        if queue.is_empty() {
            let hit_position = Vec3::from_array(hit_position); // 转为Bevy向量执行三维层距离比较。
            if let Some(nearest) = candidates.iter().copied().min_by(|first, second| {
                let first_position = Vec3::from_array(positions[*first]); // 逻辑代表保留真实层高度。
                let second_position = Vec3::from_array(positions[*second]); // 同X/Z下层不会与上层同距离。
                first_position
                    .distance_squared(hit_position)
                    .total_cmp(&second_position.distance_squared(hit_position)) // 最近真实三维点确定大面所属层。
                    .then_with(|| first.cmp(second)) // 等距时按稳定源顶点身份确定结果。
            }) {
                queue.push_back(nearest); // 命中位于大面内部或无面身份时仍选择最近局部层。
            } // 完成最近候选回退。
        } // 完成种子选择。
        let mut selected = BTreeSet::<usize>::new(); // 保存命中层圆内局部连通分量。
        while let Some(vertex) = queue.pop_front() {
            if !selected.insert(vertex) {
                continue; // 已访问逻辑点不重复展开一环。
            } // 完成BFS去重门。
            for neighbor in self.neighbors(vertex) {
                let neighbor = neighbor as usize; // 邻接表保存每组稳定代表索引。
                if candidates.contains(&neighbor) && !selected.contains(&neighbor) {
                    queue.push_back(neighbor); // 不沿圆外远处洞口绕到投影重合的另一层。
                } // 完成圆内邻居门。
            } // 完成当前点一环展开。
        } // 完成命中层局部BFS。
        affected
            .into_iter()
            .filter(|(vertex, _)| selected.contains(vertex)) // 丢弃同投影但局部不连通的洞顶或洞底。
            .collect() // 返回稳定代表升序及原圆心平面距离。
    } // 结束洞穴分层圆形逻辑顶点收集。

    /// 按稳定升序迭代一环邻居；规则网格在栈上计算至多六项，不存每顶点邻接表。
    pub fn neighbors(&self, vertex: usize) -> impl Iterator<Item = u32> + '_ {
        let mut grid_neighbors = [0u32; 6]; // 规则拓扑一环仅有四个正交点和 NE/SW 两个对角点。
        let mut count = 0usize; // 数组前缀才是当前顶点实际存在的邻居。
        if let Some(grid) = self.grid
            && vertex < self.vertex_count
        {
            let row = vertex / grid.width; // 行优先身份直接还原 Z 行。
            let column = vertex % grid.width; // 当前行内的 X 列。
            let mut push = |neighbor: usize| {
                grid_neighbors[count] = neighbor as u32; // 严格拓扑已保证全部顶点能由 U32 索引引用。
                count += 1; // 每个相邻单元只登记一次此条边。
            };
            if row > 0 {
                push(vertex - grid.width);
            } // 北侧正交边。
            if row > 0 && column + 1 < grid.width {
                push(vertex - grid.width + 1);
            } // 东北对角边。
            if column > 0 {
                push(vertex - 1);
            } // 西侧正交边。
            if column + 1 < grid.width {
                push(vertex + 1);
            } // 东侧正交边。
            if row + 1 < grid.height && column > 0 {
                push(vertex + grid.width - 1);
            } // 西南对角边。
            if row + 1 < grid.height {
                push(vertex + grid.width);
            } // 南侧正交边。
            grid_neighbors[..count].sort_unstable(); // 通用路径原先按源顶点身份升序，规则路径保持相同顺序。
        }
        grid_neighbors
            .into_iter()
            .take(count)
            .chain(self.general_neighbors(vertex).iter().copied()) // 两条路径共用零分配迭代接口。
    }

    /// 通用拓扑沿用已焊接的邻接切片；规则路径返回空切片。
    fn general_neighbors(&self, vertex: usize) -> &[u32] {
        if self.grid.is_some() {
            return &[];
        } // 规则邻居已在栈上计算。
        let Some(&group) = self.vertex_groups.get(vertex) else {
            return &[]; // 越界顶点没有逻辑组，调用方不得崩溃。
        }; // 完成顶点到逻辑组映射。
        let Some((&start, &end)) = self
            .adjacency_offsets
            .get(group as usize)
            .zip(self.adjacency_offsets.get(group as usize + 1))
        else {
            return &[]; // 越界顶点没有邻居，调用方不得崩溃。
        }; // 完成邻接偏移读取。
        &self.adjacency_vertices[start as usize..end as usize] // 返回借用切片，不在每次平滑时分配。
    } // 结束通用一环邻居读取。

    /// 返回一个GLB属性顶点所属逻辑地形点的稳定代表索引。
    pub fn representative(&self, vertex: usize) -> Option<usize> {
        if self.grid.is_some() {
            return (vertex < self.vertex_count).then_some(vertex);
        } // 规则网格没有属性拆点，源顶点即逻辑代表。
        let group = *self.vertex_groups.get(vertex)? as usize; // 先取得同XYZ逻辑组。
        self.group_representatives
            .get(group)
            .map(|value| *value as usize) // 源顺序首顶点作为可持久化稳定身份。
    } // 结束逻辑代表查询。

    /// 迭代同 XYZ 属性顶点；规则网格仅产生当前顶点，不分配全图单成员表。
    pub fn coincident_vertices(&self, vertex: usize) -> impl Iterator<Item = u32> + '_ {
        let grid_member = self.grid.is_some() && vertex < self.vertex_count; // 规则路径每个顶点都是单独逻辑组。
        std::iter::once(vertex as u32)
            .take(usize::from(grid_member))
            .chain(self.general_coincident_vertices(vertex).iter().copied()) // 通用路径继续借用完整焊接组。
    }

    /// 通用拓扑读取焊接组成员切片；规则路径不存同位置成员数组。
    fn general_coincident_vertices(&self, vertex: usize) -> &[u32] {
        if self.grid.is_some() {
            return &[];
        } // 单成员由公开迭代接口直接合成。
        let Some(&group) = self.vertex_groups.get(vertex) else {
            return &[]; // 越界顶点没有同位置成员。
        }; // 完成顶点到逻辑组映射。
        let Some((&start, &end)) = self
            .group_offsets
            .get(group as usize)
            .zip(self.group_offsets.get(group as usize + 1))
        else {
            return &[]; // 损坏组偏移不得越界。
        }; // 完成成员偏移读取。
        &self.group_vertices[start as usize..end as usize] // 成员保持原GLB顶点升序。
    } // 结束通用同位置成员查询。

    /// 读取一个逻辑地形点全部属性拆点的平均高度；正常网格成员应严格相等。
    pub fn logical_height(&self, positions: &[[f32; 3]], vertex: usize) -> Option<f32> {
        if positions.len() != self.vertex_count {
            return None; // 位置数组必须仍对应建立索引时的同一顶点表。
        } // 完成拓扑身份门。
        if self.grid.is_some() {
            return positions
                .get(vertex)
                .map(|position| position[1])
                .filter(|height| height.is_finite());
        } // 规则单成员直接读当前高度，雕刻后仍有效。
        let members = self.general_coincident_vertices(vertex); // 只焊接完整位置相同的属性拆点，洞顶和洞底保持独立。
        if members.is_empty() {
            return None; // 损坏索引不得产生除零。
        } // 完成非空组门。
        let mut sum = 0.0f64; // 大组累加使用f64，避免属性拆点数量放大f32舍入。
        for &member in members {
            let height = positions.get(member as usize)?[1]; // 每个成员都必须仍在当前GLB顶点表内。
            if !height.is_finite() {
                return None; // 非有限高度不能进入平滑目标。
            } // 完成成员高度门。
            sum += f64::from(height); // 累加同一逻辑点的当前高度。
        } // 完成全部属性拆点高度累计。
        Some((sum / members.len() as f64) as f32) // 同一几何点的属性拆分副本必须由一笔同步修改。
    } // 结束逻辑高度读取。

    /// 使用后台构建的三角形BVH返回最近实际网格命中，不读取规则高度网格。
    pub fn ray_intersection(
        &self,
        positions: &[[f32; 3]],
        indices: &[u32],
        origin: [f32; 3],
        direction: [f32; 3],
    ) -> Option<TerrainRayHit> {
        if positions.len() != self.vertex_count
            || indices.len() / 3 != self.triangle_count
            || indices.len() % 3 != 0
        {
            return None; // 索引与当前拓扑修订不一致时拒绝越界或误命中。
        }
        let origin = Vec3::from_array(origin);
        let direction = Vec3::from_array(direction);
        if !origin.is_finite()
            || !direction.is_finite()
            || direction.length_squared() <= f32::EPSILON
        {
            return None; // 非法射线不得进入除法与叉乘。
        }
        if let Some(grid) = self.grid {
            return ray_rectilinear_grid(positions, grid, origin, direction); // 规则路径只访问射线经过的真实单元和当前雕刻高度。
        }
        let mut stack = [0u32; 64]; // 平衡树深度远小于固定栈，光标每帧不分配堆内存。
        let mut stack_len = 1usize;
        stack[0] = 0;
        let mut nearest = f32::INFINITY;
        let mut hit_triangle = None;
        while stack_len > 0 {
            stack_len -= 1;
            let node_index = stack[stack_len] as usize;
            let node = *self.bvh_nodes.get(node_index)?;
            let Some((enter, _)) = ray_aabb_interval(origin, direction, node.minimum, node.maximum)
            else {
                continue;
            };
            if enter > nearest {
                continue; // 该节点最早也晚于已有命中。
            }
            if node.count != 0 {
                let start = node.first as usize;
                let end = start + node.count as usize;
                for &triangle in self.triangle_order.get(start..end)? {
                    let index = triangle as usize * 3;
                    let [a, b, c] = *indices.get(index..index + 3)? else {
                        return None;
                    };
                    let a = Vec3::from_array(*positions.get(a as usize)?);
                    let b = Vec3::from_array(*positions.get(b as usize)?);
                    let c = Vec3::from_array(*positions.get(c as usize)?);
                    if let Some(distance) = ray_triangle_distance(origin, direction, a, b, c)
                        && distance < nearest
                    {
                        nearest = distance;
                        hit_triangle = Some(triangle);
                    }
                }
                continue;
            }
            let left = node.first as usize;
            let right = node.second as usize;
            let left_enter = self.bvh_nodes.get(left).and_then(|child| {
                ray_aabb_interval(origin, direction, child.minimum, child.maximum)
                    .map(|value| value.0)
            });
            let right_enter = self.bvh_nodes.get(right).and_then(|child| {
                ray_aabb_interval(origin, direction, child.minimum, child.maximum)
                    .map(|value| value.0)
            });
            let mut push = |node: usize| {
                if stack_len < stack.len() {
                    stack[stack_len] = node as u32;
                    stack_len += 1;
                    true
                } else {
                    false
                }
            };
            match (left_enter, right_enter) {
                (Some(left_distance), Some(right_distance)) => {
                    let (near, far) = if left_distance <= right_distance {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    if !push(far) || !push(near) {
                        return None; // 防御未来预算失控导致固定栈溢出。
                    }
                }
                (Some(_), None) => {
                    if !push(left) {
                        return None;
                    }
                }
                (None, Some(_)) => {
                    if !push(right) {
                        return None;
                    }
                }
                (None, None) => {}
            }
        }
        hit_triangle.map(|triangle| TerrainRayHit {
            position: (origin + direction * nearest).to_array(),
            distance: nearest,
            triangle,
        })
    }

    /// 把一个世界轴区间裁剪到编辑索引的闭桶范围。
    fn axis_bin_range(&self, minimum: f32, maximum: f32, axis: usize) -> Option<[u32; 2]> {
        if maximum < self.world_min[axis] || minimum > self.world_max[axis] {
            return None; // 区间与实际顶点包围盒完全分离。
        } // 完成相离门。
        let first = self.point_bin(minimum.max(self.world_min[axis]), axis); // 裁剪后取得首桶。
        let last = self.point_bin(maximum.min(self.world_max[axis]), axis); // 裁剪后取得末桶。
        Some([first, last]) // 返回闭区间供笔刷遍历。
    } // 结束轴区间桶映射。

    /// 把范围内一个世界坐标映射到零基桶索引。
    fn point_bin(&self, value: f32, axis: usize) -> u32 {
        let normalized = ((value - self.world_min[axis])
            / (self.world_max[axis] - self.world_min[axis]))
            .clamp(0.0, 1.0); // 浮点边缘误差裁回合法区间。
        ((normalized * self.bin_counts[axis] as f32).floor() as u32).min(self.bin_counts[axis] - 1) // 上界顶点稳定归入末桶。
    } // 结束单点桶映射。
} // 结束编辑索引接口。

/// 射线在规则网格的 X/Z 投影中按实际格线推进；每格仍测试当前雕刻高度的真实两片三角。
fn ray_rectilinear_grid(
    positions: &[[f32; 3]], // 仅 Y 可在建索引后改变的当前作者顶点。
    grid: RectilinearGrid,  // 严格检验过的行优先完整网格。
    origin: Vec3,           // 世界空间射线起点。
    direction: Vec3,        // 调用方提供的有限非零方向。
) -> Option<TerrainRayHit> {
    let (mut current_t, end_t) = ray_grid_xz_interval(grid, origin, direction)?; // 先裁到地形的实际水平范围。
    let start = origin + direction * current_t; // 射线进入矩形时的水平位置。
    if !start.is_finite() {
        return None;
    } // 极端浮点乘积不得污染格线定位。
    let mut column = grid_axis_cell(
        positions,
        start.x,
        direction.x,
        grid.width,
        1,
        0,
        grid.world_min[0],
        grid.step[0],
    ); // 首个实际 X 单元。
    let mut row = grid_axis_cell(
        positions,
        start.z,
        direction.z,
        grid.height,
        grid.width,
        2,
        grid.world_min[1],
        grid.step[1],
    ); // 首个实际 Z 单元。
    let mut nearest = None; // 当前射线尚无有效地形表面命中。
    if direction.x == 0.0 && direction.z == 0.0 {
        let mut columns = [column; 2]; // 竖直射线在格线上的两侧单元都要受检。
        let mut rows = [row; 2]; // 格线交点最多邻接四个单元。
        let mut column_count = 1usize; // 普通单元内部只有一列。
        let mut row_count = 1usize; // 普通单元内部只有一行。
        if column > 0 && positions[column][0] == start.x {
            columns[1] = column - 1; // 恰在 X 格线上时补左侧单元。
            column_count += 1;
        }
        if row > 0 && positions[row * grid.width][2] == start.z {
            rows[1] = row - 1; // 恰在 Z 格线上时补上方单元。
            row_count += 1;
        }
        for &cell_row in &rows[..row_count] {
            for &cell_column in &columns[..column_count] {
                test_grid_cell_triangles(
                    positions,
                    grid,
                    cell_row,
                    cell_column,
                    origin,
                    direction,
                    current_t,
                    end_t,
                    &mut nearest,
                )?; // 当前雕刻高度决定最近真实命中。
            }
        }
        return nearest; // 垂直射线不会跨越其他 X/Z 单元。
    }
    loop {
        test_grid_cell_triangles(
            positions,
            grid,
            row,
            column,
            origin,
            direction,
            current_t,
            end_t,
            &mut nearest,
        )?; // 每格固定只测试两片真实三角。
        let next_x = if direction.x > 0.0 {
            (positions[column + 1][0] - origin.x) / direction.x // 正向穿越当前格的右边界。
        } else if direction.x < 0.0 {
            (positions[column][0] - origin.x) / direction.x // 反向穿越当前格的左边界。
        } else {
            f32::INFINITY // 射线平行 X 格线，不发生 X 列变化。
        };
        let next_z = if direction.z > 0.0 {
            (positions[(row + 1) * grid.width][2] - origin.z) / direction.z // 正向穿越当前格的下边界。
        } else if direction.z < 0.0 {
            (positions[row * grid.width][2] - origin.z) / direction.z // 反向穿越当前格的上边界。
        } else {
            f32::INFINITY // 射线平行 Z 格线，不发生 Z 行变化。
        };
        let next_t = next_x.min(next_z); // DDA 按射线参数由近及远访问单元。
        if nearest.is_some_and(|hit: TerrainRayHit| hit.distance <= next_t + 1.0e-5) {
            return nearest; // 后续单元不可能有比当前已命中的表面更近的交点。
        }
        if !next_t.is_finite() || next_t > end_t + 1.0e-5 {
            return nearest; // 已离开地形水平范围，或者不再有下一格。
        }
        if next_x <= next_z {
            if direction.x > 0.0 {
                column += 1;
            } else if let Some(previous) = column.checked_sub(1) {
                column = previous;
            } else {
                return nearest;
            } // 穿过 X 格线后进入相邻列。
            if column + 1 >= grid.width {
                return nearest;
            } // 水平投影已越过最后一列。
        }
        if next_z <= next_x {
            if direction.z > 0.0 {
                row += 1;
            } else if let Some(previous) = row.checked_sub(1) {
                row = previous;
            } else {
                return nearest;
            } // 穿过 Z 格线后进入相邻行。
            if row + 1 >= grid.height {
                return nearest;
            } // 水平投影已越过最后一行。
        }
        current_t = current_t.max(next_t); // 浮点舍入时仍保持区间参数单调。
    }
}

/// X/Z 矩形射线裁剪只读水平范围，Y 高度会在雕刻后改变，不能使用过期 Y 包围盒。
fn ray_grid_xz_interval(
    grid: RectilinearGrid,
    origin: Vec3,
    direction: Vec3,
) -> Option<(f32, f32)> {
    let mut enter = 0.0f32; // 射线只接受起点前方的交点。
    let mut exit = f32::INFINITY; // 任一水平轴都可收紧离开参数。
    for (axis, minimum, maximum) in [
        (0usize, grid.world_min[0], grid.world_max[0]),
        (2usize, grid.world_min[1], grid.world_max[1]),
    ] {
        let delta = direction[axis]; // 当前水平轴的射线速度。
        if delta == 0.0 {
            if origin[axis] < minimum || origin[axis] > maximum {
                return None;
            } // 平行且位于范围外不可能命中地形。
            continue; // 平行轴不限制进入/离开参数。
        }
        let first = (minimum - origin[axis]) / delta; // 与下边界的参数。
        let second = (maximum - origin[axis]) / delta; // 与上边界的参数。
        enter = enter.max(first.min(second)); // 按方向选择较近的边界。
        exit = exit.min(first.max(second)); // 按方向选择较远的边界。
        if enter > exit {
            return None;
        } // 两轴投影区间没有交集。
    }
    (exit >= 0.0).then_some((enter.max(0.0), exit)) // 水平范围与射线前方相交。
}

/// 从估算格距定位入界单元，再用真实格线纠正浮点边界误差。
fn grid_axis_cell(
    positions: &[[f32; 3]], // 当前网格真实顶点序列。
    value: f32,             // 射线进入地形矩形时的世界水平坐标。
    direction: f32,         // 当前轴射线方向，负向恰在格线时选前一格。
    count: usize,           // 当前轴的顶点数。
    stride: usize,          // X连续，Z以网格行宽跳转。
    axis: usize,            // 0=X或2=Z。
    minimum: f32,           // 创建时的首条格线。
    step: f32,              // 严格规则检测给出的真实格距。
) -> usize {
    let mut cell = ((value - minimum) / step)
        .floor()
        .clamp(0.0, (count - 2) as f32) as usize; // 常数时间近似定位。
    while cell + 1 < count - 1 && positions[(cell + 1) * stride][axis] <= value {
        cell += 1;
    } // 修正向左一格的舍入。
    while cell > 0 && positions[cell * stride][axis] > value {
        cell -= 1;
    } // 修正向右一格的舍入。
    if direction < 0.0 && cell > 0 && positions[cell * stride][axis] == value {
        cell -= 1;
    } // 负向沿格线出发必须进入左侧/上方单元。
    cell // 返回包含进入点的有效单元。
}

/// 用当前顶点高度测试一个规则单元的真实两片三角，并保留最近非负交点。
fn test_grid_cell_triangles(
    positions: &[[f32; 3]],              // 可被 F12 雕刻更新 Y 的完整源位置。
    grid: RectilinearGrid,               // 行优先顶点和单元布局。
    row: usize,                          // 当前 Z 单元行。
    column: usize,                       // 当前 X 单元列。
    origin: Vec3,                        // 世界射线起点。
    direction: Vec3,                     // 世界射线方向。
    minimum_t: f32,                      // 射线进入当前单元前的参数。
    maximum_t: f32,                      // 射线离开整张地形水平范围的参数。
    nearest: &mut Option<TerrainRayHit>, // 外层 DDA 跨单元共享最近交点。
) -> Option<()> {
    let cell = row.checked_mul(grid.width - 1)?.checked_add(column)?; // 单元行优先身份。
    let first_triangle = u32::try_from(cell.checked_mul(2)?).ok()?; // 严格保留 GLB 原始三角序号。
    let a = row * grid.width + column; // 西北角。
    let b = a + 1; // 东北角。
    let c = a + grid.width; // 西南角。
    let d = c + 1; // 东南角。
    for (triangle, corners) in [(first_triangle, [a, c, b]), (first_triangle + 1, [b, c, d])] {
        let first = Vec3::from_array(*positions.get(corners[0])?); // 当前首角高度。
        let second = Vec3::from_array(*positions.get(corners[1])?); // 当前次角高度。
        let third = Vec3::from_array(*positions.get(corners[2])?); // 当前末角高度。
        if let Some(distance) = ray_triangle_distance(origin, direction, first, second, third)
            && distance + 1.0e-5 >= minimum_t
            && distance <= maximum_t + 1.0e-5
            && nearest.is_none_or(|hit| distance < hit.distance)
        {
            *nearest = Some(TerrainRayHit {
                position: (origin + direction * distance).to_array(), // 真实三角插值交点。
                distance, // 保留调用方方向参数，不伪造单位长度。
                triangle, // GLB 中的原始三角序号。
            });
        }
    }
    Some(()) // 两片三角均按当前 Y 完成验证。
}

/// 在规则网格的真实单轴格线上二分圆形包围盒范围，不按近似格距漏掉边界顶点。
fn grid_axis_vertex_range(
    positions: &[[f32; 3]], // 规则网格当前世界顶点；X/Z 在作者雕刻时不变。
    minimum: f32,           // 笔刷包围盒在当前轴的下界。
    maximum: f32,           // 笔刷包围盒在当前轴的上界。
    count: usize,           // 当前轴包含端点的格线数量。
    stride: usize,          // X轴连续，Z轴按行宽跳转。
    axis: usize,            // 顶点数组中 0=X 或 2=Z。
) -> Option<[usize; 2]> {
    let mut first = 0usize; // 寻找首个 >= minimum 的格线。
    let mut end = count; // 二分上界不包含已经排除的格线。
    while first < end {
        let middle = first + (end - first) / 2; // 计算不会溢出的中点。
        if positions[middle * stride][axis] < minimum {
            first = middle + 1;
        } else {
            end = middle;
        } // 排除圆形包围盒左侧顶点。
    }
    let lower = first; // 保存符合下界的首列或首行。
    end = count; // 重新查找首个 > maximum 的格线。
    while first < end {
        let middle = first + (end - first) / 2; // 二分真实格线而非浮点估算索引。
        if positions[middle * stride][axis] <= maximum {
            first = middle + 1;
        } else {
            end = middle;
        } // 包含恰在半径边界上的顶点。
    }
    (lower < first).then_some([lower, first.saturating_sub(1)]) // 不相交时直接跳过整张规则网格。
}

/// 从一个受检 GLB 拓扑后台构建笔刷空间桶和一环邻接表。
pub fn build_terrain_editing_index(mesh: &TerrainMeshInput) -> Result<TerrainEditingIndex, String> {
    validate_mesh(mesh)?; // 编辑器不得接受运行时会拒绝的另一套拓扑规则。
    let vertex_count = mesh.positions.len(); // 两条路径都核对当前顶点表长度。
    let triangle_count = mesh.indices.len() / 3; // 当前三角身份只与构建时拓扑配套。
    if let Some(grid) = infer_rectilinear_grid(mesh) {
        return Ok(TerrainEditingIndex {
            grid: Some(grid),                  // 完整等距规则地形只保存行列、范围和格距。
            vertex_count,                      // 不复制数千万条顶点身份。
            triangle_count,                    // 射线直接按单元反推两个三角身份。
            bin_counts: [0; 2],                // 规则笔刷使用行列范围，不建空间桶。
            world_min: grid.world_min,         // 保留实际规则网格水平起点。
            world_max: grid.world_max,         // 保留实际规则网格水平终点。
            bin_offsets: Vec::new(),           // 不为每个桶分配偏移。
            bin_vertices: Vec::new(),          // 不重复保存所有顶点编号。
            vertex_groups: Vec::new(),         // 规则顶点直接是逻辑代表。
            group_representatives: Vec::new(), // 不为单成员逻辑点建代表表。
            group_offsets: Vec::new(),         // 无属性拆点组偏移。
            group_vertices: Vec::new(),        // 无属性拆点成员副本。
            adjacency_offsets: Vec::new(),     // 一环按行列即时计算。
            adjacency_vertices: Vec::new(),    // 不保存六倍顶点数量的邻接项。
            triangle_order: Vec::new(),        // DDA 定位真实两个三角，不建全三角排序。
            bvh_nodes: Vec::new(),             // 平地不用通用三维 BVH。
        });
    }
    let mut world_min = [f32::INFINITY; 2]; // 累计实际X/Z下界。
    let mut world_max = [f32::NEG_INFINITY; 2]; // 累计实际X/Z上界。
    for position in &mesh.positions {
        world_min[0] = world_min[0].min(position[0]); // 更新X下界。
        world_min[1] = world_min[1].min(position[2]); // 更新Z下界。
        world_max[0] = world_max[0].max(position[0]); // 更新X上界。
        world_max[1] = world_max[1].max(position[2]); // 更新Z上界。
    } // 完成投影包围盒累计。
    if world_min[0] >= world_max[0] || world_min[1] >= world_max[1] {
        return Err("动态地形顶点投影范围必须在X/Z两轴都有正跨度".to_owned()); // 完整三维地形仍必须覆盖具有面积的地图水平范围。
    } // 完成投影范围门。
    let vertex_axis = (mesh.positions.len() as f32).sqrt().ceil() as u32; // 近似规则密度用于选择有界桶数。
    let bin_counts = [
        vertex_axis.min(MAX_TERRAIN_RASTER_BINS_PER_AXIS).max(1), // X轴最多64桶。
        vertex_axis.min(MAX_TERRAIN_RASTER_BINS_PER_AXIS).max(1), // Z轴最多64桶。
    ]; // 完成两轴桶规格。
    let mut bins = vec![Vec::<u32>::new(); (bin_counts[0] * bin_counts[1]) as usize]; // 每顶点只进入一个桶。
    for (vertex, position) in mesh.positions.iter().enumerate() {
        let x = point_bin(position[0], world_min[0], world_max[0], bin_counts[0]); // 映射X桶。
        let z = point_bin(position[2], world_min[1], world_max[1], bin_counts[1]); // 映射Z桶。
        bins[(z * bin_counts[0] + x) as usize].push(vertex as u32); // 保持源顶点升序登记。
    } // 完成顶点空间桶构建。
    let mut bin_offsets = Vec::with_capacity(bins.len() + 1); // 扁平索引比嵌套Vec更适合跨任务共享。
    let mut bin_vertices = Vec::with_capacity(mesh.positions.len()); // 每顶点恰好一项。
    bin_offsets.push(0); // 第一桶从零偏移开始。
    for bin in bins {
        bin_vertices.extend(bin); // 追加当前桶稳定顶点序列。
        bin_offsets.push(bin_vertices.len() as u32); // 记录下一桶起点。
    } // 完成空间桶扁平化。
    let mut group_by_position = HashMap::<(u32, u32, u32), u32>::new(); // 完整XYZ焊接属性拆点；洞穴多层不得互相合并。
    let mut vertex_groups = Vec::with_capacity(mesh.positions.len()); // 每个源顶点恰好映射一个逻辑组。
    let mut grouped_vertices = Vec::<Vec<u32>>::new(); // 组顺序由首次出现的源顶点决定，完全确定。
    for (vertex, position) in mesh.positions.iter().enumerate() {
        let key = (
            canonical_coordinate_bits(position[0]), // 正负零代表同一世界X。
            canonical_coordinate_bits(position[1]), // 只有完全同高的属性副本才属于同一逻辑点。
            canonical_coordinate_bits(position[2]), // 正负零代表同一世界Z。
        ); // 完成精确GLB属性拆点键。
        let group = if let Some(&group) = group_by_position.get(&key) {
            group // 已出现的同XYZ顶点加入既有逻辑地形点。
        } else {
            let group = grouped_vertices.len() as u32; // 新组身份按源顶点扫描顺序递增。
            group_by_position.insert(key, group); // 后续属性拆点复用该逻辑组。
            grouped_vertices.push(Vec::new()); // 为新逻辑点建立成员表。
            group
        }; // 完成当前顶点逻辑组选择。
        vertex_groups.push(group); // 保存O(1)反向查询。
        grouped_vertices[group as usize].push(vertex as u32); // 成员天然按源索引升序。
    } // 完成全部同位置属性顶点焊接。
    let group_representatives = grouped_vertices
        .iter()
        .map(|members| members[0]) // 每组由首次扫描顶点创建，因此首成员稳定且必然存在。
        .collect::<Vec<_>>(); // 保存逻辑邻接和作者笔刷使用的稳定身份。
    let mut group_offsets = Vec::with_capacity(grouped_vertices.len() + 1); // 扁平成员偏移表。
    let mut group_vertices = Vec::with_capacity(mesh.positions.len()); // 每个源顶点恰好保留一次。
    group_offsets.push(0); // 第一组从零偏移开始。
    for members in grouped_vertices {
        group_vertices.extend(members); // 追加当前逻辑点全部属性拆点。
        group_offsets.push(group_vertices.len() as u32); // 记录下一组起点。
    } // 完成逻辑成员表扁平化。
    let mut adjacency = vec![Vec::<u32>::new(); group_representatives.len()]; // 后台建立焊接后的逻辑一环邻接。
    for triangle in mesh.indices.chunks_exact(3) {
        let a = triangle[0]; // 当前面首顶点。
        let b = triangle[1]; // 当前面次顶点。
        let c = triangle[2]; // 当前面末顶点。
        for [from, to] in [[a, b], [b, c], [c, a]] {
            let from_group = vertex_groups[from as usize] as usize; // 属性顶点先映射到真实逻辑点。
            let to_group = vertex_groups[to as usize] as usize; // 邻端同样焊接法线或UV拆点。
            if from_group == to_group {
                continue; // 同位置属性拆点之间不构成零长度平滑边。
            } // 完成逻辑自环门。
            adjacency[from_group].push(group_representatives[to_group]); // 正向边保存邻组稳定代表。
            adjacency[to_group].push(group_representatives[from_group]); // 反向边保存邻组稳定代表。
        } // 完成当前面三条无向边登记。
    } // 完成全部拓扑边收集。
    let mut adjacency_offsets = Vec::with_capacity(adjacency.len() + 1); // 扁平邻接偏移表。
    let mut adjacency_vertices = Vec::new(); // 实际容量由排序去重后的边数决定。
    adjacency_offsets.push(0); // 顶点零的邻接从零偏移开始。
    for mut neighbors in adjacency {
        neighbors.sort_unstable(); // 稳定索引值升序便于确定性平滑。
        neighbors.dedup(); // 共享相邻三角形不得重复计算同一邻居权重。
        adjacency_vertices.extend(neighbors); // 追加当前顶点完整一环。
        adjacency_offsets.push(adjacency_vertices.len() as u32); // 记录下一顶点起点。
    } // 完成邻接表扁平化。
    let (triangle_order, bvh_nodes) = build_triangle_bvh(mesh)?; // 实际拓扑光标命中索引同样在后台一次完成。
    Ok(TerrainEditingIndex {
        grid: None,            // 任意拓扑保留焊接、空间桶和三维 BVH。
        vertex_count,          // 顶点身份与构建时一致。
        triangle_count,        // 三角身份与构建时一致。
        bin_counts,            // 保存两轴桶数。
        world_min,             // 保存实际投影下界。
        world_max,             // 保存实际投影上界。
        bin_offsets,           // 安装空间桶偏移。
        bin_vertices,          // 安装空间桶顶点。
        vertex_groups,         // 安装源顶点到逻辑地形点映射。
        group_representatives, // 安装每组稳定代表索引。
        group_offsets,         // 安装同位置成员偏移。
        group_vertices,        // 安装同位置属性拆点成员。
        adjacency_offsets,     // 安装一环偏移。
        adjacency_vertices,    // 安装一环顶点。
        triangle_order,        // 安装BVH叶三角序号。
        bvh_nodes,             // 安装平衡三维包围树。
    }) // 完成不可变编辑索引构造。
} // 结束编辑索引派生。

/// 把坐标浮点值转为属性拆点焊接键；正负零在世界空间中必须视为同一点。
fn canonical_coordinate_bits(value: f32) -> u32 {
    if value == 0.0 { 0 } else { value.to_bits() } // 其余有限值保持GLB accessor的精确位身份。
} // 结束坐标键归一化。

const BVH_LEAF_TRIANGLES: usize = 8; // 小叶减少节点内存并保持射线测试局部。

/// 后台建立按三角质心中位数切分的平衡三维BVH。
fn build_triangle_bvh(mesh: &TerrainMeshInput) -> Result<(Vec<u32>, Vec<TerrainBvhNode>), String> {
    let mut order = (0..mesh.indices.len() / 3)
        .map(|value| value as u32)
        .collect::<Vec<_>>();
    let mut nodes = Vec::with_capacity(order.len().saturating_mul(2));
    build_bvh_node(mesh, &mut order, 0, &mut nodes)?;
    Ok((order, nodes))
}

fn build_bvh_node(
    mesh: &TerrainMeshInput,
    order: &mut [u32],
    order_start: usize,
    nodes: &mut Vec<TerrainBvhNode>,
) -> Result<u32, String> {
    if order.is_empty() {
        return Err("动态地形BVH不能包含空节点".to_owned());
    }
    let node_index = nodes.len() as u32;
    nodes.push(TerrainBvhNode::default());
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    let mut centroid_minimum = [f32::INFINITY; 3];
    let mut centroid_maximum = [f32::NEG_INFINITY; 3];
    for &triangle in order.iter() {
        let (triangle_minimum, triangle_maximum, centroid) = triangle_bounds(mesh, triangle)?;
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(triangle_minimum[axis]);
            maximum[axis] = maximum[axis].max(triangle_maximum[axis]);
            centroid_minimum[axis] = centroid_minimum[axis].min(centroid[axis]);
            centroid_maximum[axis] = centroid_maximum[axis].max(centroid[axis]);
        }
    }
    if order.len() <= BVH_LEAF_TRIANGLES {
        nodes[node_index as usize] = TerrainBvhNode {
            minimum,
            maximum,
            first: order_start as u32,
            second: 0,
            count: order.len() as u16,
        };
        return Ok(node_index);
    }
    let extent = Vec3::from_array(centroid_maximum) - Vec3::from_array(centroid_minimum);
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let middle = order.len() / 2;
    order.select_nth_unstable_by(middle, |left, right| {
        let left = triangle_bounds(mesh, *left)
            .map(|value| value.2[axis])
            .unwrap_or(0.0);
        let right = triangle_bounds(mesh, *right)
            .map(|value| value.2[axis])
            .unwrap_or(0.0);
        left.total_cmp(&right)
            .then_with(|| left.to_bits().cmp(&right.to_bits()))
    });
    let (left_order, right_order) = order.split_at_mut(middle);
    let left = build_bvh_node(mesh, left_order, order_start, nodes)?;
    let right = build_bvh_node(mesh, right_order, order_start + middle, nodes)?;
    nodes[node_index as usize] = TerrainBvhNode {
        minimum,
        maximum,
        first: left,
        second: right,
        count: 0,
    };
    Ok(node_index)
}

fn triangle_bounds(
    mesh: &TerrainMeshInput,
    triangle: u32,
) -> Result<([f32; 3], [f32; 3], [f32; 3]), String> {
    let index = triangle as usize * 3;
    let indices = mesh
        .indices
        .get(index..index + 3)
        .ok_or_else(|| "动态地形BVH三角索引越界".to_owned())?;
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    let mut centroid = Vec3::ZERO;
    for &vertex in indices {
        let position = *mesh
            .positions
            .get(vertex as usize)
            .ok_or_else(|| "动态地形BVH顶点索引越界".to_owned())?;
        centroid += Vec3::from_array(position);
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(position[axis]);
            maximum[axis] = maximum[axis].max(position[axis]);
        }
    }
    Ok((minimum, maximum, (centroid / 3.0).to_array()))
}

fn ray_aabb_interval(
    origin: Vec3,
    direction: Vec3,
    minimum: [f32; 3],
    maximum: [f32; 3],
) -> Option<(f32, f32)> {
    let minimum = Vec3::from_array(minimum);
    let maximum = Vec3::from_array(maximum);
    let mut enter = 0.0f32;
    let mut exit = f32::INFINITY;
    for axis in 0..3 {
        let delta = direction[axis];
        if delta.abs() <= f32::EPSILON {
            if origin[axis] < minimum[axis] || origin[axis] > maximum[axis] {
                return None;
            }
            continue;
        }
        let first = (minimum[axis] - origin[axis]) / delta;
        let second = (maximum[axis] - origin[axis]) / delta;
        enter = enter.max(first.min(second));
        exit = exit.min(first.max(second));
        if enter > exit {
            return None;
        }
    }
    (exit >= 0.0).then_some((enter.max(0.0), exit))
}

fn ray_triangle_distance(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let edge_ab = b - a;
    let edge_ac = c - a;
    let cross = direction.cross(edge_ac);
    let determinant = edge_ab.dot(cross);
    if determinant.abs() <= 1.0e-7 {
        return None;
    }
    let inverse = determinant.recip();
    let offset = origin - a;
    let u = offset.dot(cross) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = offset.cross(edge_ab);
    let v = direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = edge_ac.dot(q) * inverse;
    (distance >= 0.0 && distance.is_finite()).then_some(distance)
}

/// 独立映射辅助函数供构建阶段使用。
fn point_bin(value: f32, minimum: f32, maximum: f32, bins: u32) -> u32 {
    let normalized = ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0); // 把世界轴归一化。
    ((normalized * bins as f32).floor() as u32).min(bins - 1) // 上界归入最后一桶。
} // 结束构建期桶映射。

#[cfg(test)]
mod tests {
    use super::*; // 直接验证私有索引布局和公开访问器。
    use crate::{TerrainSampleSpec, build_flat_terrain_mesh}; // 规则新图必须测试正式白模拓扑。

    #[test]
    fn regular_grid_index_keeps_only_constant_metadata_and_computes_neighbors() {
        let mesh = build_flat_terrain_mesh(
            TerrainSampleSpec {
                world_min: [-4.0, -3.0], // X轴九个顶点覆盖八米。
                world_max: [4.0, 3.0],   // Z轴七个顶点覆盖六米。
                resolution: [9, 7],      // 非方形网格仍按行优先识别。
                base_height: -80.0,      // 沿用新地图高度范围。
                height_scale: 160.0,     // Y=0 位于合法中段。
            },
            0.0,
        )
        .unwrap(); // 使用正式平地网格创建器。
        let index = build_terrain_editing_index(&mesh).unwrap(); // 后台选择严格规则路径。
        assert!(index.grid.is_some()); // 不退化到通用多层索引。
        assert!(index.bin_vertices.is_empty() && index.vertex_groups.is_empty()); // 不复制逐顶点桶或焊接组。
        assert!(index.adjacency_vertices.is_empty() && index.bvh_nodes.is_empty()); // 不分配逐点邻接或全三角BVH。
        assert_eq!(
            index.neighbors(31).collect::<Vec<_>>(),
            [22, 23, 30, 32, 39, 40]
        ); // 中心点的四正交与 NE/SW 对角边保持升序。
        assert_eq!(index.coincident_vertices(31).collect::<Vec<_>>(), [31]); // 无属性拆点时直接返回自身。
        assert_eq!(index.representative(31), Some(31)); // GLB源顶点身份不变。
        let mut visited = Vec::new(); // 实际圆内结果按行优先输出。
        index.for_each_vertex_in_circle(&mesh.positions, [0.0, 0.0], 1.0, |vertex, _| {
            visited.push(vertex)
        }); // 中心十字五点。
        assert_eq!(visited, [22, 30, 31, 32, 40]); // 边界上的顶点不能因二分漏掉。
    }

    #[test]
    fn regular_grid_ray_crosses_cells_and_reads_sculpted_current_heights() {
        let mut mesh = build_flat_terrain_mesh(
            TerrainSampleSpec {
                world_min: [0.0, 0.0], // 射线从 X 负侧进入。
                world_max: [8.0, 8.0], // 九乘九规则单元。
                resolution: [9, 9],    // 中途多个单元可供 DDA 穿越。
                base_height: -80.0,    // 合法高度量程。
                height_scale: 160.0,   // 山丘不会触及上下界。
            },
            0.0,
        )
        .unwrap(); // 先构建真实规则平地拓扑。
        let index = build_terrain_editing_index(&mesh).unwrap(); // 索引冻结 X/Z 与拓扑，但不冻结 Y。
        mesh.positions[4 * 9 + 5][1] = 3.0; // 模拟索引建好后 F12 抬高当前山丘顶点。
        let origin = Vec3::new(-2.0, 5.0, 3.5); // 起点位于地形外，较高于山丘。
        let direction = Vec3::new(1.0, -0.45, 0.1); // 斜射线跨越多个 X/Z 单元。
        let expected = mesh
            .indices
            .chunks_exact(3)
            .enumerate()
            .filter_map(|(triangle, corners)| {
                let [a, b, c] = [
                    corners[0] as usize,
                    corners[1] as usize,
                    corners[2] as usize,
                ]; // 保留GLB三角顺序。
                ray_triangle_distance(
                    origin,
                    direction,
                    Vec3::from_array(mesh.positions[a]),
                    Vec3::from_array(mesh.positions[b]),
                    Vec3::from_array(mesh.positions[c]),
                )
                .map(|distance| (triangle as u32, distance)) // 只比较当前雕刻网格的真实相交。
            })
            .min_by(|first, second| first.1.total_cmp(&second.1))
            .unwrap(); // 山丘应提供最近命中。
        let hit = index
            .ray_intersection(
                &mesh.positions,
                &mesh.indices,
                origin.to_array(),
                direction.to_array(),
            )
            .unwrap(); // 规则 DDA 必须找到同一最近相交。
        assert!((hit.distance - expected.1).abs() < 1.0e-4); // 不用原先平地高度或高度采样缓存代替真三角。
        assert_eq!(hit.triangle, expected.0); // GLB原始三角身份供笔刷连通逻辑使用。
        assert!(hit.position[1] > 0.0); // 命中被抬起的山丘，而不是旧 Y=0 平面。
    }

    #[test]
    fn regular_grid_vertical_ray_on_grid_crossing_hits_true_surface() {
        let mesh = build_flat_terrain_mesh(
            TerrainSampleSpec {
                world_min: [0.0, 0.0], // 格线包含整数坐标。
                world_max: [3.0, 3.0], // 中央 X/Z=1 位于四格交点。
                resolution: [4, 4],    // 三乘三规则单元。
                base_height: -80.0,    // 合法高度量程。
                height_scale: 160.0,   // 平地 Y=0 合法。
            },
            0.0,
        )
        .unwrap(); // 正式创建拓扑。
        let index = build_terrain_editing_index(&mesh).unwrap(); // 使用紧凑规则索引。
        let hit = index
            .ray_intersection(
                &mesh.positions,
                &mesh.indices,
                [1.0, 5.0, 1.0],
                [0.0, -1.0, 0.0],
            )
            .unwrap(); // 竖直射线恰在四格共享顶点。
        assert_eq!(hit.position, [1.0, 0.0, 1.0]); // 不能因只选一侧格子而漏掉边界命中。
    }

    #[test]
    fn regular_grid_reverse_and_gridline_rays_match_all_triangle_search() {
        let mut mesh = build_flat_terrain_mesh(
            TerrainSampleSpec {
                world_min: [0.0, 0.0], // 射线可以从 X 正侧逆向进入。
                world_max: [8.0, 8.0], // 仍保留跨数格的水平范围。
                resolution: [9, 9],    // 单元身份直接映射源三角顺序。
                base_height: -80.0,    // 合法高度量程。
                height_scale: 160.0,   // 当前局部山丘可抬至三米。
            },
            0.0,
        )
        .unwrap(); // 正式新图的行优先完整规则网格。
        let index = build_terrain_editing_index(&mesh).unwrap(); // 索引先建立，下面再模拟作者雕刻。
        mesh.positions[4 * 9 + 5][1] = 3.0; // 当前 Y 变化必须被反向和沿格线射线看到。
        for (origin, direction) in [
            (Vec3::new(10.0, 5.0, 4.5), Vec3::new(-1.0, -0.5, -0.05)), // 反向斜射跨多个单元。
            (Vec3::new(10.0, 5.0, 4.0), Vec3::new(-1.0, -0.45, 0.0)),  // 沿 Z=4 格线平行移动。
        ] {
            let expected = mesh
                .indices
                .chunks_exact(3)
                .enumerate()
                .filter_map(|(triangle, corners)| {
                    let [a, b, c] = [
                        corners[0] as usize,
                        corners[1] as usize,
                        corners[2] as usize,
                    ]; // 读取当前GLB三角三个角。
                    ray_triangle_distance(
                        origin,
                        direction,
                        Vec3::from_array(mesh.positions[a]),
                        Vec3::from_array(mesh.positions[b]),
                        Vec3::from_array(mesh.positions[c]),
                    )
                    .map(|distance| (triangle as u32, distance)) // 不借助规则索引的穷举真值。
                })
                .min_by(|first, second| first.1.total_cmp(&second.1))
                .unwrap(); // 两条射线都应命中山丘。
            let hit = index
                .ray_intersection(
                    &mesh.positions,
                    &mesh.indices,
                    origin.to_array(),
                    direction.to_array(),
                )
                .unwrap(); // DDA 不能在逆向或格线边界漏掉近面。
            assert!(
                (hit.distance - expected.1).abs() < 1.0e-4,
                "origin={origin:?}, direction={direction:?}"
            ); // 检查最近参数。
            assert!((hit.position[1] - (origin.y + direction.y * expected.1)).abs() < 1.0e-4); // 逆向和格线边界都保持穷举相交的真实高度。
        }
    }

    #[test]
    fn circle_visit_is_local_and_topology_neighbors_are_deduplicated() {
        let mesh = TerrainMeshInput {
            positions: vec![
                [-1.0, 0.0, -1.0],
                [1.0, 0.0, -1.0],
                [-1.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
            ], // 平面四点。
            indices: vec![0, 2, 1, 1, 2, 3], // 两个共享边三角形。
        }; // 完成测试网格。
        let index = build_terrain_editing_index(&mesh).expect("合法平面必须建立编辑索引"); // 后台索引纯函数。
        let mut visited = Vec::new(); // 收集局部访问顶点。
        index.for_each_vertex_in_circle(&mesh.positions, [-1.0, -1.0], 0.1, |vertex, _| {
            visited.push(vertex)
        }); // 只命中左上角。
        assert_eq!(visited, vec![0]); // 每顶点只登记一次且圆形过滤正确。
        assert_eq!(index.neighbors(1).collect::<Vec<_>>(), [0, 2, 3]); // 共享对角边只保留一份邻居。
        let hit = index
            .ray_intersection(
                &mesh.positions,
                &mesh.indices,
                [0.25, 3.0, 0.25],
                [0.0, -1.0, 0.0],
            )
            .expect("竖直射线必须命中真实平面三角形");
        assert!((hit.position[1]).abs() <= f32::EPSILON);
        assert!(
            index
                .ray_intersection(
                    &mesh.positions,
                    &mesh.indices,
                    [3.0, 3.0, 3.0],
                    [0.0, -1.0, 0.0]
                )
                .is_none()
        );
    } // 结束编辑索引回归。

    #[test]
    fn exported_attribute_splits_share_one_logical_topology_vertex() {
        let mesh = TerrainMeshInput {
            positions: vec![
                [-1.0, 0.0, -1.0], // NW只属于首面。
                [-1.0, 0.0, 1.0],  // SW首份属性顶点。
                [1.0, 0.0, -1.0],  // NE首份属性顶点。
                [1.0, 0.0, -1.0],  // NE因法线等属性被GLB导出器拆出的副本。
                [-1.0, 0.0, 1.0],  // SW对应副本。
                [1.0, 0.0, 1.0],   // SE只属于次面。
            ],
            indices: vec![0, 1, 2, 3, 4, 5], // 两面几何共边，但索引层完全断开。
        }; // 模拟Blender同一网格因属性边界产生的合法glTF顶点拆分。
        let index = build_terrain_editing_index(&mesh).expect("属性拆点平面必须建立逻辑编辑索引");
        assert_eq!(index.representative(3), Some(2)); // NE副本归属首次出现的稳定源顶点。
        assert_eq!(index.coincident_vertices(2).collect::<Vec<_>>(), [2, 3]); // 同XYZ成员完整保留供作者同步写入。
        assert_eq!(index.coincident_vertices(4).collect::<Vec<_>>(), [1, 4]); // SW副本同样被焊接。
        assert_eq!(index.neighbors(2).collect::<Vec<_>>(), [0, 1, 5]); // 两侧三角的一环并集按逻辑点去重且不含零长自环。
        assert_eq!(index.logical_height(&mesh.positions, 3), Some(0.0)); // 任一属性副本读取同一逻辑高度。
    } // 结束GLB属性拆点焊接回归。

    #[test]
    fn cave_layers_at_the_same_xz_remain_separate_logical_vertices() {
        let mesh = TerrainMeshInput {
            positions: vec![
                [-1.0, 1.0, -1.0],
                [1.0, 1.0, -1.0],
                [-1.0, 1.0, 1.0],
                [-1.0, -1.0, -1.0],
                [1.0, -1.0, -1.0],
                [-1.0, -1.0, 1.0],
            ], // 洞顶和洞底拥有完全相同的X/Z但不同世界Y。
            indices: vec![0, 2, 1, 3, 4, 5], // 两层分别保留合法三维三角形。
        }; // 完成多层编辑索引夹具。
        let index = build_terrain_editing_index(&mesh).expect("洞顶与洞底必须允许建立同一编辑索引");

        assert_eq!(index.representative(0), Some(0)); // 上层保持自己的稳定逻辑身份。
        assert_eq!(index.representative(3), Some(3)); // 下层不能因X/Z重合被焊到上层。
        assert_eq!(index.coincident_vertices(0).collect::<Vec<_>>(), [0]); // 上层没有属性拆点副本。
        assert_eq!(index.coincident_vertices(3).collect::<Vec<_>>(), [3]); // 下层同样独立。
    } // 结束洞穴多层焊接边界回归。
}
