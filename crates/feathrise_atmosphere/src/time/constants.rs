// 世界时间常量：集中定义游戏时间倍率、世界本地昼夜边界和光照基准。

/// 每个现实秒推进的游戏秒数，24 倍意味着现实一小时正好经过一个游戏日。
pub const GAME_TIME_SCALE: f32 = 24.0; // 3600 个现实秒乘 24 等于 86400 个游戏秒。
/// 一个完整游戏日包含的秒数。
pub const SECONDS_PER_DAY: f32 = 86_400.0; // 二十四小时统一换算成秒以避免逐小时累计误差。
/// 每个游戏小时包含的秒数。
pub const SECONDS_PER_HOUR: f32 = 3_600.0; // 时刻滑块和存档都通过该常量换算。
/// 固定日出时刻。
pub const SUNRISE_HOUR: f32 = 6.0; // 玩法日照窗口不随真实季节日长改变。
/// 固定日落时刻。
pub const SUNSET_HOUR: f32 = 18.0; // 玩法日照窗口始终保持十二小时。
/// 清晨环境由完整夜景开始向日景过渡的时刻。
pub(crate) const DAWN_START_HOUR: f32 = 5.0; // 五点仍保持夜景端点，随后两小时逐渐天亮。
/// 清晨环境完全进入日景的时刻。
pub(crate) const DAWN_END_HOUR: f32 = 7.0; // 七点恢复作者日景天空与完整环境光。
/// 傍晚环境开始从日景向夜景过渡的时刻。
pub(crate) const DUSK_START_HOUR: f32 = 17.0; // 十七点仍保持日景端点，随后两小时逐渐天黑。
/// 傍晚环境完全进入夜景的时刻。
pub(crate) const DUSK_END_HOUR: f32 = 19.0; // 十九点进入深蓝夜景与最低环境光。
/// 春秋分的季节照度系数。
pub const EQUINOX_LIGHT_FACTOR: f32 = 0.85; // 春秋位于冬夏照度基准的中间值。
/// 夏至的季节照度系数。
pub const SUMMER_LIGHT_FACTOR: f32 = 1.0; // 夏季保留场景作者配置的完整基准照度。
/// 冬至的季节照度系数。
pub const WINTER_LIGHT_FACTOR: f32 = 0.70; // 冬季降低直射光但仍保持可玩亮度。
/// 午夜月光相对场景日光基准的最大比例。
pub const MOONLIGHT_MAX_FACTOR: f32 = 0.04; // 固定 HN 场景仍用同一盏方向光和日间基准百分之四。
/// 第一版可见月亮与月亮方向光使用的固定角直径。
pub(crate) const MOON_ANGULAR_DIAMETER_RADIANS: f32 = 0.518_f32.to_radians(); // 接近地球平均视月直径，不把月相引入软阴影范围。
/// 正午玩法相机环境光亮度。
pub const DAY_AMBIENT_BRIGHTNESS: f32 = 80.0; // 与 Bevy 默认环境光亮度一致。
/// 午夜玩法相机环境光亮度。
pub const NIGHT_AMBIENT_BRIGHTNESS: f32 = 8.0; // 夜晚保留十分之一环境亮度以保证可玩性。
/// 午夜天空盒相对作者原始亮度的比例。
pub const NIGHT_SKY_BRIGHTNESS_FACTOR: f32 = 0.02; // 夜间只保留少量天空盒轮廓和反射。
/// 地轴倾角近似值。
pub(crate) const EARTH_OBLIQUITY_DEGREES: f32 = 23.44; // 季节照度插值以赤纬的物理上下界归一化。
