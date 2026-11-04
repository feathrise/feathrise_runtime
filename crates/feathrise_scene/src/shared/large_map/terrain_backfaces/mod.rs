//! 地形背面显示覆盖：编辑器与玩法视图共用，临时材质共享原网格与纹理。

mod components; // 临时显示只在RenderApp替换材质身份，不干扰正式地表租约。
#[cfg(test)]
mod gpu_tests;
mod materials; // 四种地形材质保留各自扩展，只覆盖标准材质的剔除与背面受光。
mod resources; // 进程级预览意图、原材质句柄及有界增量队列。
mod systems; // 新地形、材质换代、偏好开关和显示恢复的唯一入口。
#[cfg(test)]
mod tests; // 四条正式材质路径从地形下方回读像素，验证开关和恢复。

pub use resources::TerrainBackfacesPreview;
pub(crate) use systems::install;
