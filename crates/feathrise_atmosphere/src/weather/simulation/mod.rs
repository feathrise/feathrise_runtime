// 天气模拟功能域：组织状态API、季节抽取、连续输出和ECS系统。

mod debug; // 天气UI顺序、中文名称与F1强制预览接口。
mod output; // 五类天气目标参数与连续插值纯逻辑。
mod selection; // 状态图、季节权重、场景偏置、持续时间与确定性随机纯逻辑。
mod state; // WeatherState、WeatherDebugOverride与WeatherSnapshot接口实现。
mod surface; // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
mod systems; // 场景同步、消息处理、自动推进和输出解析ECS系统。

pub(crate) use systems::{
    // 插件层只注册功能域暴露的四个ECS系统。
    advance_weather_state,      // 按虚拟时间推进输入气候下的自动天气。
    handle_weather_commands,    // 消费剧情与F1天气控制消息。
    resolve_weather_output,     // 合成自动状态与调试预览连续输出。
    sync_weather_region,        // 按当前场景输入同步中性天气连续性。
}; // 完成天气ECS系统crate内重导出。
