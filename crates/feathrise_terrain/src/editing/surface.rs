//! 普通地形吸附只取局部BVH/规则网格候选，沿真实共享边保持所选三维承托层。
use super::*;
use bevy::math::{Vec2, Vec3Swizzles};

impl TerrainEditingIndex {
    /// 此查询在后台槽运行；不新增整图三角邻接表，不复制包围矩形内未穿越的面。
    pub fn sample_connected_surface(
        &self,
        positions: &[[f32; 3]],
        indices: &[u32],
        point: Vec3,
        target_xz: Vec2,
    ) -> Option<Vec3> {
        if positions.len() != self.vertex_count
            || indices.len() != self.triangle_count * 3
            || !point.is_finite()
            || !target_xz.is_finite()
        {
            return None;
        }
        let minimum = point.xz().min(target_xz) - Vec2::splat(0.001);
        let maximum = point.xz().max(target_xz) + Vec2::splat(0.001);
        let mut triangles = Vec::new();
        let mut visit = |id: u32| {
            let Some(face) = indices.get(id as usize * 3..id as usize * 3 + 3) else {
                return;
            };
            let triangle =
                std::array::from_fn(|axis| Vec3::from_array(positions[face[axis] as usize]));
            if crate::terrain_surface_intersects_segment(triangle, point.xz(), target_xz) {
                triangles.push(triangle);
            }
        };
        if let Some(grid) = self.grid {
            if maximum.x < grid.world_min[0]
                || minimum.x > grid.world_max[0]
                || maximum.y < grid.world_min[1]
                || minimum.y > grid.world_max[1]
            {
                return None;
            }
            let cell = |value: f32, axis: usize, count: usize| {
                (((value - grid.world_min[axis]) / grid.step[axis])
                    .floor()
                    .max(0.0) as usize)
                    .min(count - 2)
            };
            for row in cell(minimum.y, 1, grid.height)..=cell(maximum.y, 1, grid.height) {
                for column in cell(minimum.x, 0, grid.width)..=cell(maximum.x, 0, grid.width) {
                    let first = ((row * (grid.width - 1) + column) * 2) as u32;
                    visit(first);
                    visit(first + 1);
                }
            }
        } else {
            let mut stack = vec![0_u32];
            while let Some(id) = stack.pop() {
                let node = self.bvh_nodes.get(id as usize)?;
                if node.maximum[0] < minimum.x
                    || node.minimum[0] > maximum.x
                    || node.maximum[2] < minimum.y
                    || node.minimum[2] > maximum.y
                {
                    continue;
                }
                if node.count == 0 {
                    stack.push(node.first);
                    stack.push(node.second);
                } else {
                    for &triangle in self
                        .triangle_order
                        .get(node.first as usize..node.first as usize + usize::from(node.count))?
                    {
                        visit(triangle);
                    }
                }
            }
        }
        crate::follow_terrain_surface(point, target_xz, triangles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bvh_support_snaps_uphill_without_falling_to_another_layer() {
        let mut mesh = TerrainMeshInput {
            positions: Vec::new(),
            indices: Vec::new(),
        };
        for offset in [0.0, 0.2] {
            let base = mesh.positions.len() as u32;
            mesh.positions.extend([
                [-2.0, -4.0 + offset, -2.0],
                [2.0, 4.0 + offset, -2.0],
                [2.0, 4.0 + offset, 2.0],
                [-2.0, -4.0 + offset, 2.0],
            ]);
            mesh.indices
                .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
        }
        let index = build_terrain_editing_index(&mesh).unwrap();
        assert_eq!(
            index.sample_connected_surface(
                &mesh.positions,
                &mesh.indices,
                Vec3::new(0.6, 1.2, 0.0),
                Vec2::new(1.0, 0.0)
            ),
            Some(Vec3::new(1.0, 2.0, 0.0))
        );
        assert_eq!(
            index.sample_connected_surface(
                &mesh.positions,
                &mesh.indices,
                Vec3::new(0.4, 0.8, 0.0),
                Vec2::ZERO
            ),
            Some(Vec3::ZERO)
        );
        assert_eq!(
            index.sample_connected_surface(
                &mesh.positions,
                &mesh.indices,
                Vec3::new(0.6, 1.2, 0.0),
                Vec2::new(3.0, 0.0)
            ),
            None
        );
    }
    #[test]
    fn regular_grid_support_uses_the_same_surface_trace() {
        let mut mesh = crate::build_flat_terrain_mesh(
            crate::TerrainSampleSpec {
                resolution: [3, 3],
                world_min: [-2.0, -2.0],
                world_max: [2.0, 2.0],
                base_height: -10.0,
                height_scale: 20.0,
            },
            0.0,
        )
        .unwrap();
        for point in &mut mesh.positions {
            point[1] = point[0] * 2.0;
        }
        let index = build_terrain_editing_index(&mesh).unwrap();
        assert!(index.grid.is_some());
        assert_eq!(
            index.sample_connected_surface(
                &mesh.positions,
                &mesh.indices,
                Vec3::new(0.6, 1.2, 0.0),
                Vec2::new(1.0, 0.0)
            ),
            Some(Vec3::new(1.0, 2.0, 0.0))
        );
    }
}
