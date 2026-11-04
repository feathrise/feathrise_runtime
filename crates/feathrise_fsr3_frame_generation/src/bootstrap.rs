//! DefaultPlugins之前执行的FidelityFX身份校验、额外队列请求和交换链回调登记。

#[path = "runtime_identity.rs"] // 身份复验属于启动功能域，不进入帧系统。
mod runtime_identity; // 独立封装编译固化哈希与完整来源资料白名单。

use std::{env, path::PathBuf}; // 发布包优先exe旁私有目录，开发构建使用Cargo私有输出。

#[path = "runtime_directory.rs"]
mod runtime_directory;

use bevy::prelude::*; // 前置插件只写主世界资源与一次性日志。
use wgpu_hal::vulkan::{
    set_queue_boundary_callbacks, set_requested_device_queue_count,
    set_swapchain_provider_callbacks,
}; // 本地wgpu-hal最小补丁提供进程期呈现所有权入口。

use crate::{
    native, // 原生哈希复验、动态 DLL 与静态 C ABI 回调。
    status::{
        Fsr3FrameGenerationBlocker, Fsr3FrameGenerationPhase, Fsr3FrameGenerationRuntimeStatus,
    },
}; // crate内导入结束。

const REQUIRED_QUEUE_COUNT: u32 = 4; // 官方Vulkan FG交换链即使同步模式也要求四个不同队列。

/// DefaultPlugins完成后设备附着阶段读取的启动事实。
#[derive(Resource, Clone, Debug, Default)]
pub(crate) enum BootstrapState {
    #[default]
    Blocked, // 未满足DLL或进程期Vulkan所有权要求。
    Ready {
        runtime_directory: PathBuf, // 实际已按编译固化哈希验证并加载的源码修订私有目录。
    },
} // BootstrapState结束。

/// 由lib.rs中的前置插件调用；必须发生在`DefaultPlugins`之前。
pub(crate) fn install(app: &mut App) {
    app.init_resource::<Fsr3FrameGenerationRuntimeStatus>(); // 设置页始终有明确事实可读。
    let runtime_directory = match runtime_directory() {
        Ok(directory) => directory,
        Err(detail) => {
            publish_failure_with_detail(app, Fsr3FrameGenerationBlocker::BootstrapFailed {
                result: native::RESULT_ERROR_PARAMETER,
            }, native::RESULT_ERROR_PARAMETER, detail);
            return;
        }
    }; // 只选择约定私有目录，不扫描系统或借用其他构建的缓存。
    if let Err(detail) = runtime_identity::verify_runtime_identity(&runtime_directory) {
        // 完整来源链必须与当前主程序构建一致。
        publish_failure_with_detail(
            app,
            Fsr3FrameGenerationBlocker::BootstrapFailed {
                result: native::RESULT_ERROR_PARAMETER,
            },
            native::RESULT_ERROR_PARAMETER,
            detail,
        ); // 精确字节不匹配时不允许其他源码修订或被替换的 manifest 自证身份。
        return;
    }
    let result = native::bootstrap(&runtime_directory); // 原生层再次按编译固化 SHA-256 验证并持有只读锁，按绝对路径加载。
    if result != native::RESULT_OK {
        publish_failure(
            app,
            Fsr3FrameGenerationBlocker::BootstrapFailed { result },
            result,
        ); // 普通Vulkan继续启动。
        return; // 不请求额外队列或登记代理。
    } // DLL引导结果结束。

    if set_requested_device_queue_count(REQUIRED_QUEUE_COUNT).is_err() {
        unsafe { native::bl5_fsr3_fg_shutdown() }; // Vulkan尚未创建，可立即释放DLL。
        publish_failure(
            app,
            Fsr3FrameGenerationBlocker::QueueReservationFailed,
            result,
        ); // 拒绝不确定的逻辑设备队列配置。
        return;
    } // 队列请求登记结束。
    if set_queue_boundary_callbacks(native::queue_callbacks()).is_err() {
        unsafe { native::bl5_fsr3_fg_shutdown() }; // 尚无VkDevice，幂等关闭。
        publish_failure(
            app,
            Fsr3FrameGenerationBlocker::QueueHookRegistrationFailed,
            result,
        ); // DLSS-G等后端已经拥有Present边界时明确互斥。
        return;
    } // Present边界登记结束。
    if set_swapchain_provider_callbacks(native::provider_callbacks()).is_err() {
        unsafe { native::bl5_fsr3_fg_shutdown() }; // provider尚未被Surface使用。
        publish_failure(
            app,
            Fsr3FrameGenerationBlocker::SwapchainProviderRegistrationFailed,
            result,
        ); // 不建立混合交换链所有权。
        return;
    } // 交换链provider登记结束。

    app.insert_resource(BootstrapState::Ready {
        runtime_directory: runtime_directory.clone(), // finish阶段日志保留准确来源。
    }); // 发布设备附着资格。
    let mut status = app
        .world_mut()
        .resource_mut::<Fsr3FrameGenerationRuntimeStatus>();
    status.phase = Fsr3FrameGenerationPhase::Ready; // 仅表示DLL和Vulkan入口准备完成。
    status.runtime_loaded = true; // 对应原生事实。
    status.fidelityfx_result = result; // 成功结果。
    status.blocker = None; // 后续设备阶段覆盖主因。
    status.detail.clear(); // 清理默认文本。
    info!(
        "FSR 3.1 帧生成前置引导完成，源码修订 {}（未签名），已请求 {} 条 Vulkan 队列，运行目录 {}",
        env!("BL5_FSR3_RUNTIME_REVISION"),
        REQUIRED_QUEUE_COUNT, // 明确来源身份，不能将本项目编译结果称作 AMD 官方签名产物。
        runtime_directory.display()
    ); // 不冒充代理或2x已经生效。
} // install结束。

fn runtime_directory() -> Result<PathBuf, String> {
    let executable = env::current_exe().map_err(|error| format!("无法定位 FSR 主程序：{error}"))?;
    runtime_directory::resolve(
        &executable,
        std::path::Path::new(env!("OUT_DIR")),
        std::path::Path::new(env!("BL5_FSR3_FRAME_GENERATION_RUNTIME_DIR")),
    )
} // runtime_directory结束。

fn publish_failure(app: &mut App, blocker: Fsr3FrameGenerationBlocker, result: i32) {
    let detail = native::last_error(); // 启动边界同步复制，不进入稳定帧。
    publish_failure_with_detail(app, blocker, result, detail);
} // publish_failure结束。

fn publish_failure_with_detail(
    app: &mut App,
    blocker: Fsr3FrameGenerationBlocker,
    result: i32,
    detail: String,
) {
    let mut status = app
        .world_mut()
        .resource_mut::<Fsr3FrameGenerationRuntimeStatus>();
    status.phase = Fsr3FrameGenerationPhase::Blocked; // 普通渲染继续。
    status.runtime_loaded = false; // 没有形成可用生命周期。
    status.fidelityfx_result = result; // 保留原生结果。
    status.blocker = Some(blocker); // 公开单一主因。
    status.detail = detail.clone(); // UI可显示准确阶段。
    drop(status); // 插入资源和日志前释放借用。
    app.insert_resource(BootstrapState::Blocked); // finish阶段安全跳过设备附着。
    warn!("FSR 3.1 帧生成前置引导失败：{detail}"); // 一次性日志。
} // publish_failure_with_detail结束。
