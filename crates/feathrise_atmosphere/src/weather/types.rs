// 天气普通类型：声明天气种类、区域、玩法修正、快照和公开调度阶段。

use bevy::prelude::*; // SystemSet 派生用于跨 crate 固定天气、存档和光照的执行顺序。
use serde::{Deserialize, Deserializer, Serialize}; // 天气枚举与快照需要进入角色存档，枚举恢复额外兼容未来未知名称。

/// 首版支持的五种基础天气。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)] // 值语义便于状态机、UI和存档直接复制，反序列化使用容错实现。
#[serde(rename_all = "snake_case")] // JSON 使用稳定的小写下划线名称，避免绑定 Rust 显示格式。
pub enum WeatherKind {
    /// 晴朗天气。
    #[default] // 旧存档和新游戏都从晴天开始。
    Clear, // 保留完整直射光、视距和远程战斗能力。
    /// 阴天天气。
    Overcast, // 作为晴天与降雨之间的必要过渡节点。
    /// 小雨天气。
    LightRain, // 提供轻度降雨、轻雾和适度玩法修正。
    /// 大雨天气。
    HeavyRain, // 提供最强降雨、风力和视距压缩。
    /// 清晨薄雾天气。
    Mist, // 只在凌晨至早晨生成，并在十点前散尽。
} // 结束天气种类声明。

impl<'de> Deserialize<'de> for WeatherKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>, // 接受角色存档所使用的任意Serde数据格式。
    {
        let value = String::deserialize(deserializer)?; // JSON天气枚举使用稳定字符串表示。
        Ok(match value.as_str() {
            "clear" => Self::Clear,          // 恢复晴朗天气。
            "overcast" => Self::Overcast,    // 恢复阴天天气。
            "light_rain" => Self::LightRain, // 恢复小雨天气。
            "heavy_rain" => Self::HeavyRain, // 恢复大雨天气。
            "mist" => Self::Mist,            // 恢复清晨薄雾天气。
            _ => Self::Clear,                // 未来版本、手改或损坏名称安全回退为晴天。
        }) // 返回兼容未知枚举名称的天气种类。
    } // 结束天气种类反序列化。
} // 结束天气种类容错Serde实现。

/// 天气连续性所属区域。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)] // 区域随天气快照保存并在场景切换时校正，恢复时兼容未知区域。
#[serde(rename_all = "snake_case")] // JSON 保持与天气枚举一致的稳定命名。
pub enum WeatherRegion {
    /// 不绑定场景地域的全局天气连续性。
    #[default] // 新会话和旧快照均使用中性全局区域。
    Global, // 场景切换保持同一天气过程，气候概率由输入配置决定。
} // 结束天气区域声明。

impl<'de> Deserialize<'de> for WeatherRegion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>, // 接受角色存档所使用的任意Serde数据格式。
    {
        let _ = String::deserialize(deserializer)?; // 保留字符串格式校验，旧地域名与未知名不再承载运行规则。
        Ok(Self::Global) // 所有历史和当前区域标签都安全迁移为中性全局区域。
    } // 结束天气区域反序列化。
} // 结束天气区域容错Serde实现。

/// 天气提供给战斗与AI系统的连续玩法修正。
#[derive(Clone, Copy, Debug, PartialEq)] // 连续输出每帧按值读取，无需额外ECS资源。
pub struct WeatherGameplayModifiers {
    /// AI视觉识别距离倍率。
    pub ai_visibility_factor: f32, // 晴天为一，大雨和薄雾会明显降低。
    /// 弓弩与火铳有效瞄准距离倍率。
    pub ranged_aim_factor: f32, // 只修正有效瞄准距离，不修改投射物伤害。
    /// 火铳举铳与装填速度倍率。
    pub firearm_speed_factor: f32, // 只有降雨降低该值，避免额外引入随机哑火。
} // 结束天气玩法修正声明。

/// 可序列化天气快照；缺失字段由默认晴天补齐，恢复时再规范化损坏数值。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)] // 快速存档和多槽位存档可以直接复制该轻量结构。
#[serde(default)] // 旧存档没有天气字段或新字段时使用安全默认值。
pub struct WeatherSnapshot {
    pub surface_wetness: f32,     // 地表湿润度，范围零至一，保留自然降雨历史。
    pub puddle_amount: f32,       // 积水量，范围零至一，独立缓慢形成与消退。
    pub surface_effect_time: f32, // 暂停感知视觉秒，以4096秒周期限制长期浮点精度损失。

    /// 当前天气节点。
    pub current: WeatherKind, // 正在稳定显示或作为过渡起点的天气。
    /// 当前过渡目标。
    pub target: WeatherKind, // 非相邻损坏目标会在恢复时改为合法的下一节点。
    /// 跨越多个相邻节点时最终希望到达的天气。
    pub queued_target: Option<WeatherKind>, // 晴转大雨等剧情请求完成中间节点后继续执行，普通自动天气为None。
    /// 后续相邻节点沿用的单段过渡总游戏秒数。
    pub queued_transition_duration_game_seconds: f32, // 没有排队目标时为零，损坏值恢复为默认过渡时长。
    /// 剧情目标到达后至少保持该天气的游戏秒数。
    pub pending_minimum_hold_game_seconds: f32, // 在途剧情命令随存档保留，到达最终目标后并入稳定剩余时间并清零。
    /// 当前过渡完成比例。
    pub transition_progress: f32, // 合法范围零至一，非有限值会回退。
    /// 当前过渡总游戏秒数。
    pub transition_duration_game_seconds: f32, // 稳定期为零，过渡期保存完整时长。
    /// 当前稳定天气剩余游戏秒数。
    pub stable_remaining_game_seconds: f32, // 过渡完成后重新抽取对应天气持续时长。
    /// 快照所属天气区。
    pub region: WeatherRegion, // 保留快照字段兼容性；当前只有不绑定地域的全局连续性。
    /// 确定性随机序列基础种子。
    pub seed: u64, // 读档后保持后续天气抽取顺序一致。
    /// 已消费的随机序号。
    pub sequence: u64, // 每次随机取值递增，避免保存复杂RNG内部布局。
} // 结束天气存档快照声明。

/// 天气系统公开调度阶段。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)] // 外部存档、光照和渲染系统通过稳定标签排序。
pub enum WeatherSystemSet {
    /// 消费命令并推进自动天气状态。
    Advance, // 固定排在世界时间Advance之后，以读取本帧最终月份和时刻。
    /// 存读档原子写入天气状态的预留阶段。
    SnapshotIo, // 快速存档和菜单多槽位恢复应放入该阶段。
    /// 把状态与调试覆盖解析为连续天气输出。
    Resolve, // 固定排在SnapshotIo之后和世界光照ApplyLighting之前。
    /// 将连续输出应用到雾、云、粒子和风的扩展阶段。
    ApplyEffects, // 当前注册玩法视觉效果，独立天气音频资源补齐后也应接入此阶段。
} // 结束天气公开调度阶段声明。
