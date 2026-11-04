// 世界时间资源：声明全局唯一的环境月份、日期与当日秒数。

use bevy::prelude::*; // Resource 派生让时钟可以被所有玩法系统共享。

/// 全局环境时钟；月份循环只服务光照季节，不代表主线剧情年份。
#[derive(Resource, Clone, Debug, PartialEq)] // 测试通过值比较验证推进、设置与恢复结果。
pub struct WorldClock {
    pub(crate) month: u8, // 一月至十二月，所有写入均通过规范化接口完成。
    pub(crate) day: u8,   // 当月日期，使用普通非闰年的月份天数约束。
    pub(crate) seconds_since_midnight: f32, // 允许 86400 表示测试面板暂停时的 24:00 哨兵。
}

/// 天气等环境系统提供的通用光照修正；最终渲染组件仍只由世界时间系统写入。
#[derive(Resource, Clone, Debug, PartialEq)] // 天气逐帧更新该资源，测试通过值比较验证中性默认与修正结果。
pub struct WorldEnvironmentModifiers {
    pub direct_light_factor: f32,        // 方向光照度乘数，晴天中性值为一。
    pub ambient_light_factor: f32,       // 玩法相机环境光亮度乘数，晴天中性值为一。
    pub minimum_ambient_brightness: f32, // 恶劣天气夜间仍需保留的最低可玩环境亮度。
    pub sky_brightness_factor: f32,      // 清屏天空与天空盒共用亮度乘数，阴雨天气用于压暗晴空背景。
    pub light_tint: Color,               // 天气希望混入太阳、月光和环境光的目标色。
    pub light_tint_strength: f32,        // 光色混合比例，零表示完全保留天体系统原色。
    pub sky_tint: Color,                 // 天气用于乘算清屏天空色温的目标色，白色为中性。
    pub sky_tint_strength: f32,          // 天空乘色比例，零表示完全保留昼夜与晨昏颜色。
}
