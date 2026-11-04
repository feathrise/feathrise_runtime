//! 雨丝水面裁切生产端：局部512²顶视真实水面高度，无CPU网格复制或读回。
use bevy::camera::visibility::VisibilitySystems; // 新代理在本帧可见性传播之前安装。
use bevy::pbr::MaterialPlugin; // 独立无光照高度材质。
use bevy::prelude::*; // 插件资源与主世界调度。
use bevy::transform::TransformSystems; // 代理与相机必须使用同帧世界变换。
use feathrise_environment::rain_water::{RainWaterMask, RainWaterMaskUpdate}; // scene与atmosphere共享单一图像和范围。

mod camera; // 唯一局部顶视相机与天气资格。
mod components; // 代理和捕获相机标记。
mod material; // 复用真实波形的无光照高度编码材质。
mod resources; // 按源实体及源材质缓存代理。
mod systems; // 变化收集、有限代理安装和清理。
#[cfg(test)] // 无窗口测试真实代理生命周期。
mod tests; // 缓存与水材质变更回归。
pub(super) use material::RainHeightMaterial; // 既有水参数遍历顺手同步同一固定uniform。
pub(super) use resources::RainMaskCache; // 主更新系统只对已登记材质做O(1)查询。
pub(super) use systems::sync_source_uniform; // 不额外遍历所有水材质更新玩家波。

pub(super) fn install(app: &mut App) {
    // 由唯一水插件装配一次。
    material::register_shader(app); // 固定片元shader复用已有真实水顶点入口。
    app.init_resource::<RainWaterMask>()
        .init_resource::<RainMaskCache>() // 空图默认不参与雨丝裁切。
        .add_plugins(MaterialPlugin::<RainHeightMaterial>::default()) // 捕获材质不参加阴影与预通道。
        .add_systems(Startup, camera::initialize) // 一次创建固定GPU图像和禁用相机。
        .add_systems(
            PostUpdate,
            (
                systems::sync_proxies,
                systems::sync_clipmap_proxies,
                camera::update,
            )
                .chain()
                .after(super::systems::update_dynamic_water_surfaces)
                .after(TransformSystems::Propagate)
                .before(bevy::camera::CameraUpdateSystems)
                .before(VisibilitySystems::UpdateFrusta)
                .before(VisibilitySystems::VisibilityPropagate)
                .before(VisibilitySystems::CheckVisibility)
                .in_set(RainWaterMaskUpdate),
        ); // 发布范围与世界变换后才更新相机Frustum，避免新bounds配旧裁剪。
} // 完成高度图生产端安装。
