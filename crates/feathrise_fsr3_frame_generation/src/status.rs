//! FSR 3.1 Frame Generation面向设置页、诊断和真机验收的结构化运行状态。

use bevy::prelude::*; // UI只读取主世界资源，不接触原生SDK。

/// 当前FSR帧生成生命周期；DLL存在从不等价于代理交换链或2x生效。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fsr3FrameGenerationPhase {
    #[default]
    Blocked, // 构建、加载、队列、代理或输入契约未满足。
    Ready,     // 运行库和设备已附着，当前保持普通Present。
    Requested, // 完整Prepare已提交，等待真实Present计数证明2x。
    Active2x,  // 最近宿主Present实际产生至少两帧。
} // Fsr3FrameGenerationPhase结束。

/// RenderApp未能提交完整Prepare时的精确输入阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum Fsr3FrameGenerationInputFailure {
    #[default]
    Pending = 0, // 刚开启、换规格或渲染反馈暂未到达。
    ViewNotPresenting = 1,    // RenderApp未为玩法主相机建立最终输出纹理。
    CameraParameters = 2,     // 近裁面、FOV或透视投影无效。
    PrepassTextures = 3,      // Bevy尚未为该视图创建预通道附件集。
    DepthTexture = 4,         // 当前视图缺少深度预通道。
    MotionVectorTexture = 5,  // 当前视图缺少运动矢量预通道。
    OutputTexture = 6,        // 主输出不是有效的单层二维纹理。
    DepthInterop = 7,         // 深度纹理无法映射到当前Vulkan设备。
    MotionVectorInterop = 8,  // 运动矢量无法映射到当前Vulkan设备。
    RenderExtent = 9,         // 有效内部分辨率超出某一输入附件。
    CommandBuffer = 10,       // wgpu未暴露当前Vulkan命令缓冲。
    NonFiniteParameters = 11, // 相机、抖动或帧时间含非有限值。
    PrepareCall = 12,         // FidelityFX Prepare调用本身失败。
    WorldUiLayer = 13,        // 当前帧独立世界UI绘制失败并已回到普通场景，不能继续插值该场景。
} // Fsr3FrameGenerationInputFailure结束。

impl Fsr3FrameGenerationInputFailure {
    pub(crate) fn from_code(code: u32) -> Self {
        match code {
            1 => Self::ViewNotPresenting,
            2 => Self::CameraParameters,
            3 => Self::PrepassTextures,
            4 => Self::DepthTexture,
            5 => Self::MotionVectorTexture,
            6 => Self::OutputTexture,
            7 => Self::DepthInterop,
            8 => Self::MotionVectorInterop,
            9 => Self::RenderExtent,
            10 => Self::CommandBuffer,
            11 => Self::NonFiniteParameters,
            12 => Self::PrepareCall,
            13 => Self::WorldUiLayer, // 跨世界保留世界标签层的明确失败原因。
            _ => Self::Pending,       // 未知或跨版本代码统一按预热处理。
        }
    } // from_code结束。

    pub(crate) fn code(self) -> u32 {
        self as u32 // repr(u32)为两个世界的原子反馈提供稳定编码。
    } // code结束。

    /// 返回设置页可直接显示的精确缺项。
    pub fn message(self) -> &'static str {
        match self {
            Self::Pending => "正在等待主相机的首个完整渲染帧",
            Self::ViewNotPresenting => "RenderApp尚未为主场景相机建立最终输出纹理",
            Self::CameraParameters => "主场景相机不是有效的透视投影",
            Self::PrepassTextures => "Bevy尚未创建帧生成所需的预通道附件",
            Self::DepthTexture => "主场景相机缺少深度预通道",
            Self::MotionVectorTexture => "主场景相机缺少运动矢量预通道",
            Self::OutputTexture => "主交换链输出纹理尺寸或层数无效",
            Self::DepthInterop => "无法取得深度纹理的Vulkan句柄",
            Self::MotionVectorInterop => "无法取得运动矢量的Vulkan句柄",
            Self::RenderExtent => "内部渲染分辨率与深度、运动矢量或输出范围不匹配",
            Self::CommandBuffer => "无法取得当前Vulkan命令缓冲",
            Self::NonFiniteParameters => "相机、抖动或帧时间参数无效",
            Self::PrepareCall => "FidelityFX帧生成Prepare调用失败",
            Self::WorldUiLayer => "当前帧独立世界UI层绘制失败，已恢复普通标签显示", // 普通路径仍显示完整标签，只有帧生成本帧被阻断。
        }
    } // message结束。
} // Fsr3FrameGenerationInputFailure实现结束。

/// 玩家请求无法成为真实2x时的单一主因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fsr3FrameGenerationBlocker {
    BootstrapFailed {
        result: i32,
    }, // 修订身份、DLL、导出或Vulkan loader解析失败。
    QueueReservationFailed,      // 额外队列请求已被其他启动中间件占用。
    QueueHookRegistrationFailed, // Present边界已有另一帧生成后端所有者。
    SwapchainProviderRegistrationFailed, // 交换链provider已有其他所有者。
    RenderDeviceUnavailable,     // Bevy尚未建立wgpu设备。
    NonVulkanDevice,             // 当前后端不是Windows Vulkan。
    QueueUnavailable {
        available: u32,
        required: u32,
    }, // 设备无法分配四个独立角色队列。
    DeviceAttachFailed {
        result: i32,
    }, // 原生设备身份或队列附着失败。
    NoGameplayCamera,            // 当前没有唯一玩法主相机。
    GameplayCameraNotPresenting, // 玩法相机未激活或不再输出到主窗口。
    NoPrimaryWindow,             // 当前没有唯一主窗口。
    WindowTransition,            // 尺寸或Present模式刚发生交换链重建。
    SwapchainUnavailable,        // FFX代理尚未拥有当前交换链。
    InputsUnavailable {
        reason: Fsr3FrameGenerationInputFailure, // 将RenderApp精确缺项带回设置页。
    },
    RuntimeFailure {
        result: i32,
    }, // FFX创建、配置、Prepare或Present失败。
} // Fsr3FrameGenerationBlocker结束。

impl Fsr3FrameGenerationBlocker {
    /// 返回设置页可直接显示的稳定中文原因。
    pub fn message(&self) -> String {
        match self {
            Self::BootstrapFailed { result } => {
                format!("FidelityFX 运行库初始化失败（{}）", result_name(*result))
            }
            Self::QueueReservationFailed => "Vulkan 额外队列请求已被其他呈现中间件占用".into(),
            Self::QueueHookRegistrationFailed => "Vulkan Present 边界已有其他帧生成后端".into(),
            Self::SwapchainProviderRegistrationFailed => {
                "Vulkan 交换链代理已有其他帧生成后端".into()
            }
            Self::RenderDeviceUnavailable => "Bevy 尚未建立渲染设备".into(),
            Self::NonVulkanDevice => "FSR 3 帧生成首期仅支持 Windows Vulkan".into(),
            Self::QueueUnavailable {
                available,
                required,
            } => format!(
                "当前 Vulkan 设备只能分配 {available} 条角色队列，FSR 3 需要 {required} 条独立队列"
            ),
            Self::DeviceAttachFailed { result } => {
                format!("FSR 3 Vulkan 设备附着失败（{}）", result_name(*result))
            }
            Self::NoGameplayCamera => "当前没有可插帧的唯一主场景相机".into(),
            Self::GameplayCameraNotPresenting => "主场景相机当前未激活或未输出到主窗口".into(),
            Self::NoPrimaryWindow => "当前没有唯一主窗口".into(),
            Self::WindowTransition => "窗口尺寸或呈现模式正在切换".into(),
            Self::SwapchainUnavailable => "FSR 3 代理交换链尚未就绪".into(),
            Self::InputsUnavailable { reason } => reason.message().into(),
            Self::RuntimeFailure { result } => {
                format!("FSR 3 帧生成运行失败（{}）", result_name(*result))
            }
        }
    } // message结束。
} // Fsr3FrameGenerationBlocker实现结束。

/// 主世界每帧可读的真实FSR帧生成状态。
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct Fsr3FrameGenerationRuntimeStatus {
    pub phase: Fsr3FrameGenerationPhase, // 当前生命周期阶段。
    pub requested: bool,                 // 当前进程启动选择是否为FSR 3固定2x。
    pub runtime_loaded: bool,            // 源码修订DLL的固定身份、加载和导出是否完整。
    pub device_attached: bool,           // 四条独立队列设备是否附着。
    pub swapchain_ready: bool,           // FFX代理是否拥有当前交换链。
    pub context_ready: bool,             // FG核心上下文是否就绪。
    pub inputs_ready: bool,              // 最近RenderApp帧是否成功Prepare。
    pub frames_presented: u32, // 最近完整120宿主帧窗口确认的近似倍率；不是单次异步Present的帧数。
    pub total_present_count: u64, // 当前代理交换链累计Present数。
    pub available_queues: u32, // 实际取得的game/compute/present/acquire角色数量。
    pub display_width: u32,    // 当前代理交换链宽度。
    pub display_height: u32,   // 当前代理交换链高度。
    pub fidelityfx_result: i32, // 最近FFX/Vulkan结果。
    pub blocker: Option<Fsr3FrameGenerationBlocker>, // 当前单一主阻断。
    pub detail: String,        // 原生阶段诊断，不在Present回调分配。
} // Fsr3FrameGenerationRuntimeStatus结束。

impl Default for Fsr3FrameGenerationRuntimeStatus {
    fn default() -> Self {
        Self {
            phase: Fsr3FrameGenerationPhase::Blocked, // 启动前不宣称可用。
            requested: false,                         // 默认不接管交换链。
            runtime_loaded: false,                    // 尚未验证DLL。
            device_attached: false,                   // 尚未取得队列。
            swapchain_ready: false,                   // 尚无窗口代理。
            context_ready: false,                     // 尚无核心上下文。
            inputs_ready: false,                      // 尚无有效Prepare。
            frames_presented: 1,                      // 普通真实帧基线。
            total_present_count: 0,                   // 尚无代理计数。
            available_queues: 0,                      // 尚未附着设备。
            display_width: 0,                         // 尚无交换链。
            display_height: 0,                        // 尚无交换链。
            fidelityfx_result: 1,                     // `FFX_API_RETURN_ERROR`代表尚未成功。
            blocker: None,                            // Bootstrap插件会发布准确原因。
            detail: String::new(),                    // 无原生诊断。
        }
    } // 默认状态结束。
} // Fsr3FrameGenerationRuntimeStatus默认实现结束。

impl Fsr3FrameGenerationRuntimeStatus {
    /// 只有代理、输入与真实Present倍数证据同时成立才算2x有效。
    pub fn is_effective_2x(&self) -> bool {
        self.phase == Fsr3FrameGenerationPhase::Active2x
            && self.requested
            && self.swapchain_ready
            && self.context_ready
            && self.inputs_ready
            && self.frames_presented >= 2
            && self.blocker.is_none() // 严格禁止把DLL加载或设置请求冒充成生效。
    } // is_effective_2x结束。
} // Fsr3FrameGenerationRuntimeStatus实现结束。

fn result_name(result: i32) -> String {
    let name = match result {
        0 => "成功",
        1 => "未分类错误",
        2 => "描述类型不匹配",
        3 => "Vulkan/运行库错误",
        4 => "未找到匹配 Provider",
        5 => "内存分配失败",
        6 => "参数无效",
        -3 => "Vulkan 初始化失败",
        -4 => "Vulkan 设备丢失",
        -1000001004 => "交换链已过期",
        _ => return format!("结果 {result}"),
    }; // FFX稳定结果与常见Vulkan错误映射结束。
    name.into() // UI取得拥有的短文本。
} // result_name结束。

#[cfg(test)] // 纯状态测试不启动GPU或加载AMD DLL。
mod tests {
    use super::*; // 验证公开有效性契约。

    #[test]
    fn render_input_failure_codes_round_trip() {
        let reasons = [
            Fsr3FrameGenerationInputFailure::Pending,
            Fsr3FrameGenerationInputFailure::ViewNotPresenting,
            Fsr3FrameGenerationInputFailure::CameraParameters,
            Fsr3FrameGenerationInputFailure::PrepassTextures,
            Fsr3FrameGenerationInputFailure::DepthTexture,
            Fsr3FrameGenerationInputFailure::MotionVectorTexture,
            Fsr3FrameGenerationInputFailure::OutputTexture,
            Fsr3FrameGenerationInputFailure::DepthInterop,
            Fsr3FrameGenerationInputFailure::MotionVectorInterop,
            Fsr3FrameGenerationInputFailure::RenderExtent,
            Fsr3FrameGenerationInputFailure::CommandBuffer,
            Fsr3FrameGenerationInputFailure::NonFiniteParameters,
            Fsr3FrameGenerationInputFailure::PrepareCall,
        ]; // 覆盖当前全部稳定编码。
        for reason in reasons {
            assert_eq!(
                Fsr3FrameGenerationInputFailure::from_code(reason.code()),
                reason,
            ); // RenderApp写入与主世界读取必须无损对应。
        }
    } // 编码往返测试结束。

    #[test]
    fn loaded_runtime_does_not_count_as_effective_without_present_evidence() {
        let mut status = Fsr3FrameGenerationRuntimeStatus {
            phase: Fsr3FrameGenerationPhase::Requested,
            requested: true,
            runtime_loaded: true,
            device_attached: true,
            swapchain_ready: true,
            context_ready: true,
            inputs_ready: true,
            frames_presented: 1,
            blocker: None,
            ..default()
        }; // 模拟全部初始化完成但只Present真实帧。
        assert!(!status.is_effective_2x()); // DLL和Prepare不能冒充2x。
        status.phase = Fsr3FrameGenerationPhase::Active2x;
        status.frames_presented = 2;
        assert!(status.is_effective_2x()); // 至少一张生成帧后成立。
    } // 测试结束。

    #[test]
    fn queue_blocker_keeps_actual_and_required_counts() {
        let blocker = Fsr3FrameGenerationBlocker::QueueUnavailable {
            available: 2,
            required: 4,
        }; // 模拟常见双队列设备。
        assert!(blocker.message().contains("2"));
        assert!(blocker.message().contains("4"));
    } // 测试结束。
} // 状态测试模块结束。
