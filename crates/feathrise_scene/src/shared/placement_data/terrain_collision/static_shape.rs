//! 静态地形的真实Trimesh查询与常数时间物理元数据，避免Avian插入时重扫全部三角形。
use avian3d::parry::{
    bounding_volume::{Aabb, BoundingSphere},
    mass_properties::MassProperties,
    math::Pose,
    query::{PointProjection, PointQuery, Ray, RayCast, RayIntersection},
    shape::{CompositeShape, FeatureId, Shape, ShapeType, TriMesh, TypedShape},
};
use bevy::prelude::*;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct StaticTerrainShape {
    mesh: Arc<TriMesh>,
    sphere: BoundingSphere,
}

#[cfg(test)]
#[path = "static_shape_cast_tests.rs"]
mod cast_tests;

impl StaticTerrainShape {
    pub(crate) fn new(mesh: TriMesh) -> Self {
        let sphere = mesh.local_bounding_sphere(); // 可能扫描顶点，只在后台构造时执行一次。
        Self {
            mesh: Arc::new(mesh),
            sphere,
        }
    }

    pub(crate) fn mesh(&self) -> &TriMesh {
        &self.mesh // 只读几何仅由后台适配读取，不在普通帧复制完整三角。
    }
}

impl RayCast for StaticTerrainShape {
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max: f32,
        solid: bool,
    ) -> Option<RayIntersection> {
        self.mesh.cast_local_ray_and_get_normal(ray, max, solid)
    }
}

impl PointQuery for StaticTerrainShape {
    fn project_local_point(&self, point: Vec3, solid: bool) -> PointProjection {
        self.mesh.project_local_point(point, solid)
    }
    fn project_local_point_and_get_feature(&self, point: Vec3) -> (PointProjection, FeatureId) {
        self.mesh.project_local_point_and_get_feature(point)
    }
}

impl Shape for StaticTerrainShape {
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }
    fn scale_dyn(&self, scale: Vec3, _: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(Self::new(
            self.mesh.as_ref().clone().scaled(scale),
        )))
    }
    fn compute_local_aabb(&self) -> Aabb {
        self.mesh.local_aabb()
    }
    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.sphere
    }
    fn compute_aabb(&self, pose: &Pose) -> Aabb {
        self.mesh.aabb(pose)
    }
    fn mass_properties(&self, _: f32) -> MassProperties {
        // 仅供RigidBody::Static的地形；无须对开放洞壁推导封闭体积及惯量。
        // Avian on_insert和Changed<Collider>会调用此接口，不能同步遍历26万三角。
        MassProperties::default()
    }
    fn shape_type(&self) -> ShapeType {
        ShapeType::Custom
    }
    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Custom(self)
    }
    fn ccd_thickness(&self) -> f32 {
        self.mesh.ccd_thickness()
    }
    fn ccd_angular_thickness(&self) -> f32 {
        self.mesh.ccd_angular_thickness()
    }
    fn feature_normal_at_point(&self, feature: FeatureId, point: Vec3) -> Option<Vec3> {
        self.mesh.feature_normal_at_point(feature, point)
    }
    fn as_composite_shape(&self) -> Option<&dyn CompositeShape> {
        Some(self.mesh.as_ref()) // 使用真正的BVH、三角形和内边缘法线约束，绝不填成凸体。
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::parry::query::{
        ContactManifold, DefaultQueryDispatcher, PersistentQueryDispatcher,
    };

    #[test]
    fn static_mesh_preserves_capsule_contacts_queries_and_zero_mass_after_scaling() {
        let mesh = TriMesh::new(
            vec![
                Vec3::new(-2.0, 0.0, -2.0),
                Vec3::new(-2.0, 0.0, 2.0),
                Vec3::new(2.0, 0.0, 2.0),
                Vec3::new(2.0, 0.0, -2.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap();
        let shape = StaticTerrainShape::new(mesh);
        let scaled = shape.scale_dyn(Vec3::new(2.0, 3.0, 4.0), 10).unwrap();
        assert_eq!(scaled.mass_properties(1.0), MassProperties::default());
        let ray = Ray::new(Vec3::new(3.0, 10.0, 0.0), Vec3::NEG_Y);
        assert_eq!(scaled.cast_local_ray(&ray, 20.0, false), Some(10.0));
        assert_eq!(scaled.project_local_point(Vec3::Y, false).point, Vec3::ZERO);
        let capsule = avian3d::parry::shape::Capsule::new_y(0.5, 0.5);
        let mut manifolds: Vec<ContactManifold<(), ()>> = Vec::new();
        let mut workspace = None;
        DefaultQueryDispatcher
            .contact_manifolds(
                &Pose::translation(0.0, 0.8, 0.0),
                scaled.as_ref(),
                &capsule,
                0.1,
                &mut manifolds,
                &mut workspace,
            )
            .unwrap();
        assert!(manifolds.iter().any(|manifold| !manifold.points.is_empty()));
        // Tnua的胶囊贴地扫掠同样走通用Composite查询，不能只验证可见射线。
        let hit = avian3d::parry::query::cast_shapes(
            &Pose::IDENTITY,
            Vec3::ZERO,
            scaled.as_ref(),
            &Pose::translation(0.0, 2.0, 0.0),
            Vec3::NEG_Y,
            &capsule,
            avian3d::parry::query::ShapeCastOptions::with_max_time_of_impact(5.0),
        )
        .unwrap()
        .unwrap();
        assert!((hit.time_of_impact - 1.0).abs() < 0.001);
    }
}
