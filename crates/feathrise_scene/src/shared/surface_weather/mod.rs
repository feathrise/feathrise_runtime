//! 游戏天气、独立作者积水图与通用地表/河湖GPU数据的单一桥接。

mod resources; // 当前地图、文件候选与上一完整遮罩身份。
mod systems; // 地图纹理原子采用及固定天气数据发布。
pub(crate) use resources::{PuddleRuntimeState, PuddleFileCache};
pub(crate) use systems::resolve_puddle_mask;
#[cfg(test)] // 生命周期回归直接运行正式系统。
mod tests; // 换图、空图、坏热更和无重复上传。

use bevy::prelude::*; // 插件和Update/PostUpdate排序。
use feathrise_atmosphere::weather::WeatherSystemSet; // 同帧天气先解析再桥接。
use feathrise_environment::terrain_surface::SurfaceWeatherUpload; // 共享64字节上传只在桥接后发生。
use feathrise_map_content::{MapPuddleLiveOverride, MapPuddleSaveHandoff}; // 作者插件未启用时也提供安全空覆盖和交接票据。

#[derive(Default)] // 无配置插件由场景总入口注册一次。
pub(crate) struct SurfaceWeatherPlugin; // 场景拥有项目依赖，通用环境crate保持独立。

impl Plugin for SurfaceWeatherPlugin {
    // 安装所有地图共用的唯一桥。
    fn build(&self, app: &mut App) {
        // 所有状态按真实地图ID换代。
        app.init_resource::<resources::PuddleRuntimeState>() // 默认没有文件候选和旧遮罩。
            .init_resource::<PuddleFileCache>()
            .init_resource::<MapPuddleLiveOverride>() // 非test_panel构建安全保持空覆盖。
            .init_resource::<MapPuddleSaveHandoff>() // 保存后保留作者图直到正式纹理通过完整内容证明。
            .add_systems(
                Update,
                systems::resolve_puddle_mask.after(WeatherSystemSet::Resolve),
            ) // 资产和作者草稿在主线程原子采用。
            .add_systems(Update, systems::prune_puddle_file_cache)
            .add_systems(
                PostUpdate,
                systems::publish_surface_weather.before(SurfaceWeatherUpload),
            ); // 最新完整遮罩与天气先发布后上传。
    } // 完成插件装配。
} // 完成公共环境桥插件。
