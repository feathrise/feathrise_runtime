//! Windows Vulkan上的NVIDIA Streamline DLSS Frame Generation固定2x集成。
//!
//! `StreamlineBootstrapPlugin`必须在Bevy `DefaultPlugins`之前添加，确保ash首次加载Vulkan入口时使用
//! 已验签的Streamline代理；`FrameGenerationPlugin`在游戏组合层添加，负责设备能力、输入与设置生命周期。

#![cfg_attr(
    not(all(target_os = "windows", target_arch = "x86_64")),
    allow(dead_code)
)] // build.rs会对不支持目标给出明确错误。

mod bootstrap; // Vulkan创建前的SDK身份、验签与代理登记。
mod native; // 唯一C ABI边界。
mod render; // RenderApp帧输入与UI前捕获。
mod runtime; // 主世界请求、租约、Reflex与设备附着。
mod status; // 面向UI和诊断的公开结构化状态。
mod types; // 输入来源身份与SDK无关，不复制持久设置。

pub use bootstrap::StreamlineBootstrapPlugin; // 根程序必须在DefaultPlugins前安装。
pub use runtime::FrameGenerationPlugin; // GamePlugin在设置和光追生命周期就绪后安装。
pub use status::{
    // 设置页只读取这些公开事实，不接触原生SDK。
    FrameGenerationBlocker,
    FrameGenerationPhase,
    FrameGenerationRuntimeStatus,
};
