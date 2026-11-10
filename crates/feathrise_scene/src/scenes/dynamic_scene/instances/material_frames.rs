//! 每个实例只更新自己持有的小材质参数；图片、网格和稀疏高度页不随整体位置重建。

use bevy::prelude::*;
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;
use feathrise_scene_core::residency::{InstanceFrame, InstanceQuery, InstanceResMut};
use super::super::{clipmap::{ClipmapHeightMaterial, SparseClipmapSurfaceMaterial, SparseHeightPageRuntime},
    water::ClipmapWaterMaterial};

pub(super) fn bind(frame: InstanceFrame, mut pages: InstanceResMut<SparseHeightPageRuntime>,
    puddles: feathrise_scene_core::residency::InstanceRes<feathrise_environment::terrain_surface::SurfacePuddleMask>,
    puddle_state: feathrise_scene_core::residency::InstanceRes<crate::shared::surface_weather::PuddleRuntimeState>,
    terrains: InstanceQuery<&MeshMaterial3d<TerrainSurfaceMaterial>>,
    sparse: InstanceQuery<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
    fallback: InstanceQuery<&MeshMaterial3d<ClipmapHeightMaterial>>,
    waters: InstanceQuery<&MeshMaterial3d<ClipmapWaterMaterial>>,
    mut terrain_materials: ResMut<Assets<TerrainSurfaceMaterial>>,
    mut sparse_materials: ResMut<Assets<SparseClipmapSurfaceMaterial>>,
    mut fallback_materials: ResMut<Assets<ClipmapHeightMaterial>>,
    mut water_materials: ResMut<Assets<ClipmapWaterMaterial>>) {
    let matrix = frame.frame().source_to_world;
    let bounds = puddle_state.world_bounds();
    pages.set_source_frame(matrix);
    for handle in &terrains {
        if terrain_materials.get(&handle.0).is_some_and(|material| material.extension.source_frame() != matrix) {
            terrain_materials.get_mut(&handle.0).unwrap().extension.set_source_frame(matrix);
        }
        if terrain_materials.get(&handle.0).is_some_and(|material| material.extension.follows_global_puddle_mask() || material.extension.puddle_mask() != &puddles.image || material.extension.source_puddle_bounds() != bounds) {
            terrain_materials.get_mut(&handle.0).unwrap().extension.set_source_puddle_mask(puddles.image.clone(), bounds);
        }
    }
    for handle in &sparse {
        // 地表笔划、保存热更会替换内层PBR，而高度扩展可能早已绑定当前姿态；两层必须独立验收。
        // 否则缩放42倍后，身份矩阵会把源纹素当成世界米，只在整块网格中央显示一小片。
        if sparse_materials.get(&handle.0).is_some_and(|material| material.extension.params.source_to_world != matrix
            || material.base.extension.source_frame() != matrix) {
            let mut material = sparse_materials.get_mut(&handle.0).unwrap();
            material.extension.set_source_frame(matrix);
            material.base.extension.set_source_frame(matrix);
        }
        if sparse_materials.get(&handle.0).is_some_and(|material| material.base.extension.follows_global_puddle_mask() || material.base.extension.puddle_mask() != &puddles.image || material.base.extension.source_puddle_bounds() != bounds) {
            sparse_materials.get_mut(&handle.0).unwrap().base.extension.set_source_puddle_mask(puddles.image.clone(), bounds);
        }
    }
    for handle in &fallback {
        if fallback_materials.get(&handle.0).is_some_and(|material| material.extension.params.source_to_world != matrix) {
            fallback_materials.get_mut(&handle.0).unwrap().extension.set_source_frame(matrix);
        }
    }
    for handle in &waters {
        if water_materials.get(&handle.0).is_some_and(|material| material.extension.source_frame() != matrix) {
            water_materials.get_mut(&handle.0).unwrap().extension.set_source_frame(matrix);
        }
    }
}

#[cfg(test)]
#[path = "material_frames_tests.rs"]
mod tests;
