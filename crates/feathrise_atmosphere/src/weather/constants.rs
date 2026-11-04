// 天气常量：集中定义持续时间、过渡长度、输出下限和确定性随机种子。

use bevy::math::Vec2; // 中性默认风向供天气输出、雨、云和植被共同读取。

/// 一小时包含的游戏秒数。
pub const GAME_SECONDS_PER_HOUR: f32 = 3_600.0; // 持续时间统一使用游戏秒，避免与现实秒混淆。
/// 一分钟包含的游戏秒数。
pub const GAME_SECONDS_PER_MINUTE: f32 = 60.0; // 晨雾和天气过渡使用分钟级配置。
/// 新游戏晴天至少保持的游戏时长。
pub const INITIAL_CLEAR_GAME_SECONDS: f32 = 4.0 * GAME_SECONDS_PER_HOUR; // 玩家进入新游戏后不会立刻遇到随机天气切换。
/// 剧情命令未指定时使用的天气过渡时长。
pub const DEFAULT_TRANSITION_GAME_SECONDS: f32 = 25.0 * GAME_SECONDS_PER_MINUTE; // 普通过渡位于设计要求二十至三十分钟的中点。
/// 小雨和大雨互相切换时使用的快速过渡时长。
pub(crate) const RAIN_TRANSITION_GAME_SECONDS: f32 = 10.0 * GAME_SECONDS_PER_MINUTE; // 雨势变化比完整天色变化更快。
/// 普通过渡的最短游戏时长。
pub(crate) const MIN_TRANSITION_GAME_SECONDS: f32 = 20.0 * GAME_SECONDS_PER_MINUTE; // 自动天气至少用二十分钟完成天色衔接。
/// 普通过渡的最长游戏时长。
pub(crate) const MAX_TRANSITION_GAME_SECONDS: f32 = 30.0 * GAME_SECONDS_PER_MINUTE; // 自动天气最多用三十分钟完成天色衔接。
/// 雾天气允许开始抽取的最早时刻。
pub(crate) const MIST_GENERATION_START_HOUR: f32 = 3.0; // 凌晨三点后才会形成晨雾。
/// 雾天气允许开始抽取的最晚时刻。
pub(crate) const MIST_GENERATION_END_HOUR: f32 = 8.0; // 八点以后不再生成新的薄雾过程。
/// 雾天气开始主动消散的时刻。
pub(crate) const MIST_DISPERSAL_START_HOUR: f32 = 9.5; // 九点半开始用半小时平滑散尽晨雾。
/// 雾天气必须完全消失的时刻。
pub(crate) const MIST_DISPERSAL_END_HOUR: f32 = 10.0; // 十点后不能继续保留雾天气状态。
/// 天气系统给玩法相机建议的最低环境光亮度。
pub const MIN_PLAYABLE_AMBIENT: f32 = 5.0; // 大雨与午夜叠加时仍需辨认地形和角色。
/// 不绑定地域的默认单位风向，沿世界+X轴。
pub const DEFAULT_WIND_DIRECTION: Vec2 = Vec2::X; // 雨、云和植被共享该退路，避免各表现层自行选择方向。
/// 默认确定性天气随机种子。
pub const DEFAULT_WEATHER_SEED: u64 = 0x57EA_7E42_474C_4F42; // 固定中性种子让新会话初始天气序列可复现。
/// 损坏快照允许保留的最大稳定期。
pub(crate) const MAX_STABLE_GAME_SECONDS: f32 = 30.0 * 24.0 * GAME_SECONDS_PER_HOUR; // 防止异常存档让天气锁死数月。
/// 损坏快照允许保留的最大过渡时长。
pub(crate) const MAX_SNAPSHOT_TRANSITION_GAME_SECONDS: f32 = 6.0 * GAME_SECONDS_PER_HOUR; // 正常过渡远短于该上限，钳制后仍可自行恢复。
