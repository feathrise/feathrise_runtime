//! 天气到植被的单一风参数桥：不依赖F1/F12是否安装，也不遍历草株或树实体。
//! 晴天保持原有风摆基线；天气风强只在固定成本的全局资源上转换。

mod systems; // 纯映射、时间相位连续性与同帧资源同步。
#[cfg(test)] // 不启动RenderApp即可验证天气、设置与风相位契约。
mod tests; // 覆盖晴天兼容、雨天增强、无风和跨时间回绕。

use bevy::prelude::*; // App、Plugin和Update调度入口。
use feathrise_atmosphere::weather::WeatherSystemSet; // 正式天气输出必须先完成解析。

#[derive(Default)] // 桥只持有固定大小的上一时间和相位偏移。
pub(crate) struct VegetationWeatherPlugin; // 场景层拥有天气与植被两侧依赖。

impl Plugin for VegetationWeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<systems::VegetationWindPhase>() // 无场景时也保留相位连续性。
            .add_systems(
                Update, // F12输入在天气解析之前，植被在渲染提取前读取最终值。
                systems::sync_vegetation_weather_wind.after(WeatherSystemSet::Resolve), // 单帧只写一份草与Mesh共用的全局风。
            ); // 避免场景切换或test_panel关闭时出现第二个写入者。
    } // 完成唯一桥接插件装配。
} // 完成天气植被插件实现。
