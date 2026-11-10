//! 活动竞争布局到GPU叶片密度纹理的事件式适配入口。

use feathrise_scene_core::residency::SceneResidencyAppExt;

use bevy::prelude::{App, IntoScheduleConfigs, PostUpdate, Update}; // 候选在Update运行，采用状态在延迟命令应用后的PostUpdate观察。

mod async_build; // 大场景候选的纯CPU阶段在AsyncCompute池构建并按身份回收。
mod atlas; // 基础级量化、完整Mip和脏块局部重建保持为无ECS纯数据模块。
mod components; // 公开层绑定与私有已采用版本独立于系统实现。
mod gpu_compose; // 正式源采样、竞争模式和提交块编码只在加载/换版时驱动基础级Compute。
mod gpu_mip; // 正式WGSL、RenderGraph调度和非阻塞回读共同组成候选GPU门。
mod resources; // 活动缓存、候选批次和跨世界消息集中定义。
mod status; // 可见窗口采用当前解析密度的只读阶段独立发布。
mod systems; // 主世界只在完整候选批次通过后原子更新叶片窗口。

pub use components::VegetationBladeLayer; // crate根继续只导出消费者需要的显式层绑定。
pub use resources::VegetationBladeValidationLayers; // 迁移场景可先运行不可见正式GPU parity而不切换旧窗口。
pub use status::{VegetationBladeAdoptionPhase, VegetationBladeAdoptionStatus}; // F3和严格烟雾读取真实窗口采用证据。

use async_build::VegetationBladeDensityCpuBuild; // 主世界持有最多一个未登记候选CPU任务。
use resources::{
    VegetationBladeDensityCache, VegetationGpuMipCoordinator, VegetationGpuMipInbox,
    VegetationGpuMipRequest, VegetationGpuMipRuntimeAvailable,
}; // 安装入口初始化无RenderApp逻辑测试也需要的稳定资源。
use status::sync_blade_adoption_status; // PostUpdate只观察真正写入的窗口版本身份。
use systems::sync_resolved_blade_density; // 同一系统根据RenderApp能力选择GPU候选门或CPU逻辑回退。

pub(super) fn install(app: &mut App) {
    app.init_resident_resource::<VegetationBladeDensityCache>()
        .init_resident_resource::<VegetationBladeDensityCpuBuild>()
        .init_resident_resource::<VegetationBladeValidationLayers>()
        .init_resident_resource::<VegetationGpuMipCoordinator>()
        .init_resident_resource::<VegetationGpuMipInbox>()
        .init_resident_resource::<VegetationGpuMipRequest>()
        .init_resident_resource::<VegetationBladeAdoptionStatus>()
        .add_resident_systems(
            Update,
            sync_resolved_blade_density.after(crate::layout::recompute_dirty_tiles),
        )
        .add_resident_systems(PostUpdate, sync_blade_adoption_status); // 延迟插入已生效后再发布真实采用分子与分母。
    let runtime_available = gpu_mip::install(app); // 有RenderApp时注册正式Compute与带版本回读闭环。
    app.insert_resource(VegetationGpuMipRuntimeAvailable(runtime_available)); // 状态机据此决定是否允许CPU逻辑回退。
}
