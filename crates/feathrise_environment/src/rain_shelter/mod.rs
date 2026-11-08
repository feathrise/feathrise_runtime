//! 显式轻量遮雨代理：CPU/GPU共享同一OBB与固定上传，不读取场景网格。

mod components; // 作者局部代理组件。
mod helpers; // CPU同源射线判定。
mod resources; // 完整活动批次与上传顺序。
mod systems; // 固定缓冲创建与变更上传。
#[cfg(test)] // 无窗口回归覆盖真实上传与空间边界。
mod tests;
mod types; // 固定GPU体布局。 // 验证共享契约。

pub use components::RainShelterBox; // 无碰撞节点也能显式登记遮雨体。
pub use resources::{RainShelterGpu, RainShelterState, RainShelterUpload}; // CPU消费者与宿主发布边界。
pub(crate) use systems::install_rain_shelter;
pub use systems::rain_shelter_buffer_handle; // 所有材质使用唯一资产身份。
pub use types::RainShelterVolume; // 材质与作者桥共用相同OBB。 // 由地表插件安装一次。
pub const RAIN_SHELTER_RAY_WGSL: &str =
    include_str!("../terrain_surface/shaders/rain_shelter.wesl"); // 材质和Hanabi共用实际生产射线函数。
