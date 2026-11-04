//! 世界氛围领域门面。
//!
//! 时间子域负责日历、世界本地天体轨迹和最终光照写入；天气子域负责连续环境修正、
//! 雾、云、雨与统一风场。两个插件入口独立保留，以维持组合根原有注册顺序。

pub mod skybox; // 地图作者动态天空/天空盒与 IBL 的异步加载、热更新、校验及回退。
pub mod time;
pub mod weather;

pub use skybox::{
    AuthoredSkyMode, AuthoredSkyboxPhase, AuthoredSkyboxRequest, AuthoredSkyboxStatus,
}; // 场景与 F12 共享唯一互斥天空模式、资源请求和诊断入口。
pub use time::{
    WorldCelestialLight, WorldCelestialLightKind, WorldClock, WorldEnvironmentModifiers,
    WorldSkyProfile, WorldTimePlugin, WorldTimeSnapshot, WorldTimeSystemSet,
    dynamic_day_night_factor,
};
pub use weather::{
    DynamicMapWeatherClimate, UNIFORM_CLIMATE_PROFILE, WeatherAuthoringPreview,
    WeatherClimateProfile, WeatherCommand, WeatherDebugOverride, WeatherGameplayModifiers,
    WeatherKind, WeatherOutput, WeatherPlugin, WeatherPreviewCamera, WeatherRegion,
    WeatherSnapshot, WeatherState, WeatherSystemSet,
};
