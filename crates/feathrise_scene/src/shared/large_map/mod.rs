//! 正式大地图功能域入口，统一负责GLB选择、地形碰撞和动态水面。
//! 单张地图只声明场景配置，不直接维护重复的地图资产和水波实现。

#[cfg(any(feature = "test_panel", feature = "author_view"))] // 发布构建不登记F12可视三角网格目标或同步系统。
mod terrain_grid; // 正式地图ready标记与普通CPU地形Bevy Wireframe增量同步。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 正式构建保持作者平滑着色，不注册F12材质同步。
mod terrain_shading; // 当前大地图只改GPU法线参数，不复制网格或重建碰撞。
#[cfg(any(feature = "test_panel", feature = "author_view"))]
pub(crate) mod terrain_backfaces; // 父/子地图按正式地形身份应用背面显示偏好，关闭F12仍保持。
mod water; // 初始化可变形水面并更新背景波浪和玩家互动波纹。

#[cfg(any(feature = "test_panel", feature = "author_view"))] // 只有F12构建需要登记和同步可视地形三角网格。
pub(crate) use terrain_grid::{
    // GameScenePlugin与Clipmap专用材质共享同一显示契约。
    mark_dynamic_terrain_grid_target,
    sync_changed_regular_terrain_grid_targets,
    sync_regular_terrain_grid_visibility,
    terrain_grid_color,
}; // 完成地形网格内部系统与统一颜色重导出。
#[cfg(any(feature = "test_panel", feature = "author_view"))]
pub use terrain_shading::TerrainShadingPreview; // 场景页与玩家视角共享唯一临时地形模式。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 只有编辑器构建需要把临时预览投影到地表材质。
pub(crate) use terrain_shading::sync_large_map_shading; // GameScenePlugin统一注册跨三类大地图的增量同步。
pub use water::{
    BuiltWaterSurface, DynamicWaterMaterial, DynamicWaterMeshOverrideNumericPatch,
    DynamicWaterMeshOverrideStatus, DynamicWaterMeshOverrides, MAX_MAP_SPLINE_WATER_TRIANGLES,
    MAX_RIVER_RIBBON_SECTIONS, MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME,
    MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME, MAX_WATERFALL_RIBBON_SECTIONS, PaintedWaterMeshData,
    PaintedWaterPurpose, PaintedWaterSurface, PaintedWaterSurfaceSpawn, PaintedWaterTerrainInput,
    StagedWaterSurface, WaterGpuUploadQueue, WaterGpuUploadReceipts, WaterGpuUploadTicket,
    WaterSurfaceDescriptor, WaterSurfaceKind, WaterSurfaceMeshData, advance_water_surface_material,
    build_painted_water_mesh, build_painted_water_mesh_with_depth, build_river_water_surface,
    build_waterfall_water_surface, create_water_surface_material, spawn_painted_water_surface,
    spawn_staged_water_surface, spawn_staged_water_warmup, spawn_water_surface,
    retire_water_cpu, stage_water_surface_assets, stage_water_surface_chunk, chunk_water_surfaces, validate_spline_water_build_budget,
}; // 动态地图与F12编辑器共享湖、河、急流和瀑布的统一构建与正式GPU水材质入口。
pub(crate) use water::{
    DynamicWaterCpuInteraction, DynamicWaterMeshOverrideSystems, DynamicWaterSurface,
    DynamicWaterUniform, LargeMapWaterPlugin,
}; // Clipmap复用内部互动状态；地图聚合注册插件，placement结算复用单根处理调度边界。

pub(crate) use water::WaterRenderChunk;
pub(crate) use water::retire_water_cpu_in_world;
#[cfg(test)]
pub(crate) use water::install_water_retirement;
