//! 从 Bevy 实际 RenderDevice 冻结 Vulkan、Solari 与原型GPU诊断必需能力结论。

use bevy::{
    prelude::*, // App与Resource访问。
    render::{
        RenderApp,
        renderer::{RenderAdapterInfo, RenderDevice},
    }, // 实际适配器、后端与启用features。
};
use bevy_solari::SolariPlugins; // 使用第一方crate自身声明的required_wgpu_features，避免复制位集合漂移。

use crate::{
    resources::{RaytracingCapability, RaytracingPrototypeStatus},
    types::RaytracingPrototypeBlocker,
};

/// 返回本项目实时光追完整管线实际使用的设备能力集合。
fn required_raytracing_features() -> wgpu::Features {
    SolariPlugins::required_wgpu_features() // 先继承第一方TLAS、绑定数组和非一致索引能力。
        | wgpu::Features::TIMESTAMP_QUERY // 四段GPU时间仍是原子激活硬门。
        | wgpu::Features::PRIMITIVE_INDEX // 动态地形Deferred片元需要发布与唯一TLAS代理同序的三角编号。
} // 完成项目附加能力集合。

/// 在全部插件 build 完成且 RenderDevice 已建立后发布一次能力快照。
pub(crate) fn initialize_capability(app: &mut App) {
    let capability = if let Some(render_app) = app.get_sub_app(RenderApp) {
        let render_device = render_app.world().resource::<RenderDevice>(); // 读取Bevy真正启用的设备features。
        let adapter_info = render_app.world().resource::<RenderAdapterInfo>(); // 读取实际适配器与后端，不猜环境变量。
        let required = required_raytracing_features(); // 同时覆盖Solari、GPU计时与动态地形三角身份要求。
        let missing = required.difference(render_device.features()); // 仅报告设备创建后真实缺失位。
        let backend = format!("{:?}", adapter_info.backend); // F1显示Vulkan、Dx12等明确名称。
        let missing_capabilities = if missing.is_empty() {
            String::new() // 空字符串让F1明确显示“无缺失”。
        } else {
            format!("{missing:?}") // 保留wgpu官方feature名称便于驱动诊断。
        };
        let blocker = if adapter_info.backend != wgpu::Backend::Vulkan {
            Some(RaytracingPrototypeBlocker::BackendNotVulkan {
                backend: backend.clone(), // 不强制重启或让无Vulkan设备初始化崩溃。
            })
        } else if !missing.is_empty() {
            Some(RaytracingPrototypeBlocker::MissingDeviceFeatures {
                features: missing_capabilities.clone(), // 缺失能力完整显示在F1。
            })
        } else {
            None // Vulkan与全部Solari能力同时满足。
        };
        RaytracingCapability {
            available: blocker.is_none(), // 只有无阻断时生命周期才允许创建代理或相机租约。
            adapter_name: adapter_info.name.clone(), // 使用驱动报告的实际适配器名称。
            backend,                      // 保存实际后端。
            blocker,                      // 保存稳定不可用原因。
            missing_capabilities,         // 单独供F1“缺失能力”字段显示。
        }
    } else {
        RaytracingCapability {
            available: false, // 无RenderApp的测试或服务器构建只能保持完整光栅/无渲染状态。
            adapter_name: "无RenderApp".to_string(),
            backend: "无RenderApp".to_string(),
            blocker: Some(RaytracingPrototypeBlocker::MissingDeviceFeatures {
                features: "RenderApp未建立".to_string(),
            }),
            missing_capabilities: "RenderApp未建立".to_string(),
        }
    };

    if let Some(mut status) = app
        .world_mut()
        .get_resource_mut::<RaytracingPrototypeStatus>()
    {
        status.set_capability(
            capability.adapter_name.clone(),
            capability.backend.clone(),
            capability.missing_capabilities.clone(),
        ); // finish阶段把适配器身份同步到公开只读状态。
    }
    app.insert_resource(capability); // 覆盖build阶段默认不可用占位快照。
}

#[cfg(test)] // 设备能力集合不需要初始化真实GPU即可锁定项目附加位。
mod tests {
    use super::*; // 直接验证生产能力集合构造函数。

    #[test] // 缺少片元三角编号时必须阻断，而不是等Deferred管线运行时失败。
    fn dynamic_terrain_primitive_index_is_a_required_raytracing_feature() {
        let required = required_raytracing_features(); // 取得与finish阶段完全相同的位集合。
        assert!(required.contains(SolariPlugins::required_wgpu_features())); // 第一方Solari能力不得被项目附加位覆盖丢失。
        assert!(required.contains(wgpu::Features::TIMESTAMP_QUERY)); // GPU停止线证据继续存在。
        assert!(required.contains(wgpu::Features::PRIMITIVE_INDEX)); // Deferred三角身份能力必须显式存在。
    } // 完成设备能力门回归测试。
} // 完成能力模块纯CPU测试。
