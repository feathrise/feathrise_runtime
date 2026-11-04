//! GPU叶片运行时插件及活动竞争布局到叶片密度纹理的桥接。

use bevy::prelude::{App, Plugin}; // 兼容插件只聚合专业叶片插件，不复制渲染实现。

mod density; // 活动布局只在原子换版时转换为叶片后端共享密度图。

pub use density::{
    VegetationBladeAdoptionPhase, VegetationBladeAdoptionStatus, VegetationBladeLayer,
    VegetationBladeValidationLayers,
}; // 消费者可绑定可见窗口、观察真实采用状态或只请求迁移期GPU校验。

/// 安装保持288字节八档窗口、16字节实例和四外观槽批次ABI的叶片后端。
#[derive(Clone, Copy, Debug, Default)]
pub struct VegetationBladePlugin; // 无限层由布局核心解析，外观超过四种时由项目拆分叶片批次。

impl Plugin for VegetationBladePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::VegetationPlugin>() {
            app.add_plugins(crate::VegetationPlugin); // 独立安装叶片后端时也保证活动布局和拒绝消息已注册。
        }
        app.add_plugins(crate::blade::GpuBladePlugin); // 安装已迁入的GPU生成、LOD和间接绘制管线。
        #[cfg(feature = "diagnostics")]
        app.add_plugins(crate::blade::GpuBladeDiagnosticsPlugin); // 可选异步GPU诊断保持独立feature。
        density::install(app); // 仅处理显式绑定窗口，旧直接密度图消费者行为不变。
    }
}
