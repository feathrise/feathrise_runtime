// 天气表现域入口：汇总环境光照桥、低成本云幕、距离/体积雾和相机局部GPU降雨。

mod cloud; // 世界视线动态云层只对玩法或F12透视天气相机生效。
mod environment; // 环境桥只把连续天气参数转换成世界时间通用修正。
mod fog; // 距离雾在世界时间完成天空写入后读取最终清屏色。
mod rain; // 单发射体GPU雨围绕玩法主相机持续复用。
pub(crate) mod rain_occlusion; // 自动屋顶遮雨捕获独立于主相机深度。
mod volumetric_fog; // 有界体积雾与天体光束只接管玩法窗口主相机。

use bevy::prelude::*; // State资源用于在菜单和加载阶段停用全部玩法天气表现。
use feathrise_scene_core::state::ScenePhase; // 当前场景决定相机残留期间是否仍属于玩法画面。

pub(crate) use cloud::{install_dynamic_cloud_material, sync_weather_cloud_curtain}; // 构建时登记材质，世界光照后同步唯一动态云层。
pub(crate) use environment::apply_weather_environment; // 插件在天气输出解析后、世界光照前注册环境桥。
pub(crate) use fog::apply_weather_fog; // 插件在世界光照后注册距离雾同步。
pub(crate) use rain::sync_weather_rain_occlusion; // 雨向捕获需求完成后单独同步第二纹理槽。
pub(crate) use rain::sync_weather_rain_water; // 场景局部水面高度图完成后发布给同帧雨丝。
pub(crate) use rain::{
    seed_paused_weather_rain_preview, sync_weather_rain, sync_weather_rain_shelter,
}; // Update同步雨效，PostUpdate发布同帧遮雨并处理暂停预热。
pub(crate) use volumetric_fog::sync_weather_volumetric_fog; // PostUpdate按天气外观和玩家步进预算同步体积雾。

/// 判断当前状态是否允许显示玩法天气表现。
pub(crate) fn is_gameplay_scene(scene: Option<&State<ScenePhase>>) -> bool {
    scene.is_none_or(|scene| {
        matches!(
            scene.get(), // 读取当前而非下一场景，避免加载阶段提前显示降雨。
            ScenePhase::Active // 数据驱动地图按清单气候显示正式天气表现。
        ) // 结束玩法场景枚举匹配。
    }) // 没有状态机的最小ECS测试按玩法环境处理，保持系统可独立验证。
} // 结束玩法场景判断。

/// 判断相机局部天气是否适合当前视图；F12正交测绘视图只保留全局天空、光照和风。
pub(crate) fn supports_camera_local_weather(
    is_authoring_preview: bool,
    projection: Option<&Projection>,
) -> bool {
    !is_authoring_preview || !matches!(projection, Some(Projection::Orthographic(_))) // 只有显式F12正交相机关闭欧氏距离雾和局部云雨体积。
}

#[cfg(test)] // 场景边界测试防止主菜单意外继承玩法天气表现。
mod tests {
    use super::*; // 测试直接调用私有场景判断函数。

    #[test] // 菜单与加载阶段必须关闭视觉天气，全部正式玩法地图必须启用。
    fn gameplay_scene_filter_excludes_menu_and_loading() {
        assert!(!is_gameplay_scene(Some(&State::new(ScenePhase::Idle)))); // 主菜单不显示雾雨。
        assert!(!is_gameplay_scene(Some(&State::new(
            ScenePhase::Loading
        )))); // 加载画面不显示雾雨。
        assert!(is_gameplay_scene(Some(&State::new(
            ScenePhase::Active, // 数据驱动地图不得只有光照修正而缺少云、雨和雾。
        )))); // 完成动态地图天气表现断言。
        assert!(is_gameplay_scene(None)); // 无状态的最小测试环境默认允许独立运行。
    } // 结束玩法场景过滤测试。

    #[test] // 正交限制只属于F12测绘视图，不能意外关闭玩法相机或无投影最小测试的天气。
    fn only_authoring_orthographic_views_disable_camera_local_weather() {
        let orthographic = Projection::Orthographic(OrthographicProjection::default_3d());
        let perspective = Projection::Perspective(PerspectiveProjection::default());
        assert!(!supports_camera_local_weather(true, Some(&orthographic))); // 顶、前、侧视图不使用透视距离型效果。
        assert!(supports_camera_local_weather(true, Some(&perspective))); // F12透视保留完整正式天气。
        assert!(supports_camera_local_weather(true, None)); // 最小ECS测试缺少Projection时保持旧资格。
        assert!(supports_camera_local_weather(false, Some(&orthographic))); // 玩法相机投影策略不由作者视图规则改写。
    }
} // 结束天气表现入口测试模块。
