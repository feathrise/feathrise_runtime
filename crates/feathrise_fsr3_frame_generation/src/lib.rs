//! Windows Vulkan上的AMD FidelityFX FSR 3.1 Frame Generation固定2x集成。
//!
//! `Fsr3FrameGenerationBootstrapPlugin`必须在Bevy `DefaultPlugins`之前添加，预留四条队列并登记
//! 进程期交换链provider；`Fsr3FrameGenerationPlugin`在游戏组合层添加，管理玩法输入和状态。

#![cfg_attr(
    not(all(target_os = "windows", target_arch = "x86_64")),
    allow(dead_code)
)] // build.rs会对不支持目标提供明确构建错误。

mod bootstrap; // Vulkan创建前的官方DLL身份和交换链所有权登记。
mod distribution; // 只读分发身份，不改变运行请求或原生 ABI 指纹。
mod native; // 唯一C ABI边界。
mod render; // RenderApp UI前捕获与同步Prepare。
mod runtime; // 主世界资格、租约、设备附着与状态汇总。
mod status; // 面向设置页和诊断的公开结构化状态。
#[cfg(test)]
mod prebuilt_tests; // 在不创建 GPU 的情况下验证真实桥接库链接、DLL 引导与配套身份。

use bevy::prelude::*; // 插件定义和生命周期委托。

pub use distribution::distribution_build_identity;

pub use status::{
    Fsr3FrameGenerationBlocker, Fsr3FrameGenerationInputFailure, Fsr3FrameGenerationPhase,
    Fsr3FrameGenerationRuntimeStatus,
}; // 业务/UI只读取公开事实，不接触原生SDK。

/// 必须在`DefaultPlugins`之前安装的FSR 3帧生成启动插件。
pub struct Fsr3FrameGenerationBootstrapPlugin; // 固定SDK 1.1.4与同步2x策略。

impl Plugin for Fsr3FrameGenerationBootstrapPlugin {
    fn build(&self, app: &mut App) {
        bootstrap::install(app); // DLL、队列和交换链回调必须先于Vulkan初始化。
    } // build结束。
} // Fsr3FrameGenerationBootstrapPlugin实现结束。

/// DefaultPlugins之后安装的玩法运行插件。
pub struct Fsr3FrameGenerationPlugin; // 当前设置枚举是唯一运行请求源。

impl Plugin for Fsr3FrameGenerationPlugin {
    fn build(&self, app: &mut App) {
        runtime::install(app); // 安装主世界租约和RenderApp输入系统。
    } // build结束。

    fn finish(&self, app: &mut App) {
        runtime::finish(app); // RenderDevice已建立、Surface尚未首次配置时附着四队列设备。
    } // finish结束。
} // Fsr3FrameGenerationPlugin实现结束。
