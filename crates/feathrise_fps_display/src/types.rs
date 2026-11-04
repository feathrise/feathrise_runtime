//! 轻量FPS的呈现来源身份；不同SDK、交换链代次和尺寸不能共享采样区间。

#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 固定大小身份只做逐帧值比较。
pub(super) enum PresentationBackend {
    // 后端累计计数各自独立，数值相近也不得混用。
    #[cfg(any(feature = "dlss_frame_generation", test))] // 单独FSR构建不携带未使用的DLSS变体。
    Dlss, // NVIDIA Streamline报告的实际Present总数。
    #[cfg(any(feature = "fsr3_frame_generation", test))] // 单独DLSS构建不携带未使用的FSR变体。
    Fsr, // FidelityFX代理交换链的实际Present总数。
} // PresentationBackend结束。

#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 同尺寸交换链重建仍由epoch识别。
pub(super) struct PresentationIdentity {
    // 完整身份在每主世界帧先于计数差值校验。
    pub backend: PresentationBackend, // 避免DLSS/FSR切换时沿用另一后端计数。
    pub epoch: u64,                   // DLSS原生Present代次；FSR沿用已有尺寸与计数回退识别机制。
    pub extent: (u32, u32),           // 输出尺寸变化时重新建立半秒基线。
} // PresentationIdentity结束。

#[derive(Clone, Copy)] // POD采样输入无需分配或复制图像数据。
pub(super) struct PresentationCounter {
    // 输入只能来自有效运行状态快照。
    pub identity: PresentationIdentity, // 当前计数所属的唯一来源。
    pub count: u64,                     // 实际累计普通帧和生成帧，不是配置倍率推算值。
} // PresentationCounter结束。
