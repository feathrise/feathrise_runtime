//! 通用地形地表材质入口。
//! 【功能设计】复用原GPU草土地表的PBR扩展、离屏预热证据和RenderApp观察方式，
//! 将材质权重、生命周期与诊断提升为独立环境能力，避免地表材质继续依赖植被密度语义。

mod components; // 地表承接、体积材质身份与可提取纹理探针组件。
mod images; // 统一图片加载设置与完整失败/就绪判定。
mod material; // 声明八层权重PBR扩展、描述符和材质构造逻辑。
mod volume; // 三维作者材质的稳定ID解析和渲染属性派生。
pub use volume::{
    build_volume_field_buffer, build_volume_fields_with_puddles, build_volume_lookup,
    build_volume_surface_mesh, volume_surface_key,
};
mod render; // 注册材质插件并发布上一完整RenderApp帧的就绪证据。
mod instance_render; // 子地图候选按实例地址和世代取得独立GPU证据，父图摘要保持隔离。
mod resources; // 声明地表阶段、主世界快照和渲染帧计数器资源。
mod shaders; // 注册随crate内置的通用地表WGSL资产。
mod weather; // 地表与河湖共享固定天气GPU数据，不逐材质推进雨效。

pub use weather::{SurfaceEffectsQualityProfile, surface_effects_quality_profile};
pub use weather::{
    SurfacePuddleMask, SurfaceWeatherGpu, SurfaceWeatherUpload, surface_weather_buffer_handle,
}; // 宿主环境桥与动态水面共用入口。 // P4雨纹与水滴共用分档预算。

#[cfg(test)] // 地表和河湖共享雨纹的生产WGSL验证。
mod surface_rain_tests; // 固定九候选、质量降级和雨停短路。
#[cfg(test)] // 湿润/PBR与完整前向Deferred片元验证。
mod wet_surface_tests; // 排除陡坡和无遮罩积水并校验连续扩张。
#[cfg(test)]
mod wet_surface_test_support; // 单包与仓库集成测试共用预处理，生产接口不承担测试源码出口。
#[cfg(test)]
mod native_gpu_tests; // 无窗口真实设备编译原生前向片元，回读稀疏三维表面与积水查询。
// 含宿主 Clipmap 高度库的完整 GPU 预热契约由 tests/workspace 验证。

pub use components::{
    TerrainSurfaceCarrier, TerrainSurfaceMaterialOverride, TerrainSurfaceTextureProbe,
    TerrainSurfaceInstanceScope, TerrainSurfaceInstanceWarmup, TerrainSurfaceWarmup,
    TerrainVolumeSurface,
}; // 地图适配器安装承接标记、运行时替代租约并创建固定纹理探针。
pub use images::{
    load_terrain_surface_array, load_terrain_surface_image, terrain_surface_image_failed, terrain_surface_images_loaded,
}; // 固定采样设置和加载硬门供所有地图适配器复用。
pub use material::{
    // 对外提供固定八层材质描述和Bevy扩展材质类型。
    MAX_TERRAIN_SURFACE_LAYERS,
    TerrainSurfaceDescriptor,
    TerrainSurfaceExtension,
    TerrainSurfaceIndexedDescriptor,
    TerrainSurfaceIndexedLayer,
    TerrainSurfaceLayer,
    TerrainSurfaceMaterial,
};
pub use render::TerrainSurfacePlugin; // 统一安装材质资产、提取组件与RenderApp观察系统。
pub use resources::{
    // F3、地图生命周期和内部零权重句柄的公共出口。
    TerrainSurfaceFailure,
    TerrainSurfaceFallbackImages,
    TerrainSurfacePhase,
    TerrainSurfaceRenderStats,
    TerrainSurfaceInstanceFeedback,
    terrain_surface_warmup_can_finalize,
};
