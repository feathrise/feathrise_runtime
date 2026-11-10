//! 动态场景 GLB 地形的共享纯算法入口。
//!
//! 本 crate 不访问 ECS、Assets 或文件系统；调用方负责以有界方式冻结输入，随后可在
//! `AsyncComputeTaskPool` 中完成法线、高度网格、RGBA16 高度单元和编辑索引派生。

mod derive; // 三维网格校验、法线、空间分桶和室外最高表面高度栅格化。
mod editing; // F12笔刷复用的顶点空间索引与拓扑邻接表。
mod surface; // 吸附沿真实共享边保持当前三维承托层的后台局部算法。
mod glb; // 保留顶点顺序与三角连接关系的最小白模GLB编码器。
mod types; // 跨场景与编辑器共享的拥有输入、规格和完成值。
mod sculpt; // 任意GLB高度雕刻核心同时服务旧单地形与独立地形对象。
mod static_lod; // 普通静态三角网格的后台LOD，保留材质边界、顶点属性和原资产。
pub mod volume; // 局部体积原型的密度编辑、确定性等值面和带作者源的GLB编码。

pub use derive::{derive_terrain, derive_vertex_normals, pack_height_cells_rgba16}; // 公开后台纯计算入口。
pub use editing::{TerrainEditingIndex, TerrainRayHit, build_terrain_editing_index}; // 公开F12局部笔刷与实际三角射线索引。
pub use surface::{follow_terrain_surface, terrain_surface_intersects_segment}; // 普通GLB与Clipmap共用连续承托层，不依赖固定向下探测高度。
pub use glb::{
    DYNAMIC_TERRAIN_MESH_NAME, DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME, build_flat_terrain_glb,
    build_flat_terrain_mesh, build_white_terrain_glb, parse_dynamic_terrain_glb,
    validate_flat_terrain_glb_resolution,
}; // 公开统一名称、新图、加载与保存事务入口。
pub use types::*; // 共享稳定预算、输入和结果类型。
pub use sculpt::sculpt_height_vertices;
pub use static_lod::{
    StaticMeshCapture, StaticMeshLod, StaticMeshLodView, static_mesh_bytes, build_static_mesh_lods, copy_static_mesh_attribute_range,
    empty_static_mesh_attribute,
};
