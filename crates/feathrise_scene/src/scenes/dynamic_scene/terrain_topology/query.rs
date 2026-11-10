//! 真实三维网格的只读后台射线及表面查询，保留完整世界参数。
use super::*;
#[derive(Clone)]
pub(crate) struct TerrainMeshQuery {
    pub(super) shape: avian3d::prelude::Collider,
    matrix: Mat4,
    inverse: Mat4,
}

impl TerrainMeshQuery {
    /// 源图内的BVH保持共享；倾斜对象查询只组合固定大小矩阵，不复制整张三角网格。
    pub(crate) fn in_frame(&self, matrix: Mat4) -> Result<Self, String> {
        Self::new(self.shape.clone(), matrix * self.matrix)
    }
    pub(crate) fn new(shape: avian3d::prelude::Collider, matrix: Mat4) -> Result<Self, String> {
        let inverse = matrix.inverse();
        if !matrix.is_finite() || !inverse.is_finite() {
            return Err("三维地形世界变换无效".into());
        }
        Ok(Self {
            shape,
            matrix,
            inverse,
        })
    }
    pub(crate) fn ray(&self, origin: Vec3, direction: Vec3, max: f32) -> Option<f32> {
        let ray = avian3d::parry::query::Ray::new(
            self.inverse.transform_point3(origin),
            self.inverse.transform_vector3(direction),
        );
        self.shape.shape().cast_local_ray(&ray, max, false) // 不规范化局部方向，t始终是原世界射线的参数。
    }
    pub(crate) fn bounds(&self) -> (Vec3, Vec3) {
        let bounds = self.shape.shape().compute_local_aabb();
        let mut min = Vec3::splat(f32::INFINITY); let mut max = Vec3::splat(f32::NEG_INFINITY);
        for bits in 0..8 { let point = self.matrix.transform_point3(Vec3::new(
            if bits & 1 == 0 { bounds.mins.x } else { bounds.maxs.x },
            if bits & 2 == 0 { bounds.mins.y } else { bounds.maxs.y },
            if bits & 4 == 0 { bounds.mins.z } else { bounds.maxs.z }));
            min=min.min(point);max=max.max(point); }
        (min,max)
    }
    /// 三维工具需要完整朝向；逆转置保持非均匀缩放下的世界法线正确。
    pub(crate) fn ray_normal(
        &self,
        origin: Vec3,
        direction: Vec3,
        max: f32,
    ) -> Option<(f32, Vec3)> {
        let ray = avian3d::parry::query::Ray::new(
            self.inverse.transform_point3(origin),
            self.inverse.transform_vector3(direction),
        );
        let hit = self
            .shape
            .shape()
            .cast_local_ray_and_get_normal(&ray, max, false)?;
        // Parry把射线法线翻向来射方向；承托分类必须读取真实特征朝向，不能把洞顶背面当作地面。
        // 正式地形以Custom包装TriMesh，as_trimesh不可见；特征接口同时保留包装和原网格的绕序。
        let local_normal = self
            .shape
            .shape()
            .feature_normal_at_point(hit.subshape, hit.feature, ray.point_at(hit.time_of_impact))
            .unwrap_or(hit.normal);
        let normal = self
            .inverse
            .transpose()
            .transform_vector3(local_normal)
            .try_normalize()?;
        Some((hit.time_of_impact, normal))
    }
    /// 只收集XZ短线段穿过的真实承托三角；必须在调用方的唯一后台查询槽中使用。
    pub(crate) fn append_surface_triangles(
        &self,
        start: Vec2,
        end: Vec2,
        triangles: &mut Vec<[Vec3; 3]>,
    ) {
        if !start.is_finite() || !end.is_finite() {
            return;
        }
        // 正式地形用StaticTerrainShape包装Trimesh；借用Composite接口才能保留其真实BVH。
        let shape = self.shape.shape();
        let Some(mesh) = shape.as_composite_shape() else {
            return;
        };
        let minimum = start.min(end) - Vec2::splat(0.0001);
        let maximum = start.max(end) + Vec2::splat(0.0001);
        for triangle_id in mesh.bvh().leaves(|node| {
            // 将节点包围盒投到世界XZ再剪枝；斜置/缩放地形不能用无穷高逆变换柱纳入整图。
            let center = self.matrix.transform_point3(node.center());
            let half = (node.maxs() - node.mins()) * 0.5;
            let world_half = self.matrix.x_axis.truncate().abs() * half.x
                + self.matrix.y_axis.truncate().abs() * half.y
                + self.matrix.z_axis.truncate().abs() * half.z;
            let node_minimum = (center - world_half).xz();
            let node_maximum = (center + world_half).xz();
            node_maximum.cmpge(minimum).all() && node_minimum.cmple(maximum).all()
        }) {
            mesh.map_part_at(triangle_id, &mut |pose, part, _| {
                let Some(triangle) = part.as_triangle() else {
                    return;
                };
                let world_triangle = [triangle.a, triangle.b, triangle.c].map(|point| {
                    let local_point = pose.map_or(point, |pose| pose.transform_point(point));
                    self.matrix.transform_point3(local_point)
                });
                // 矩形仅用于BVH粗筛；真正保存的工作集沿线段增长，不能按对角矩形面积增长。
                if feathrise_terrain::terrain_surface_intersects_segment(world_triangle, start, end) {
                    triangles.push(world_triangle);
                }
            });
        }
    }
    pub(super) fn matches(&self, shape: &avian3d::prelude::Collider, matrix: Mat4) -> bool {
        std::ptr::eq(self.shape.shape().as_ref(), shape.shape().as_ref()) && self.matrix == matrix
    }
    pub(crate) fn contains_surface(&self, point: Vec3) -> bool {
        let local = self.inverse.transform_point3(point);
        let projection = self.shape.shape().project_local_point(local, false);
        self.matrix
            .transform_point3(projection.point)
            .distance_squared(point)
            < 0.000001
    }
}
