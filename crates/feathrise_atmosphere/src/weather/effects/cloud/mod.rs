//! 动态天气云层：以相机内侧远景载体绘制世界视线投影云，不再读取球面 UV。

mod dome_mesh; // 远景载体只提供世界位置，拓扑与纹理坐标不再决定云形。
mod material; // 自定义透明材质、GPU 参数与内置 WGSL 管线。
mod noise; // 一次性生成四通道周期噪声，避免逐像素多层解析噪声成本。
mod profile; // 将连续天气、天空色与风场解析为稳定的云层外观参数。
mod systems; // 单实例生命周期、相机跟随与材质更新。

pub(crate) use material::install_dynamic_cloud_material; // WeatherPlugin 构建时登记材质管线。
pub(crate) use systems::sync_weather_cloud_curtain; // 天气效果调度只调用统一云层同步入口。

#[cfg(test)]
use material::DynamicCloudMaterial; // 最小 ECS 测试直接登记自定义材质资产表。
#[cfg(test)]
use noise::{CLOUD_NOISE_SEED, CLOUD_NOISE_SIZE, build_cloud_noise_image, periodic_value_noise}; // 周期噪声回归读取正式生成器与固定规格。
#[cfg(test)]
use profile::{
    CloudSunLight, active_virtual_delta_seconds, advance_cloud_scroll, cloud_curtain_profile,
}; // 纯逻辑测试复用正式云量、太阳边光与动画解析。
#[cfg(test)]
use systems::WeatherCloudCurtain; // ECS 生命周期测试只按自有标记统计实例。

#[cfg(test)]
mod tests; // 纯逻辑与最小 ECS 回归锁定云量、投影和相机隔离契约。
