//! 新场景 Geometry Clipmap 的可编辑多层组合网格拓扑与相机环带布局。
//!
//! 本模块根据编译后的可变内外边界生成世界固定环带组合Mesh和相机吸附位置；高度页、材质、碰撞及
//! 保存由各自系统负责。环带内缘逐边拆成前一级的格距，并要求顶点
//! 着色器从 `ATTRIBUTE_UV_1.x` 直接读取每个顶点的几何 Mip及世界稳定中心；
//! `ATTRIBUTE_UV_1.y`保留为零。粗层按自身格距吸附，内缘沿用上一细层
//! 中心，使相邻级在同一世界坐标读取同一高度值。场景外裁切必须由后续材质在
//! 片元阶段按世界边界执行。

mod authority; // 相机无关的局部 Mip 0 作者高度页；摆放和 Mesh 植被共享。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 发布构建不编译高度感知F12网格覆盖层。
mod grid; // 共享原Mesh与正式高度页的专用线模式纯渲染子实体。
mod ground;
pub(super) mod material;
pub(super) mod overlay; // F12 植被世界投影覆盖，共享正式位移网格。
mod raytracing; // 世界固定真实高度代理，与移动光栅环带严格分开。
mod residency; // 相机附近稀疏块异步编码为固定预算 GPU 页，并包装现有地表材质。
mod topology; // 可变环带组合网格、共享接缝与有限场景内的相机吸附位置。 // 只读已采用量化页与精确后台笔刷射线查询。
pub(super) mod stitches;
pub(crate) use authority::{
    AuthoritativeHeightRuntime, clear_authoritative_height_pages, drive_authoritative_height_pages,
};
#[cfg(test)]
pub(crate) use ground::{test_authoring_ground_snapshot, test_ground_snapshot};
pub(crate) use ground::{
    ClipmapGroundContentRevision, ClipmapGroundQueryRevision, clear_clipmap_ground_surface,
    sync_clipmap_ground_surface,
}; // 正式显示更新后发布，离场立即撤销。 // 水体CPU足迹测试复用同一量化页夹具。
pub use ground::{
    ClipmapGroundHit, ClipmapGroundPublishSet, ClipmapGroundSnapshot, DynamicClipmapGroundSurface,
}; // 作者工具只消费完整同代地表。 // 显示分页采用后再驱动唯一权威IO，作者消费者只读取不可变快照。

#[cfg(any(feature = "test_panel", feature = "author_view"))] // 动态场景插件只在F12构建安装专用材质和同步系统。
pub(crate) use grid::{ClipmapTerrainGridPlugin, add_instance_grid_systems}; // 实例调度复用同一覆盖系统，发布包不注册额外材质。
pub use material::SparseClipmapSurfaceMaterial;
pub(crate) use material::{
    CLIPMAP_TILE_CELLS, ClipmapFlatHeightPage, ClipmapHeightAtlas, ClipmapHeightExtension,
    ClipmapHeightMaterial, ClipmapHeightMaterialPlugin, ClipmapHeightUniform,
    SparseClipmapSurfaceLease, encode_page_table_entry,
};
pub use raytracing::{
    ClipmapRaytracingAnchor, ClipmapRaytracingInstanceRoute, ClipmapRaytracingPhase,
    ClipmapRaytracingPublishSet, ClipmapRaytracingRequest, ClipmapRaytracingStatus,
};
pub(crate) use raytracing::{ClipmapRaytracingPlugin, ClipmapRaytracingRuntime};
pub(crate) use residency::{
    SparseHeightPageRuntime,
    clear_sparse_height_pages,
    drive_sparse_display_gpu_transaction,
    drive_sparse_height_pages,
    reap_retired_sparse_height_pages, // 离场后仍归还真实完成的分页任务槽。
    sync_sparse_clipmap_surface,      // GPU显示页独立于玩家中心碰撞发布。
};
#[cfg(test)]
pub(crate) use topology::ClipmapConfig;
pub(crate) use topology::{ClipmapBounds, build_display_lod_mesh, snapped_display_focus}; // 正式运行时只消费可变编译计划；物理由独立固定块构造。 // 旧固定模板仅作为新可变环带的等价拓扑测试夹具。
