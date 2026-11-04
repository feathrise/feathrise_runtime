//! 自动屋顶遮雨：独立雨向纯深度捕获、原子GPU纹理和固定局部缓存。
mod casters; // 标准世界模型自动登记，保留原渲染层。
mod components; // 专用相机标记。
mod constants; // 固定纹理和米制范围。
mod helpers; // 纯雨向投影数学。
mod render; // GPU深度与矩阵一次发布。
mod resources; // 主世界身份与完整捕获需求。
mod systems; // 相机资格、脏状态与生命周期。
#[cfg(test)] // 无窗口验证真实生命周期和投影协议。
mod tests; // 覆盖切图、暂停与雨向几何。
use bevy::camera::{CameraUpdateSystems, visibility::VisibilitySystems}; // 在相机和视锥计算前更新需求。
use bevy::prelude::*; // 插件装配。
use bevy::render::{
    RenderApp, extract_component::ExtractComponentPlugin, extract_resource::ExtractResourcePlugin,
}; // GPU提取。
use bevy::transform::TransformSystems;
pub(crate) use constants::DEPTH as RAIN_OCCLUSION_DEPTH; // 消费端偏移不重复写魔数。
pub(crate) use resources::RainOcclusionMap; // Hanabi消费唯一纹理契约。 // 读取本帧源世界变换。
#[derive(SystemSet, Clone, Debug, Hash, PartialEq, Eq)] // 消费者在完整主世界需求后同步图像。
pub(crate) struct RainOcclusionUpdate; // 专用生产完成顺序。
pub(crate) fn install(app: &mut App) {
    // 由唯一天气插件安装。
    app.init_resource::<RainOcclusionMap>(); // 无GPU测试保持安全默认资源。
    if app.get_sub_app(RenderApp).is_none() {
        return;
    } // 不要求最小宿主有图像/渲染插件。
    app.init_resource::<resources::CaptureState>(); // 每个应用一张局部图。
    app.add_plugins((
        ExtractResourcePlugin::<RainOcclusionMap>::default(),
        ExtractComponentPlugin::<components::RainOcclusionCamera>::default(),
    )); // 提取小型需求。
    casters::install(app); // 在场景创建前登记观察器。
    render::install(app); // 内置Shader与独立深度转换。
    app.add_systems(Startup, systems::initialize); // 只创建禁用的纯深度相机。
    app.add_systems(
        PostUpdate,
        systems::update
            .after(TransformSystems::Propagate)
            .before(CameraUpdateSystems)
            .before(VisibilitySystems::UpdateFrusta)
            .in_set(RainOcclusionUpdate),
    ); // 同帧投影先于视锥和提取。
} // 装配结束。
