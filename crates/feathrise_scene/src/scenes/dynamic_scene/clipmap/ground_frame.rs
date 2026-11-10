//! 倾斜网格的世界竖直承托与连续吸附；只在调用方已有的后台查询槽遍历局部作者格。
use super::*;
use feathrise_scene_core::residency::SceneSourceFrame;
use std::collections::{BTreeSet, VecDeque};

// 单次作者查询保留固定内存上界；超出局部查询范围明确失败，不能退回另一层或近似平面。
const MAX_FRAME_QUERY_CELLS: usize = 65_536;

impl ClipmapGroundSnapshot {
    /// 世界向下射线必须整体逆变换；只换算长度再沿源Y投射会在倾斜地形上横向错位。
    pub(crate) fn support_below_in_frame(&self, origin: Vec3, maximum: f32, frame: SceneSourceFrame) -> ClipmapGroundHit {
        if !origin.is_finite() || !maximum.is_finite() || maximum <= 0.0 {
            return ClipmapGroundHit::Failed;
        }
        if !self.ready { return ClipmapGroundHit::WaitingForHeight; }
        let local_origin = frame.to_local(origin);
        let direction = frame.world_to_source.transform_vector3(Vec3::NEG_Y);
        let scale = direction.length();
        if !scale.is_finite() || scale <= 0.0 { return ClipmapGroundHit::Failed; }
        let mesh_hit = self.three_dimensional_hit(local_origin, direction / scale, maximum * scale)
            .map(|(point, normal)| (point.distance(local_origin) / scale, point, normal));
        let limit = mesh_hit.map_or(maximum, |hit| hit.0);
        let base_hit = match self.authoritative_ray(local_origin, direction, limit) {
            Ok(hit) => hit,
            Err(status) => return status,
        };
        let hit = base_hit.or(mesh_hit.map(|(_, point, normal)| (point, normal)));
        let Some((point, normal)) = hit else { return ClipmapGroundHit::Miss; };
        // 真实首个面若背向世界上方就拒绝承托，不能穿过倒扣地形寻找更低的另一层。
        let normal = frame.world_to_source.transpose().transform_vector3(normal).try_normalize();
        if normal.is_some_and(|normal| normal.y > 0.05) {
            ClipmapGroundHit::Hit(frame.to_world(point))
        } else { ClipmapGroundHit::Miss }
    }

    /// 保留原命中层，沿世界XZ线段收集相邻的真实作者三角，而非固定旧Y再反变换目标。
    pub(crate) fn support_along_surface_in_frame(&self, point: Vec3, target_xz: Vec2, frame: SceneSourceFrame) -> ClipmapGroundHit {
        if !point.is_finite() || !target_xz.is_finite() { return ClipmapGroundHit::Failed; }
        if !self.ready { return ClipmapGroundHit::WaitingForHeight; }
        let local = frame.to_local(point);
        let mut triangles = Vec::new();
        if self.is_three_dimensional_surface(local) {
            for mesh in self.terrain_meshes.iter() {
                let Ok(mesh) = mesh.in_frame(frame.source_to_world) else { return ClipmapGroundHit::Failed; };
                mesh.append_surface_triangles(point.xz(), target_xz, &mut triangles);
            }
        } else {
            let start = match self.sample_authoritative_surface(local.xz()) {
                ClipmapGroundHit::Hit(start) if start.distance(local) <= 0.001 => start,
                ClipmapGroundHit::Hit(_) | ClipmapGroundHit::Miss => return ClipmapGroundHit::Miss,
                status => return status,
            };
            let Some(authority) = self.authority.as_ref() else { return ClipmapGroundHit::WaitingForHeight; };
            let Some((seed, _)) = authority.cell_and_fraction(start.xz()) else { return ClipmapGroundHit::Miss; };
            let mut queue = VecDeque::from([seed]);
            let mut visited = BTreeSet::new();
            while let Some(cell) = queue.pop_front() {
                if !visited.insert(cell) { continue; }
                if visited.len() > MAX_FRAME_QUERY_CELLS { return ClipmapGroundHit::Failed; }
                let corners = match self.authoritative_cell(cell) { Ok(corners) => corners, Err(status) => return status };
                // 洞口边所在的格保持保守断开；不把裁剪掉的二维底面当作连续桥梁。
                if self.holes.iter().any(|hole| {
                    corners[0].xz().cmplt(Vec2::from_array(hole.max)).all()
                        && corners[2].xz().cmpgt(Vec2::from_array(hole.min)).all()
                }) { continue; }
                let corners = corners.map(|corner| frame.to_world(corner));
                let mut intersects = false;
                for triangle in [[corners[0], corners[1], corners[3]], [corners[3], corners[1], corners[2]]] {
                    if feathrise_terrain::terrain_surface_intersects_segment(triangle, point.xz(), target_xz) {
                        triangles.push(triangle);
                        intersects = true;
                    }
                }
                if !intersects { continue; }
                for axis in 0..2 {
                    if cell[axis] > 0 { let mut next = cell; next[axis] -= 1; queue.push_back(next); }
                    if cell[axis] + 1 < authority.spec.cells[axis] { let mut next = cell; next[axis] += 1; queue.push_back(next); }
                }
            }
        }
        let Some(hit) = feathrise_terrain::follow_terrain_surface(point, target_xz, triangles) else { return ClipmapGroundHit::Miss; };
        let local_hit = frame.to_local(hit);
        if !self.is_three_dimensional_surface(local) && !self.segment_is_known(local, local_hit) {
            return ClipmapGroundHit::WaitingForHeight;
        }
        ClipmapGroundHit::Hit(hit)
    }

    fn authoritative_cell(&self, cell: [u64; 2]) -> Result<[Vec3; 4], ClipmapGroundHit> {
        let Some(authority) = self.authority.as_ref().filter(|authority| authority.identity.content == self.content_revision) else {
            return Err(ClipmapGroundHit::WaitingForHeight);
        };
        let spec = &authority.spec;
        let positions = [[cell[0], cell[1]], [cell[0], cell[1] + 1], [cell[0] + 1, cell[1] + 1], [cell[0] + 1, cell[1]]];
        let position = |grid: [u64; 2]| Vec2::new(
            (f64::from(spec.world_min[0]) + grid[0] as f64 * spec.actual_spacing_meters[0]) as f32,
            (f64::from(spec.world_min[1]) + grid[1] as f64 * spec.actual_spacing_meters[1]) as f32,
        );
        let min = position(positions[0]); let max = position(positions[2]);
        match self.request_authoritative_rect(min, max) {
            Ok(true) => {}, Ok(false) => return Err(ClipmapGroundHit::WaitingForHeight), Err(_) => return Err(ClipmapGroundHit::Failed),
        }
        let mut result = [Vec3::ZERO; 4];
        for (index, grid) in positions.into_iter().enumerate() {
            let height = match authority.grid_height(grid[0], grid[1]) {
                Ok(Some(height)) => height,
                Ok(None) => return Err(ClipmapGroundHit::WaitingForHeight),
                Err(_) => return Err(ClipmapGroundHit::Failed),
            };
            let p = position(grid);
            let height = self.author_stitches.height(p, height, self.height.world_min, self.height.world_max);
            result[index] = Vec3::new(p.x, height, p.y);
        }
        Ok(result)
    }

    /// DDA只经过射线实际穿越的基础格；参数始终是世界射线距离，非均匀缩放也不重解释t。
    fn authoritative_ray(&self, origin: Vec3, direction: Vec3, maximum: f32) -> Result<Option<(Vec3, Vec3)>, ClipmapGroundHit> {
        if self.is_density_surface() { return Ok(None); }
        let h = &self.height;
        let Some((entry, exit)) = ray_box(origin, direction,
            Vec3::new(h.world_min.x, h.height_min, h.world_min.y),
            Vec3::new(h.world_max.x, h.height_max, h.world_max.y)) else { return Ok(None); };
        let exit = exit.min(maximum);
        if entry > exit { return Ok(None); }
        let Some(authority) = self.authority.as_ref() else { return Err(ClipmapGroundHit::WaitingForHeight); };
        let point = origin + direction * entry;
        let Some((mut cell, fraction)) = authority.cell_and_fraction(point.xz().clamp(h.world_min, h.world_max)) else { return Ok(None); };
        for axis in 0..2 {
            let component = if axis == 0 { direction.x } else { direction.z };
            if component < 0.0 && fraction[axis] == 0.0 && cell[axis] > 0 { cell[axis] -= 1; }
        }
        let spec = &authority.spec;
        for _ in 0..MAX_FRAME_QUERY_CELLS {
            let corners = self.authoritative_cell(cell)?;
            let mut nearest: Option<(f32, Vec3)> = None;
            for triangle in [[corners[0], corners[1], corners[3]], [corners[3], corners[1], corners[2]]] {
                if let Some(t) = ray_triangle(origin, direction, triangle)
                    && t <= exit + 0.0001 && t >= entry - 0.0001
                    && !self.holes.iter().any(|hole| hole.contains((origin + direction * t).xz()))
                    && nearest.is_none_or(|old| t < old.0) {
                    if let Some(normal) = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]).try_normalize() {
                        nearest = Some((t, normal));
                    }
                }
            }
            if let Some((t, normal)) = nearest { return Ok(Some((origin + direction * t, normal))); }
            let crossing: [f32; 2] = std::array::from_fn(|axis| {
                let component = if axis == 0 { direction.x } else { direction.z };
                if component == 0.0 { return f32::INFINITY; }
                let edge = cell[axis] + u64::from(component > 0.0);
                let coordinate = f64::from(spec.world_min[axis]) + edge as f64 * spec.actual_spacing_meters[axis];
                ((coordinate - f64::from(if axis == 0 { origin.x } else { origin.z })) / f64::from(component)) as f32
            });
            let next = crossing[0].min(crossing[1]);
            if !next.is_finite() || next >= exit { return Ok(None); }
            for axis in 0..2 {
                if crossing[axis] > next { continue; }
                let component = if axis == 0 { direction.x } else { direction.z };
                if component > 0.0 {
                    cell[axis] += 1;
                    if cell[axis] >= spec.cells[axis] { return Ok(None); }
                } else {
                    let Some(value) = cell[axis].checked_sub(1) else { return Ok(None); };
                    cell[axis] = value;
                }
            }
        }
        Err(ClipmapGroundHit::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenes::dynamic_scene::{terrain_queries::TerrainObjectRaySnapshot, terrain_topology::TerrainMeshQuery};
    use avian3d::prelude::Collider;

    #[test]
    fn tilted_clipmap_support_uses_world_vertical_and_world_normal() {
        let ground = super::super::tests::snapshot();
        let ClipmapGroundHit::Hit(local) = ground.sample_authoritative_surface(Vec2::new(20.3, 15.7)) else { panic!("作者页应就绪"); };
        for rotation in [Quat::from_rotation_x(0.32), Quat::from_rotation_z(-0.29), Quat::from_euler(EulerRot::YXZ, 0.7, 0.25, -0.17)] {
            let frame = SceneSourceFrame::from_transform(Transform::from_xyz(12., -6., 8.)
                .with_rotation(rotation).with_scale(Vec3::new(1.2, 2., 0.8)));
            let expected = frame.to_world(local);
            let query = TerrainObjectRaySnapshot::Clipmap(ground.clone(), frame);
            let ClipmapGroundHit::Hit(actual) = query.support_below_status(expected + Vec3::Y * 6., 7.) else { panic!("倾斜基础作者面应有世界承托"); };
            assert!(actual.distance(expected) < 0.001, "实际{actual:?}，预期{expected:?}");
            assert_eq!(query.support_below_status(expected + Vec3::Y * 6., 5.), ClipmapGroundHit::Miss);
        }
        let frame = SceneSourceFrame::from_transform(Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::PI)));
        let expected = frame.to_world(local);
        let query = TerrainObjectRaySnapshot::Clipmap(ground.clone(), frame);
        assert_eq!(query.support_below_status(expected + Vec3::Y * 6., 7.), ClipmapGroundHit::Miss);
        let mut unavailable = ground;
        unavailable.ready = false;
        let query = TerrainObjectRaySnapshot::Clipmap(unavailable, frame);
        assert_eq!(query.support_below_status(expected + Vec3::Y * 6., 7.), ClipmapGroundHit::WaitingForHeight);
    }

    #[test]
    fn tilted_base_surface_snap_follows_actual_world_triangles() {
        let ground = super::super::tests::snapshot();
        let ClipmapGroundHit::Hit(start) = ground.sample_authoritative_surface(Vec2::new(20.3, 15.7)) else { panic!("作者页应就绪"); };
        let ClipmapGroundHit::Hit(target) = ground.sample_authoritative_surface(Vec2::new(23.7, 18.2)) else { panic!("作者页应就绪"); };
        let frame = SceneSourceFrame::from_transform(Transform::from_xyz(12., -6., 8.)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.7, 0.25, -0.17)).with_scale(Vec3::new(1.2, 2., 0.8)));
        let start = frame.to_world(start); let target = frame.to_world(target);
        let query = TerrainObjectRaySnapshot::Clipmap(ground.clone(), frame);
        let ClipmapGroundHit::Hit(actual) = query.sample_support_along_surface(start, target.xz()) else { panic!("世界XZ段应保留原倾斜作者面"); };
        assert!(actual.distance(target) < 0.001, "实际{actual:?}，预期{target:?}");
        drop(query); // 释放成功查询持有的页快照，之后才能独占修改夹具以模拟缺页。
        let mut waiting = ground;
        Arc::get_mut(waiting.authority.as_mut().unwrap()).unwrap().pages.clear();
        let query = TerrainObjectRaySnapshot::Clipmap(waiting, frame);
        assert_eq!(query.sample_support_along_surface(start, target.xz()), ClipmapGroundHit::WaitingForHeight);
    }

    #[test]
    fn tilted_three_dimensional_snap_preserves_nearby_layers() {
        let mut vertices = Vec::new(); let mut indices = Vec::new();
        for y in [0., 0.25] {
            let base = vertices.len() as u32;
            vertices.extend([Vec3::new(0., y, 0.), Vec3::new(0., y, 2.), Vec3::new(2., y + 1., 2.), Vec3::new(2., y + 1., 0.)]);
            indices.extend([[base, base + 1, base + 3], [base + 3, base + 1, base + 2]]);
        }
        let collider = Collider::trimesh(vertices, indices);
        let mut ground = super::super::tests::snapshot();
        ground.terrain_meshes = Arc::from([TerrainMeshQuery::new(collider.clone(), Mat4::IDENTITY).unwrap()]);
        let frame = SceneSourceFrame::from_transform(Transform::from_xyz(12., -6., 8.)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, 0.23, -0.19)).with_scale(Vec3::new(2., 3., 0.8)));
        for query in [TerrainObjectRaySnapshot::Mesh(collider, frame), TerrainObjectRaySnapshot::Clipmap(ground, frame)] {
            for offset in [0., 0.25] {
                let start = frame.to_world(Vec3::new(0.3, 0.15 + offset, 1.));
                let target = frame.to_world(Vec3::new(1.7, 0.85 + offset, 1.));
                let ClipmapGroundHit::Hit(actual) = query.sample_support_along_surface(start, target.xz()) else { panic!("倾斜洞层不应改选相邻面"); };
                assert!(actual.distance(target) < 0.001);
                let ClipmapGroundHit::Hit(actual) = query.support_below_status(target + Vec3::Y * 0.05, 0.1) else { panic!("世界下方应命中原洞层"); };
                assert!(actual.distance(target) < 0.001);
            }
        }
    }

    #[test]
    fn ninety_degree_clipmap_keeps_ray_pick_and_refuses_vertical_wall_support() {
        let ground = super::super::tests::snapshot();
        let ClipmapGroundHit::Hit(local) = ground.sample_surface(Vec2::new(20.3, 15.7)) else { panic!("正式显示页应就绪"); };
        for rotation in [Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)] {
            let frame = SceneSourceFrame::from_transform(Transform::from_xyz(30., 40., -20.)
                .with_rotation(rotation).with_scale(Vec3::new(2., 3., 0.5)));
            let expected = frame.to_world(local);
            let up = frame.source_to_world.transform_vector3(Vec3::Y).normalize();
            let query = TerrainObjectRaySnapshot::Clipmap(ground.clone(), frame);
            let actual = query.ray(expected + up * 10., -up).expect("90度Clipmap仍可用真实射线选中");
            assert!(actual.distance(expected) < 0.001);
        }
        // 用真正平地隔离几何倾角：90度平面是墙，不能把源朝上规则冒充世界承托。
        let mut wall = ground;
        let authority = Arc::get_mut(wall.authority.as_mut().unwrap()).unwrap();
        authority.pages.values_mut().for_each(|page| *page = super::super::super::authority::AuthorityPage::Flat);
        wall.height.height_min = 0.; wall.height.height_max = 0.;
        let frame = SceneSourceFrame::from_transform(Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)));
        let expected = frame.to_world(Vec3::new(20., 0., 15.));
        let query = TerrainObjectRaySnapshot::Clipmap(wall, frame);
        assert_eq!(query.support_below_status(expected + Vec3::Y, 2.), ClipmapGroundHit::Miss);
    }

    #[test]
    fn tilted_meter_height_bounds_include_values_outside_creation_range() {
        use feathrise_scene_content::{SparseHeightTile, SparseHeightTileKey};
        let mut ground = super::super::tests::snapshot();
        let authority = Arc::get_mut(ground.authority.as_mut().unwrap()).unwrap();
        let key = SparseHeightTileKey { x: 0, z: 0 };
        let mut tile = SparseHeightTile::blank(&authority.spec, key).unwrap();
        for z in 0..=64 { for x in 0..=96 {
            tile.set([x, z], authority.spec.encode_height(800.).unwrap()).unwrap();
        }}
        authority.pages.insert(key, super::super::super::authority::AuthorityPage::Tile(Arc::new(tile)));
        ground.height.height_max = 800.; // 已采用内容的实际范围，不是创建时的0～32米。
        let frame = SceneSourceFrame::from_transform(Transform::from_xyz(30., 40., -20.)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, 0.3, 0.4, -0.2)).with_scale(Vec3::new(2., 3., 0.5)));
        let expected = frame.to_world(Vec3::new(20.3, 800., 15.7));
        let query = TerrainObjectRaySnapshot::Clipmap(ground, frame);
        let (min, max) = query.mesh_bounds().unwrap();
        assert!(expected.cmpge(min).all() && expected.cmple(max).all());
        let ClipmapGroundHit::Hit(actual) = query.support_below_status(expected + Vec3::Y * 4., 5.) else { panic!("超出创建量程的米制高度仍应有承托"); };
        assert!(actual.distance(expected) < 0.001, "实际{actual:?}，预期{expected:?}");
    }
}
