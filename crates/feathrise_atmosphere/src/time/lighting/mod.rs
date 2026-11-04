// 天体光照功能域：组织世界本地天体弧线、方向光和玩法相机环境亮度同步。

mod helpers; // 年度赤纬倾斜、固定昼长弧线与光照采样纯函数。
mod systems; // 将采样结果写入方向光、相机环境光、清屏色和天空盒。

pub use helpers::dynamic_day_night_factor; // F12用同一公开纯函数计算作者基础值对应的当前运行时值，禁止复制曲线。
pub(super) use helpers::{lunar_direction, solar_direction}; // 可见天体域复用同一太阳/月亮轨迹，禁止另写一套方位数学。
pub(super) use systems::apply_world_lighting; // 插件层只需要注册统一光照应用系统。
