//! 场景容器与独立地形对象的共享合同；读取旧图不移动文件、不隐式生成地面。

mod types;
mod validation;
#[cfg(test)]
mod tests;

pub use types::*;
pub use validation::validate_terrain_objects;

use crate::{SceneManifestAsset, SceneTerrainDescriptor, SceneTerrainKind};

impl SceneTerrainDescriptor {
    /// 空描述只服务旧调用方的容器兼容；新场景的JSON不保存这份占位数据。
    pub fn empty() -> Self {
        Self {
            resolved_stitches: Default::default(),
            kind: SceneTerrainKind::Empty,
            collision_mode: Default::default(),
            collision_lods: Vec::new(),
            display_lods: None,
            player_collision_preload_radius_meters: 16.0,
            role_collision_preload_radius_meters: None,
            player_collision_chunk_cells: None,
            role_collision_chunk_cells: None,
            player_collision_span_meters: None,
            role_collision_span_meters: None,
            glb_path: String::new(),
            height_path: None,
            height_texture_path: None,
            resolution: [0, 0],
            base_height: 0.0,
            height_scale: 1.0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.kind == SceneTerrainKind::Empty
    }
}

impl SceneManifestAsset {
    /// 旧作者工具仅映射显式兼容主对象；新对象从自己的作者域读取，不回退为空场景造地面。
    pub fn legacy_terrain(&self) -> &SceneTerrainDescriptor {
        self.terrains.as_ref().and_then(|objects| objects.first())
            .filter(|object| object.id == TerrainId::legacy()).map_or(&self.terrain, |object| &object.terrain)
    }
    /// 显式空集合必须继续为空；只有没有新字段的旧图拥有一个兼容主地形。
    pub fn terrain_count(&self) -> usize {
        self.terrains.as_ref().map_or_else(
            || usize::from(!self.terrain.is_empty()),
            |objects| objects.len(),
        )
    }

    /// 第一笔对象操作才建立新格式；旧路径及作者数据保持原样。
    pub fn terrain_objects_for_edit(&self) -> std::sync::Arc<[SceneTerrainObject]> {
        self.terrains.clone().unwrap_or_else(|| {
            if self.terrain.is_empty() {
                return Default::default();
            }
            vec![SceneTerrainObject {clipmap_stitches: Default::default(),
                mesh_lod: None,
                id: TerrainId::legacy(),
                display_name: "原主地形".to_owned(),
                enabled: true,
                transform: Default::default(),
                world_min: self.world_min,
                world_max: self.world_max,
                terrain: self.terrain.clone(),
                voxel: None,
                surface_path: None,
                vegetation_path: None,
                water_path: None,
                puddles: self.puddles.clone(),
            }].into()
        })
    }
}
