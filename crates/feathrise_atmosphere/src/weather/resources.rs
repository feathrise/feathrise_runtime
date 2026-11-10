// 天气资源：声明私有状态机、调试覆盖和供其他玩法系统读取的连续输出。

use bevy::color::LinearRgba; // 连续天气色调使用线性颜色，便于无跳变插值。
use bevy::prelude::*; // Resource 派生让全局天气状态可以由ECS系统共享。

use crate::weather::climate::WeatherClimateProfile; // 动态场景只发布已登记静态气候值，不复制状态机。
use crate::weather::types::{WeatherGameplayModifiers, WeatherKind, WeatherRegion}; // 资源字段复用公开普通天气类型。

/// 当前动态场景从清单解析出的正式天气气候。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct DynamicSceneWeatherClimate {
    profile: Option<WeatherClimateProfile>, // None表示清单尚未就绪，天气模拟暂不推进动态场景。
}

impl DynamicSceneWeatherClimate {
    /// 原子采用当前动态场景完整气候配置。
    pub fn set(&mut self, profile: WeatherClimateProfile) {
        self.profile = Some(profile); // 小型Copy配置同帧生效，不创建后台或逐帧分配。
    }

    /// 离开或切换动态场景时撤销旧清单所有权。
    pub fn clear(&mut self) {
        self.profile = None; // 下一张图清单完成校验前不沿用旧场景局部权重。
    }

    /// 返回当前已经完整采用的动态场景气候。
    pub const fn profile(&self) -> Option<WeatherClimateProfile> {
        self.profile // 天气推进系统只读该轻量快照。
    }
}

/// 全局天气状态机；字段仅由规范化接口和模拟系统写入。
#[derive(Resource, Clone, Debug, PartialEq)] // 单元测试通过值比较验证存档恢复和确定性推进。
pub struct WeatherState {
    pub(crate) surface_wetness: f32, // 地表湿润度，范围零至一，保留自然降雨历史。
    pub(crate) puddle_amount: f32,   // 积水量，范围零至一，独立缓慢形成与消退。
    pub(crate) surface_effect_time: f32, // 暂停感知视觉秒，以4096秒周期限制长期浮点精度损失。

    pub(crate) current: WeatherKind, // 当前稳定天气或过渡起点。
    pub(crate) target: WeatherKind,  // 过渡目标始终与当前节点相邻。
    pub(crate) queued_target: Option<WeatherKind>, // 多段剧情过渡尚未到达的最终天气意图。
    pub(crate) queued_transition_duration_game_seconds: f32, // 排队路径每一段使用的规范化过渡时长。
    pub(crate) pending_minimum_hold_game_seconds: f32, // 到达剧情最终目标后需要保证的最低稳定游戏秒数。
    pub(crate) transition_progress: f32,               // 零至一的过渡进度。
    pub(crate) transition_duration_game_seconds: f32,  // 当前过渡总游戏秒数。
    pub(crate) stable_remaining_game_seconds: f32,     // 稳定期剩余游戏秒数。
    pub(crate) region: WeatherRegion,                  // 所有场景使用中性全局连续性。
    pub(crate) seed: u64,                              // 确定性随机基础种子。
    pub(crate) sequence: u64,                          // 已消费随机值的稳定序号。
} // 结束全局天气状态声明。

/// 运行时天气强制模式与过渡预览比例。
#[derive(Resource, Clone, Copy, Debug, PartialEq)] // F12 调试控件每帧读写轻量值，不进入场景或正式设置文件。
pub struct WeatherDebugOverride {
    pub(crate) forced: Option<WeatherKind>, // None表示自动天气，一至五模式对应固定天气。
    pub(crate) preview: f32,                // 零显示自动输出，一显示完整强制天气效果。
    pub(crate) preview_override_active: bool, // 自动模式下仅在用户拖过预览滑块后临时覆盖真实过渡进度。
} // 结束天气调试覆盖声明。

/// F12 作者视口的天气表现锚点开关。
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct WeatherAuthoringPreview {
    active: bool, // true 时云、雨、雾围绕作者预览相机定位；天气数值仍只来自唯一正式输出。
}

impl Default for WeatherAuthoringPreview {
    fn default() -> Self {
        Self {
            active: false, // F12关闭时完全不改变正式玩法天气链路。
        }
    }
}

impl WeatherAuthoringPreview {
    /// 构造只负责摄像机定位的作者预览状态。
    pub const fn new(active: bool) -> Self {
        Self { active }
    }

    /// 当前是否由F12作者相机接管天气视觉锚点。
    pub const fn active(&self) -> bool {
        self.active // 效果系统据此在主相机和作者相机之间选择唯一目标。
    }
}

/// 解析后的连续天气环境参数。
#[derive(Resource, Clone, Copy, Debug, PartialEq)] // 光照、雾、粒子、风和玩法系统共享同一帧输出，音频可在资源补齐后复用。
pub struct WeatherOutput {
    pub surface_wetness: f32,     // 地表湿润度，范围零至一，保留自然降雨历史。
    pub puddle_amount: f32,       // 积水量，范围零至一，独立缓慢形成与消退。
    pub surface_effect_time: f32, // 暂停感知视觉秒，以4096秒周期限制长期浮点精度损失。

    /// 云层覆盖程度。
    pub cloud_cover: f32, // 零至一供云幕材质和天空压暗使用。
    /// 降水强度。
    pub precipitation: f32, // 零至一供雨粒子使用，并为后续独立雨声资源预留同源输入。
    /// 归一化能见度。
    pub visibility: f32, // 零至一供DistanceFog和玩法修正参考。
    /// 归一化风力。
    pub wind: f32, // 零至一供雨向、云层和植被统一读取。
    /// 水平世界风向。
    pub wind_direction: Vec2, // 归一化+X东、+Z南方向供雨、云和植被读取同一向量。
    /// 直射天体光倍率。
    pub direct_light_factor: f32, // 由世界时间最终乘入太阳或月光照度。
    /// 环境光倍率。
    pub ambient_factor: f32, // 由世界时间最终乘入昼夜环境光。
    /// 天气要求的环境光最低亮度。
    pub min_ambient: f32, // 防止大雨与午夜叠加成不可玩全黑画面。
    /// 天空亮度倍率。
    pub sky_factor: f32, // 正式天空色和玩法天空盒统一参考该值。
    /// 天气色调。
    pub tint: LinearRgba, // 线性色调由光照与雾颜色共同使用。
    /// 战斗与AI连续修正。
    pub gameplay: WeatherGameplayModifiers, // 只影响视认、瞄准和火铳动作速度。
} // 结束连续天气输出声明。
