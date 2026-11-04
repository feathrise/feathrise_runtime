//! 帧生成输入来源的稳定身份；不包含逐帧抖动或遥测，避免持续清空插值历史。

use feathrise_settings::{AntiAliasingMode, DlssMode, DlssRuntimePath, FsrMode}; // 复用设置域类型。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] // 值快照随请求提取，不建立新的设置源。
pub(crate) struct InputIdentity {
    pub(crate) dlss: DlssMode, // 请求切换时提前使同尺寸旧历史失效。
    pub(crate) fsr: FsrMode,   // FSR与DLSS质量档可能具有相同内部尺寸。
    pub(crate) antialiasing: AntiAliasingMode, // 原生抗锯齿选择变化也是输入换代。
    pub(crate) raytracing: bool, // 光栅与光追之间不能复用旧颜色历史。
    pub(crate) reconstruction: bool, // SR/RR切换即使尺寸一致也重新预热。
    pub(crate) effective_path: DlssRuntimePath, // 实际回退或延迟生效同样触发换代。
    pub(crate) effective_fsr: FsrMode, // FSR实际失败回退不能仅从请求判断。
}
