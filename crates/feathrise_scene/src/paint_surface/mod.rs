//! 模型与地形共用的只读表面查询；模型 BVH、连通笔刷和来源读取仅由有界后台任务调用。

mod geometry;
mod model_source;
mod paint;
mod candidates; // 三角面内细分模型作者锚点，后台生成草根候选，不逐株射线查询。
#[cfg(test)]
mod tests;

pub use geometry::{MeshSurfacePrimitive, MeshSurfaceSnapshot, SurfaceHit};
pub use model_source::{load_model_surface, load_model_surface_source, read_model_surface_source, ModelSurfaceSourceInput};
pub use paint::ResolvedSurfaceVegetation;

use bevy::prelude::*;
use feathrise_scene_content::{SceneInstancePath, SurfaceTarget, ModelSurfaceAnchor};
use crate::{ClipmapGroundHit, TerrainObjectRaySnapshot};

/// 完整实例地址与源目标一起授予查询资格；相同源对象的多个实例不能互借姿态。
#[derive(Clone)]
pub struct PaintSurfaceSnapshot {
    pub target: SurfaceTarget,
    pub instance_path: SceneInstancePath,
    pub geometry_revision: [u8; 32],
    pub geometry: PaintSurfaceGeometry,
}

#[derive(Clone)]
pub enum PaintSurfaceGeometry {
    Model { mesh: MeshSurfaceSnapshot, object_to_world: Mat4 },
    Terrain(TerrainObjectRaySnapshot),
}

#[derive(Clone, Debug)]
pub struct PaintSurfaceHit {
    pub target: SurfaceTarget,
    pub instance_path: SceneInstancePath,
    pub geometry_revision: [u8; 32],
    pub local_position: Vec3,
    pub local_normal: Vec3,
    pub position: Vec3,
    pub normal: Vec3,
    pub model_anchor: Option<ModelSurfaceAnchor>,
}

impl PaintSurfaceSnapshot {
    /// 作者与运行实例共用同一模型适配；完整实例地址只在调用方明确冻结时授予查询资格。
    pub fn from_model(target: SurfaceTarget, instance_path: SceneInstancePath,
        mesh: MeshSurfaceSnapshot, object_to_world: Mat4) -> Result<Self, String> {
        target.validate()?;
        if !matches!(target, SurfaceTarget::Model { .. }) || instance_path.terrain_id.is_some()
            || !geometry::valid_matrix(object_to_world) {
            return Err("模型表面目标、实例地址或对象姿态无效".into());
        }
        Ok(Self { target, instance_path, geometry_revision: mesh.geometry_revision(),
            geometry: PaintSurfaceGeometry::Model { mesh, object_to_world } })
    }

    /// 二维密度只消费真正的高度外表面；洞内与模型使用各自三维/稀疏载体。
    /// 缺页与失败必须保留明确状态，不能在后台批次中被当作“没有面”跳过。
    pub fn height_density_surface(&self, origin: Vec3, maximum: f32) -> ClipmapGroundHit {
        let PaintSurfaceGeometry::Terrain(terrain) = &self.geometry else { return ClipmapGroundHit::Miss; };
        let hit = match terrain.support_below_status(origin, maximum) {
            ClipmapGroundHit::Hit(hit) => hit,
            status => return status,
        };
        if let TerrainObjectRaySnapshot::Clipmap(snapshot, frame) = terrain {
            let local = frame.to_local(hit);
            if snapshot.is_three_dimensional_surface(local) { return ClipmapGroundHit::Miss; }
            return match snapshot.sample_authoritative_surface(local.xz()) {
                ClipmapGroundHit::Hit(point) if point.distance(local) < 0.05 => ClipmapGroundHit::Hit(hit),
                ClipmapGroundHit::Hit(_) => ClipmapGroundHit::Miss,
                status => status,
            };
        }
        ClipmapGroundHit::Hit(hit)
    }

    /// 种植射线读取作者几何，不依赖物理世界是否启用该对象的 Collider。
    pub fn ray_surface(&self, origin: Vec3, direction: Vec3, maximum: f32) -> Option<PaintSurfaceHit> {
        let (local_position, local_normal, position, normal, model_anchor) = match &self.geometry {
            PaintSurfaceGeometry::Model { mesh, object_to_world } => {
                let hit = mesh.ray_surface(*object_to_world, origin, direction, maximum)?;
                (hit.local_position, hit.local_normal, hit.position, hit.normal, Some(hit.anchor))
            }
            PaintSurfaceGeometry::Terrain(terrain) => {
                let (position, normal) = terrain.ray_surface(origin, direction, maximum)?;
                let frame = match terrain {
                    TerrainObjectRaySnapshot::Clipmap(_, frame) | TerrainObjectRaySnapshot::Mesh(_, frame) => frame,
                };
                let local_normal = frame.source_to_world.transpose().transform_vector3(normal).try_normalize()?;
                (frame.to_local(position), local_normal, position, normal, None)
            }
        };
        Some(PaintSurfaceHit { target: self.target.clone(), instance_path: self.instance_path.clone(),
            geometry_revision: self.geometry_revision, local_position, local_normal, position, normal, model_anchor })
    }
}
