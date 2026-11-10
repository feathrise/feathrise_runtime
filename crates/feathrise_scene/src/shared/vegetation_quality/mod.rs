//! GPU 叶片植被质量桥接插件。
//! 【功能设计】设置模型保持渲染后端无关，场景聚合层只把三个纯倍率写入玩法植被视图。

use feathrise_scene_core::residency::SceneResidencyAppExt;

mod systems; // 系统负责幂等应用当前持久植被档，并让后续场景相机自动继承。

use bevy::prelude::*; // App、Plugin 与 PostUpdate 调度接口。
use feathrise_settings::RenderSettings; // 独立使用场景插件时初始化安全高档设置资源。

use systems::apply_vegetation_quality; // 注册唯一轻量视图同步系统。

/// 把持久植被画质应用到所有当前及后续玩法 `GpuBladeView`。
pub(crate) struct VegetationQualityPlugin; // 插件不持有配置，权威值始终来自统一 RenderSettings。

impl Plugin for VegetationQualityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderSettings>() // 场景 crate 独立测试时仍具备高档兼容默认值。
            .add_resident_systems(PostUpdate, apply_vegetation_quality); // UI 在 Update 改值后同帧写入玩法视图。
    } // 完成只增加一个有界视图比较的质量桥接。
} // 完成植被质量插件实现。
