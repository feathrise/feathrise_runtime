//! DLSS Frame Generation面向设置页、诊断和烟雾测试的结构化运行状态。

use bevy::prelude::*; // 公开状态作为主世界资源由UI只读消费。

/// 当前帧生成生命周期阶段；DLL存在从不等价于能力或插帧生效。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameGenerationPhase {
    #[default]
    Blocked, // 构建身份、加载器、设备能力或输入契约未满足。
    Ready,     // Streamline和设备已附着，但玩家未开启或当前没有玩法帧。
    Requested, // 插件接受2x请求，等待Present返回真实生成帧证据。
    Active2x,  // 最近宿主Present实际输出至少两帧且状态无错误。
} // FrameGenerationPhase结束。

/// 玩家开启请求无法成为真实2x时的单一主因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameGenerationBlocker {
    InitializationPending, // Streamline前置初始化尚未完成，不把可选数字ID缺失误报为阻断。
    BootstrapFailed { result: i32 }, // 签名、插件、身份或slInit失败。
    VulkanLoaderRegistrationFailed, // Vulkan入口已被其他代码初始化或登记。
    QueueHookRegistrationFailed, // Present/提交边界已被其他中间件占用。
    RenderAdapterUnavailable, // Bevy未建立Vulkan RenderAdapter。
    NonVulkanAdapter,      // 构建或运行时后端不是本轮支持的Vulkan。
    DeviceUnsupported { result: i32 }, // GPU、驱动、Windows版本或HAGS不满足官方要求。
    RequiresVsyncOff,      // Streamline 2.12的Vulkan帧生成不支持垂直同步。
    NoGameplayCamera,      // 菜单、加载或场景切换期没有唯一玩法窗口相机。
    NoPrimaryWindow,       // 没有唯一主窗口时无法确定Present交换链。
    WindowTransition,      // 尺寸或呈现模式刚变化，等待交换链完成重建。
    InputsUnavailable,     // 当前帧缺深度、运动矢量、无HUD颜色或有效命令缓冲。
    ResolutionTooLow { minimum: u32 }, // 输出任一边低于插件报告的下限。
    ReflexNotDetected,     // Present线程未观察到同帧Reflex标记。
    CommonConstantsInvalid, // 相机矩阵、深度方向或运动矢量常量无效。
    RuntimeFailure { result: i32, status_bits: u32 }, // 其他Streamline调用或DLSSG状态失败。
} // FrameGenerationBlocker结束。

impl FrameGenerationBlocker {
    /// 返回设置页可以直接显示的稳定中文说明。
    pub fn message(&self) -> String {
        match self {
            Self::InitializationPending => "Streamline 正在等待初始化".into(),
            Self::BootstrapFailed { result } => {
                format!("Streamline 初始化失败（{}）", result_name(*result))
            }
            Self::VulkanLoaderRegistrationFailed => "Vulkan 已先于帧生成代理初始化".into(),
            Self::QueueHookRegistrationFailed => "Vulkan 提交/呈现边界已被其他中间件占用".into(),
            Self::RenderAdapterUnavailable => "Bevy 尚未建立渲染适配器".into(),
            Self::NonVulkanAdapter => "帧生成首期仅支持 Windows Vulkan".into(),
            Self::DeviceUnsupported { result } => {
                format!("当前设备不支持 DLSS 帧生成（{}）", result_name(*result))
            }
            Self::RequiresVsyncOff => "Vulkan 帧生成需要关闭垂直同步".into(),
            Self::NoGameplayCamera => "当前没有可插帧的主场景相机".into(),
            Self::NoPrimaryWindow => "当前没有唯一的主窗口".into(),
            Self::WindowTransition => "窗口尺寸或呈现模式正在切换".into(),
            Self::InputsUnavailable => "当前帧缺少深度、运动矢量、无 HUD 颜色或独立 UI 输入".into(),
            Self::ResolutionTooLow { minimum } => {
                format!("输出分辨率过低，最短边至少需要 {minimum} 像素")
            }
            Self::ReflexNotDetected => "Reflex 未在当前 Present 时序中生效".into(),
            Self::CommonConstantsInvalid => "帧生成相机公共常量无效".into(),
            Self::RuntimeFailure {
                result,
                status_bits,
            } => format!(
                "帧生成运行失败（{}，状态 0x{status_bits:08X}）",
                result_name(*result)
            ),
        }
    } // message结束。
} // FrameGenerationBlocker实现结束。

/// 主世界每帧可读的真实帧生成状态。
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct FrameGenerationRuntimeStatus {
    pub phase: FrameGenerationPhase,             // 当前生命周期阶段。
    pub requested: bool,                         // 玩家持久设置是否请求开启。
    pub initialized: bool,                       // `slInit`是否完成。
    pub device_supported: bool,                  // 物理设备和Reflex/DLSS-G能力是否通过。
    pub inputs_ready: bool,                      // 最近渲染帧是否完整提交四项必需输入与公共常量。
    pub frames_presented: u32,                   // 最近一次宿主Present实际输出总帧数。
    pub total_present_count: u64,                // SDK确认的累计呈现帧数，供HUD按真实时间采样。
    pub presentation_epoch: u64,                 // 启停、窗口规格变化或失败后重新采样。
    pub presentation_ready: bool,                // 可信计数快照就绪，不等同于2x资格。
    pub maximum_generated_frames: u32,           // 硬件报告的最大插入帧数，本轮只消费至少一帧。
    pub minimum_dimension: u32,                  // 插件报告的最小输出边长。
    pub estimated_vram_bytes: u64,               // 显式估算值；未请求时为零。
    pub streamline_result: i32,                  // 最近一次官方`sl::Result`数值。
    pub dlssg_status_bits: u32,                  // 最近一次官方`DLSSGStatus`位集合。
    pub blocker: Option<FrameGenerationBlocker>, // 当前单一主阻断；Active2x时为空。
    pub detail: String, // 启动/SDK返回的附加诊断文本，不在Present回调格式化。
} // FrameGenerationRuntimeStatus结束。

impl Default for FrameGenerationRuntimeStatus {
    fn default() -> Self {
        Self {
            phase: FrameGenerationPhase::Blocked, // 未完成启动时不宣称就绪。
            requested: false,                     // 旧配置与缺省设置关闭。
            initialized: false,                   // 尚未调用slInit。
            device_supported: false,              // 尚未检查物理设备。
            inputs_ready: false,                  // 尚未提交任何玩法输入。
            frames_presented: 1,                  // 宿主原始帧是安全基线。
            total_present_count: 0,               // 尚未确认任何呈现。
            presentation_epoch: 0,                // 初始采样身份。
            presentation_ready: false,            // 不能把初始化当作已取得FPS样本。
            maximum_generated_frames: 0,          // 未查询能力。
            minimum_dimension: 0,                 // 未查询限制。
            estimated_vram_bytes: 0,              // 稳定帧不执行昂贵估算。
            streamline_result: 0,                 // 无调用时保留成功枚举值但由阶段表达未初始化。
            dlssg_status_bits: 0,                 // 尚无运行状态位。
            blocker: Some(FrameGenerationBlocker::InitializationPending), // 构建插件必须显式覆盖。
            detail: String::new(),                // 无附加文本。
        }
    } // 默认状态结束。
} // FrameGenerationRuntimeStatus默认实现结束。

impl FrameGenerationRuntimeStatus {
    /// 只有Present的真实帧数证据成立时才返回2x有效。
    pub fn is_effective_2x(&self) -> bool {
        self.phase == FrameGenerationPhase::Active2x
            && self.frames_presented >= 2
            && self.dlssg_status_bits == 0
            && self.blocker.is_none() // 四项事实必须同时成立。
    } // is_effective_2x结束。
} // FrameGenerationRuntimeStatus实现结束。

/// 把官方结果数值翻译成稳定短语；未知新枚举仍保留原数值。
pub(crate) fn result_name(result: i32) -> String {
    let name = match result {
        0 => "成功",
        1 => "I/O 错误",
        2 => "驱动版本过旧",
        3 => "Windows 版本过旧",
        4 => "硬件加速 GPU 调度未开启",
        5 => "Vulkan 设备未创建",
        6 => "未找到支持的适配器",
        7 => "适配器不支持",
        8 => "缺少插件",
        9 => "Vulkan API 错误",
        10 => "DXGI API 错误",
        11 => "D3D API 错误",
        12 => "NRD API 错误",
        13 => "NVAPI 错误",
        14 => "Reflex API 错误",
        15 => "NGX 初始化失败",
        16 => "JSON 解析失败",
        17 => "缺少代理",
        18 => "缺少资源状态",
        19 => "集成无效",
        20 => "缺少输入参数",
        21 => "Streamline 未初始化",
        22 => "计算失败",
        23 => "未调用初始化",
        24 => "异常处理器错误",
        25 => "参数无效",
        26 => "缺少公共常量",
        27 => "公共常量重复",
        28 => "API 缺失或版本不匹配",
        29 => "公共常量缺失",
        30 => "接口版本不支持",
        31 => "功能缺失",
        32 => "功能不支持",
        33 => "缺少必要 Hook",
        34 => "插件加载失败",
        35 => "插件优先级错误",
        36 => "缺少功能依赖",
        37 => "功能管理器状态无效",
        38 => "状态无效",
        39 => "显存不足警告",
        _ => return format!("Streamline 结果 {result}"),
    }; // 已知结果映射结束。
    name.into() // UI取得拥有的短字符串。
} // result_name结束。

#[cfg(test)] // 纯状态测试不需要GPU或NVIDIA SDK运行。
mod tests {
    use super::*; // 验证公开有效性契约。

    #[test]
    fn loaded_dll_never_counts_as_effective_without_present_evidence() {
        let mut status = FrameGenerationRuntimeStatus {
            phase: FrameGenerationPhase::Requested,
            requested: true,
            initialized: true,
            device_supported: true,
            inputs_ready: true,
            frames_presented: 1,
            blocker: None,
            ..default()
        }; // 模拟所有初始化均成功但还未产生插值帧。
        assert!(!status.is_effective_2x()); // DLL加载与请求接受不能冒充生效。
        status.phase = FrameGenerationPhase::Active2x; // Present回调发布真实活动阶段。
        status.frames_presented = 2; // 至少一帧中间帧。
        assert!(status.is_effective_2x()); // 完整证据才算2x。
    } // 测试结束。

    #[test]
    fn official_result_names_preserve_known_and_future_values() {
        assert_eq!(result_name(4), "硬件加速 GPU 调度未开启");
        assert_eq!(result_name(39), "显存不足警告");
        assert_eq!(result_name(400), "Streamline 结果 400");
    }
} // 状态测试模块结束。
