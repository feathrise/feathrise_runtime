//! 地表和河湖共享的固定天气GPU桥；不依赖项目天气或地图作者crate。

mod quality;
mod resources; // 公开64字节状态和上传调度标签。
mod systems; // 固定缓冲安装与变更上传。
mod types; // CPU/GPU共用表面质量预算。 // 四档固定映射与峰值上界。

pub use quality::surface_effects_quality_profile;
pub use types::SurfaceEffectsQualityProfile; // 宿主和水滴池共用值类型。 // 不创建另一套设置状态。

pub use resources::{SurfacePuddleMask, SurfaceWeatherGpu, SurfaceWeatherUpload}; // 宿主在上传前写入连续状态。
pub(super) use systems::install_surface_weather;
pub use systems::surface_weather_buffer_handle; // 地表和水材质共用同一固定资产身份。 // 地表插件统一安装一次。
