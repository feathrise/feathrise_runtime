//! 树木、灌木与岩石的场景细节质量桥接插件。
//! 【功能设计】保存每个视觉节点的作者范围，切档只派生 Bevy `VisibilityRange`，不改碰撞、实例数或资产生命周期。

use feathrise_scene_core::residency::SceneResidencyAppExt;

mod helpers; // 纯函数负责从作者范围派生各档 LOD 与最远显示边界。
mod systems; // 观察器接管新节点，变化驱动系统只在切档时扫描已备份节点。

use bevy::prelude::*; // App、Plugin 与 PostUpdate 调度接口。
use feathrise_environment::vegetation::VegetationMeshBatchQuality;
use feathrise_settings::RenderSettings; // 独立安装场景插件时仍初始化安全高档默认值。 // 分块实例后端复用同一场景细节档位。

use crate::resources::SceneDetailQualityState; // 保存上次已应用档位以消除稳定帧扫描。
use systems::{
    apply_scene_detail_quality, capture_scene_detail_visibility_range,
    register_scene_detail_visibility_range, unregister_scene_detail_visibility_range,
}; // 注册节点增删与有界切档同步路径。

/// 把持久场景细节档应用到全部树木、灌木与岩石视觉 LOD 节点。
pub(crate) struct SceneDetailQualityPlugin; // 权威值始终来自统一 RenderSettings，插件自身不复制配置。

impl Plugin for SceneDetailQualityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderSettings>() // 场景 crate 独立测试时保持作者高档基线。
            .init_resource::<VegetationMeshBatchQuality>() // 无渲染逻辑测试也能验证批次倍率桥接。
            .init_resident_resource::<SceneDetailQualityState>() // 首帧执行一次兜底同步，稳定帧随后 O(1) 返回。
            .add_observer(capture_scene_detail_visibility_range) // 任意后续场景或热重载节点在生成当下继承当前档。
            .add_observer(register_scene_detail_visibility_range) // 作者备份真实落地后才登记实体，避免瞬时节点留下失效索引。
            .add_observer(unregister_scene_detail_visibility_range) // 切图或热替换时 O(1) 移除已销毁节点索引。
            .add_resident_systems(
                PostUpdate,
                apply_scene_detail_quality.before(TransformSystems::Propagate),
            ); // UI在Update改值后先更新批次倍率，随后同帧选择最终逐实例LOD。
    } // 完成观察新增节点与变化驱动切档的组合。
} // 完成场景细节质量插件实现。
