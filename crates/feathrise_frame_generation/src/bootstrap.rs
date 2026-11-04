//! DefaultPlugins之前执行的Streamline身份、签名和Vulkan代理引导。
mod private_data;
mod verification; // 固定生产文件哈希与原生签名校验共同保护启动身份。 // 在设备创建前协商SDK交换链关联所需的privateData能力。

use std::{env, path::PathBuf}; // 开发构建使用Cargo私有目录，发布包使用exe旁的私有子目录。

use bevy::prelude::*; // 前置插件只写主世界资源与日志。
use wgpu_hal::vulkan::{
    // 本地最小补丁提供进程期入口和队列边界登记。
    QueueBoundaryCallbacks,
    set_entry_library_path,
    set_queue_boundary_callbacks,
}; // Vulkan代理登记导入结束。

use crate::{
    // 前置层只依赖原生窄桥和公开状态。
    native,
    status::{FrameGenerationBlocker, FrameGenerationPhase, FrameGenerationRuntimeStatus},
}; // crate内导入结束。

const PROJECT_ID: &str = "14b34155-a529-4df8-8d57-b4a76b98807d"; // 与现有Bevy NGX项目UUID保持唯一一致。

/// 后续设备附着插件读取的前置引导事实。
#[derive(Resource, Clone, Debug, Default)]
pub(crate) enum BootstrapState {
    #[default]
    Blocked, // 未满足身份、SDK或代理登记要求。
    Ready {
        runtime_dir: PathBuf,
    }, // slInit完成且ash将从同一已验签代理加载入口。
} // BootstrapState结束。

/// 必须在`DefaultPlugins`之前添加的Streamline引导插件。
pub struct StreamlineBootstrapPlugin; // 无可变配置；构建身份来自经过build.rs校验的环境。

impl Plugin for StreamlineBootstrapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameGenerationRuntimeStatus>(); // 设置页始终能读取明确阻断。
        let application_id = env!("BL5_STREAMLINE_APPLICATION_ID"); // build.rs已验证显式非零u32，缺省配置使用内部零哨兵。
        let application_id = application_id
            .parse::<u32>()
            .expect("build.rs must validate STREAMLINE_APPLICATION_ID"); // 缺省配置已转为内部零哨兵，原生层走本项目 UUID 路径。
        let runtime_dir = runtime_directory(); // 发布包优先使用exe旁白名单，开发构建回退Cargo私有目录。
        if let Err(detail) = verification::verify(&runtime_dir) {
            // 任何身份不符都在加载DLL前关闭本后端。
            publish_bootstrap_failure(
                app,
                FrameGenerationBlocker::BootstrapFailed { result: 19 },
                19,
                detail,
            ); // 官方InvalidIntegration，保留哈希或读取失败的精确文件。
            return; // 普通Vulkan路径继续。
        } // 固定版本验证结束。
        let Some(upscaling_dir) = dlss_wgpu::approved_windows_runtime_directory() else {
            // 复用超分后端唯一白名单选择，不扫描或猜测SDK路径。
            publish_bootstrap_failure(
                app,
                FrameGenerationBlocker::BootstrapFailed { result: 19 },
                19,
                "没有合格的DLSS超分/重建运行库目录".into(),
            ); // FG首次NGX初始化不能漏掉SR/RR。
            return; // 不建立缺少组合依赖的NGX生命周期。
        }; // SR/RR目录资格完成。
        if let Err(detail) = verification::verify_upscaling(&upscaling_dir) {
            // 签名之前还要锁定正式SR/RR文件版本。
            publish_bootstrap_failure(
                app,
                FrameGenerationBlocker::BootstrapFailed { result: 19 },
                19,
                detail,
            ); // 公开具体文件身份失败。
            return; // 不加载未知版本。
        } // 两个隔离目录均通过身份检查。
        let result = native::bootstrap(&runtime_dir, &upscaling_dir, application_id, PROJECT_ID); // 首次NGX初始化同时看到FG及SR/RR，原生层逐文件验签。
        if result != native::RESULT_OK {
            // slInit失败不得接管Vulkan入口。
            publish_bootstrap_failure(
                app,
                FrameGenerationBlocker::BootstrapFailed { result },
                result,
                native::last_error(), // 原生签名或初始化失败的具体阶段。
            ); // 保留官方结果和文本。
            return; // 普通Vulkan仍可继续启动。
        } // 原生引导结果结束。

        let callbacks = QueueBoundaryCallbacks {
            render_submit_start: Some(native::bl5_fg_on_render_submit_start), // 紧贴真实vkQueueSubmit前。
            render_submit_end: Some(native::bl5_fg_on_render_submit_end), // 紧贴真实vkQueueSubmit后。
            present_start: Some(native::bl5_fg_on_present_start), // 紧贴代理vkQueuePresent前。
            present_end: Some(native::bl5_fg_on_present_end),     // Present后查询一次真实输出帧数。
            present_failure: Some(native::bl5_fg_on_present_failure), // Surface错误不得沿用上一帧2x证据。
            device_destroy_start: Some(native::shutdown_before_device_destroy), // 最后先关全部SL插件，再释放宿主静态NGX loader，最后才允许VkDevice销毁。
        }; // 完整进程期回调快照结束。
        if set_queue_boundary_callbacks(callbacks).is_err() {
            // 二次登记意味着集成所有权不明确。
            native::shutdown(); // Vulkan尚未创建，可以立即安全清理。
            publish_bootstrap_failure(
                app,
                FrameGenerationBlocker::QueueHookRegistrationFailed,
                result,
                "Vulkan队列回调已被其他后端登记".into(), // 明确交换链所有权冲突。
            ); // 明确拒绝半接入。
            return; // 保留默认Vulkan路径。
        } // 队列边界登记结束。

        let interposer_path = runtime_dir.join("sl.interposer.dll"); // 与已验签且已加载的同一绝对文件一致。
        if set_entry_library_path(&interposer_path).is_err() {
            // ash入口已经被其他代码登记时不能中途替换。
            native::shutdown(); // 尚无Vulkan对象，可以关闭SDK。
            publish_bootstrap_failure(
                app,
                FrameGenerationBlocker::VulkanLoaderRegistrationFailed,
                result,
                "Vulkan加载器已初始化，无法登记Streamline代理".into(), // 明确初始化顺序失败。
            ); // 设置页显示集成次序问题。
            return; // 不创建混合入口实例。
        } // Vulkan加载器登记结束。

        wgpu_hal::vulkan::prefer_unorm_surface_formats(); // DLSS-G要求UNORM交换链图像。
        bevy::render::view::window::use_native_window_view_formats(); // 代理图像无MUTABLE保证；使用原生UNORM视图，最终upscaling显式编码sRGB。
        private_data::register(app); // RawVulkan回调必须早于DefaultPlugins创建设备，运行插件阶段已经太迟。
        app.insert_resource(BootstrapState::Ready { runtime_dir }); // 后续finish阶段可以附着真实适配器。
        let mut status = app
            .world_mut()
            .resource_mut::<FrameGenerationRuntimeStatus>();
        status.phase = FrameGenerationPhase::Ready; // 仅表示slInit与入口登记完成，不表示设备支持或2x有效。
        status.initialized = true; // 对应原生事实。
        status.blocker = None; // 设备检查会在DefaultPlugins完成后覆盖下一阶段。
        status.detail.clear(); // 清理前置引导提示。
        info!("Streamline 前置引导完成，等待 Vulkan 适配器能力检查"); // 文案明确没有宣称FG生效。
    } // build结束。
} // StreamlineBootstrapPlugin实现结束。

fn runtime_directory() -> PathBuf {
    // 发布后的exe不能依赖构建机OUT_DIR绝对路径。
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let deployed = parent.join("streamline-runtime"); // xtask只把七个验签生产DLL放入该隔离目录。
        if deployed.join("sl.interposer.dll").is_file() {
            return deployed; // 目录一旦存在就交给原生层逐文件验签；不绕过损坏发布包。
        }
    }
    PathBuf::from(env!("BL5_STREAMLINE_RUNTIME_DIR")) // cargo run/check使用build.rs已哈希校验的私有副本。
} // runtime_directory结束。

fn publish_bootstrap_failure(
    app: &mut App,
    blocker: FrameGenerationBlocker,
    result: i32,
    detail: String,
) {
    // 统一启动失败回退。
    warn!("DLSS Frame Generation 前置引导失败：{detail}"); // 同时覆盖Rust文件身份与原生初始化失败。
    let mut status = app
        .world_mut()
        .resource_mut::<FrameGenerationRuntimeStatus>();
    status.phase = FrameGenerationPhase::Blocked; // 普通渲染继续但FG不可用。
    status.initialized = false; // slInit没有形成可用生命周期。
    status.streamline_result = result; // 保留官方结果码。
    status.blocker = Some(blocker); // 公开单一主因。
    status.detail = detail; // 公开签名、身份或插件阶段文本。
    drop(status); // 插入资源和日志前释放借用。
    app.insert_resource(BootstrapState::Blocked); // 后续插件不尝试读取原生适配器。
} // publish_bootstrap_failure结束。
