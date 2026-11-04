//! Streamline C++桥的唯一Rust FFI声明与安全窄封装。
//!
//! 【功能设计】本模块不把NVIDIA头文件类型泄露给业务层；所有跨ABI数据只使用固定宽度整数、
//! POD数组和调用方分配缓冲。原生全局状态由SDK的Present线程更新，Rust只复制快照。

use std::{
    // FFI字符串和路径转换只在启动、错误刷新与退出边界执行。
    ffi::{CStr, CString, OsStr}, // 项目UUID使用C字符串，诊断使用调用方缓冲复制。
    os::windows::ffi::OsStrExt as _, // Windows私有SDK目录必须无损转为UTF-16。
    path::Path,                  // 启动入口接受已经由build.rs锁定的私有运行目录。
}; // 标准库导入结束。

/// Streamline成功结果；与`sl::Result::eOk`的稳定枚举值一致。
pub(crate) const RESULT_OK: i32 = 0; // 业务层只需要判断成功，不在Rust复制厂商枚举。

/// 原生桥向Rust公开的无锁运行状态快照。
#[repr(C)] // 必须与`Bl5FgNativeStatus`逐字段、逐对齐一致。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeStatus {
    pub(crate) last_result: i32,          // 最近一次Streamline API结果。
    pub(crate) dlssg_status: u32,         // `sl::DLSSGStatus`位集合。
    pub(crate) frames_presented: u32,     // 最近一次宿主Present实际输出的总帧数。
    pub(crate) max_generated_frames: u32, // 当前设备允许的最大插入帧数。
    pub(crate) min_dimension: u32,        // 当前实现要求的最小交换链边长。
    pub(crate) estimated_vram: u64,       // 显式查询时的估算字节数；稳定帧默认为零。
    pub(crate) initialized: u8,           // `slInit`是否完成。
    pub(crate) attached: u8,              // Vulkan物理设备是否通过能力检查并附着。
    pub(crate) requested: u8,             // 插件是否接受了开启请求。
    pub(crate) effective: u8,             // 最近Present是否真实产生至少一个中间帧。
    pub(crate) total_present_count: u64,  // SDK已确认增量的累计呈现计数。
    pub(crate) presentation_epoch: u64,   // 与累计计数一致发布的采样代次。
    pub(crate) presentation_ready: u8,    // 原生快照完整且已有可信Present。
} // NativeStatus结束。
const _: () = assert!(core::mem::size_of::<NativeStatus>() == 64); // 与C++静态断言共同锁定跨ABI写入边界。
const _: () = assert!(core::mem::offset_of!(NativeStatus, total_present_count) == 40); // 确保累计计数在两种语言中对齐相同。

/// Vulkan纹理标签所需的只读原生描述。
#[repr(C)] // C++桥按同布局构造`sl::Resource`，不持有Rust对象地址。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeTexture {
    pub(crate) image: u64,        // `VkImage`原始位模式。
    pub(crate) view: u64,         // `VkImageView`原始位模式。
    pub(crate) layout: u32,       // 标签命令缓冲处的`VkImageLayout`数值。
    pub(crate) width: u32,        // 有效子资源宽度。
    pub(crate) height: u32,       // 有效子资源高度。
    pub(crate) format: u32,       // `VkFormat`原始枚举值。
    pub(crate) mip_levels: u32,   // 当前图像完整mip数量，首期均为一。
    pub(crate) array_layers: u32, // 当前图像完整数组层数，首期均为一。
    pub(crate) usage: u32,        // `VkImageUsageFlags`原始位集合。
    pub(crate) aspect_mask: u32,  // 被标签子资源的`VkImageAspectFlags`。
} // NativeTexture结束。

/// 每帧相机常量的稳定跨ABI快照。
#[repr(C)] // 矩阵由Rust预先转成Streamline要求的行主序。
#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeConstants {
    pub(crate) camera_view_to_clip: [f32; 16], // 当前无抖动视图到裁剪矩阵。
    pub(crate) clip_to_camera_view: [f32; 16], // 当前无抖动裁剪到视图矩阵。
    pub(crate) clip_to_prev_clip: [f32; 16],   // 当前裁剪到上一帧裁剪矩阵。
    pub(crate) prev_clip_to_clip: [f32; 16],   // 上一帧裁剪到当前裁剪矩阵。
    pub(crate) jitter_offset: [f32; 2],        // Bevy运动矢量采用无抖动矩阵，首期为零。
    pub(crate) mvec_scale: [f32; 2],           // Bevy预通道已输出归一化UV位移，因此为一。
    pub(crate) camera_pos: [f32; 3],           // 世界空间相机位置。
    pub(crate) camera_up: [f32; 3],            // 世界空间相机上方向。
    pub(crate) camera_right: [f32; 3],         // 世界空间相机右方向。
    pub(crate) camera_forward: [f32; 3],       // 世界空间相机前方向。
    pub(crate) camera_near: f32,               // 透视近裁面。
    pub(crate) camera_far: f32,                // 玩法可见性远裁面。
    pub(crate) camera_fov: f32,                // 垂直视场角弧度。
    pub(crate) camera_aspect_ratio: f32,       // 宽高比。
    pub(crate) reset: u8,                      // 相机断续、首次帧或尺寸变化时重置时间历史。
    pub(crate) _padding: [u8; 3],              // 保持后续C++读取自然对齐稳定。
} // NativeConstants结束。

impl Default for NativeConstants {
    fn default() -> Self {
        // 无有效视图时提供完全有限的单位矩阵安全值。
        Self {
            camera_view_to_clip: [0.0; 16],   // 调用方在实际标签前必定覆盖。
            clip_to_camera_view: [0.0; 16],   // 调用方在实际标签前必定覆盖。
            clip_to_prev_clip: [0.0; 16],     // 调用方在实际标签前必定覆盖。
            prev_clip_to_clip: [0.0; 16],     // 调用方在实际标签前必定覆盖。
            jitter_offset: [0.0; 2],          // 首期不把DLSS内部抖动重复传给FG。
            mvec_scale: [1.0; 2],             // Bevy运动矢量已经是归一化UV位移。
            camera_pos: [0.0; 3],             // 安全零值。
            camera_up: [0.0, 1.0, 0.0],       // Bevy世界上方向。
            camera_right: [1.0, 0.0, 0.0],    // Bevy世界右方向。
            camera_forward: [0.0, 0.0, -1.0], // Bevy相机默认朝向负Z。
            camera_near: 0.1,                 // Bevy默认透视近裁面。
            camera_far: 1000.0,               // Bevy默认可见性远裁面。
            camera_fov: core::f32::consts::FRAC_PI_4, // Bevy默认四十五度。
            camera_aspect_ratio: 1.0,         // 尺寸未解析前的有限值。
            reset: 1,                         // 默认不复用未知历史。
            _padding: [0; 3],                 // 明确清零ABI填充。
        }
    } // 默认常量构造结束。
} // NativeConstants默认实现结束。

unsafe extern "C" {
    // 所有符号由同crate的静态C++桥提供，不从未知DLL直接链接。
    fn bl5_fg_bootstrap(
        runtime_dir: *const u16,             // NUL结尾UTF-16私有运行目录。
        upscaling_dir: *const u16, // 既有仅SR/RR目录，NGX首次初始化必须看到组合内全部功能。
        application_id: u32,       // NVIDIA分配的数字应用ID。
        project_id: *const std::ffi::c_char, // 本项目现有NGX UUID。
        verbose: bool,             // debug构建允许官方详细日志。
    ) -> i32; // 返回`sl::Result`数值。
    fn bl5_fg_attach_vulkan(physical_device: u64) -> i32; // 设备创建后能力检查。
    fn bl5_fg_set_enabled(
        enabled: bool,       // 固定2x开关。
        width: u32,          // 最终交换链宽度。
        height: u32,         // 最终交换链高度。
        color_format: u32,   // 交换链VkFormat。
        depth_width: u32,    // 深度/运动矢量宽度。
        depth_height: u32,   // 深度/运动矢量高度。
        depth_format: u32,   // 深度VkFormat。
        motion_format: u32,  // 运动矢量VkFormat。
        hudless_format: u32, // 无HUD颜色VkFormat。
        ui_format: u32,      // 独立预乘UI颜色与Alpha的VkFormat。
    ) -> i32; // 返回`sl::Result`数值。
    fn bl5_fg_set_driver_frame(frame_index: u32); // 发布真实提交帧号给队列回调。
    fn bl5_fg_sleep(frame_index: u32) -> i32; // Simulation前执行Reflex sleep。
    fn bl5_fg_mark(frame_index: u32, marker: u32) -> i32; // 提交PCL时序标记。
    fn bl5_fg_set_constants(frame_index: u32, constants: *const NativeConstants) -> i32; // 每帧公共相机常量。
    fn bl5_fg_tag_inputs(
        frame_index: u32,              // 与常量和Present标记一致的帧号。
        command_buffer: u64,           // 当前正在记录的VkCommandBuffer。
        depth: *const NativeTexture,   // 深度预通道纹理。
        motion: *const NativeTexture,  // 运动矢量预通道纹理。
        hudless: *const NativeTexture, // 后处理完成、UI之前的颜色副本。
        output_width: u32,             // 最终交换链宽度。
        output_height: u32,            // 最终交换链高度。
    ) -> i32; // 返回`slSetTagForFrame`结果。
    fn bl5_fg_clear_tags(frame_index: u32) -> i32; // 无有效玩法帧时撤销全部输入。
    fn bl5_fg_tag_ui(frame_index: u32, command_buffer: u64, ui: *const NativeTexture) -> i32; // 同帧透明UI由SDK立即复制，不持有跨帧wgpu借用。
    pub(crate) fn bl5_fg_shutdown(); // wgpu最后一个设备资源释放后、VkDevice销毁前关闭Streamline。
    fn bl5_fg_get_status(output: *mut NativeStatus); // 复制无锁状态快照。
    fn bl5_fg_copy_last_error(output: *mut std::ffi::c_char, capacity: usize); // 加锁复制诊断文本。
    pub(crate) fn bl5_fg_on_render_submit_start(); // wgpu vkQueueSubmit前回调。
    pub(crate) fn bl5_fg_on_render_submit_end(); // wgpu vkQueueSubmit后回调。
    pub(crate) fn bl5_fg_on_present_start(); // wgpu vkQueuePresent前回调。
    pub(crate) fn bl5_fg_on_present_end(); // wgpu vkQueuePresent后回调并查询真实倍数。
    pub(crate) fn bl5_fg_on_present_failure(); // Surface丢失/过期时撤销旧Present倍数证据。
} // 原生符号声明结束。

pub(crate) fn bootstrap(
    runtime_dir: &Path,
    upscaling_dir: &Path,
    application_id: u32,
    project_id: &str,
) -> i32 {
    // 安全构造启动字符串。
    let mut runtime_wide = OsStr::new(runtime_dir.as_os_str())
        .encode_wide()
        .collect::<Vec<_>>(); // 启动期一次小分配，完整保留Windows路径。
    runtime_wide.push(0); // Win32路径必须NUL结尾。
    let upscaling_wide: Vec<u16> = upscaling_dir
        .as_os_str()
        .encode_wide()
        .chain(core::iter::once(0))
        .collect(); // 独立SR/RR白名单路径仅在同步初始化期间借用。
    let project_id = CString::new(project_id).expect("project UUID must not contain NUL"); // 编译期固定UUID必定合法。
    unsafe {
        // 指针只在同步调用期间有效，原生桥立即复制路径和身份。
        bl5_fg_bootstrap(
            runtime_wide.as_ptr(),
            upscaling_wide.as_ptr(), // 不传整个DLSS SDK目录。
            application_id,
            project_id.as_ptr(),
            cfg!(debug_assertions),
        )
    }
} // bootstrap结束。

pub(crate) fn attach_vulkan(physical_device: u64) -> i32 {
    // 只传原生句柄位模式。
    unsafe { bl5_fg_attach_vulkan(physical_device) } // 原生桥不取得句柄所有权。
} // attach_vulkan结束。

#[allow(clippy::too_many_arguments)] // 参数逐一对应SDK选项，打包会掩盖格式/尺寸错配。
pub(crate) fn set_enabled(
    enabled: bool,       // 玩家与运行资格共同解析后的最终请求。
    width: u32,          // 输出宽度。
    height: u32,         // 输出高度。
    color_format: u32,   // 输出格式。
    depth_width: u32,    // 输入宽度。
    depth_height: u32,   // 输入高度。
    depth_format: u32,   // 深度格式。
    motion_format: u32,  // 运动格式。
    hudless_format: u32, // 无HUD颜色格式。
    ui_format: u32,      // 完整UI快照格式，参与选项规格换代。
) -> i32 {
    unsafe {
        // 全部值均来自本帧已存在的wgpu资源描述。
        bl5_fg_set_enabled(
            enabled,
            width,
            height,
            color_format,
            depth_width,
            depth_height,
            depth_format,
            motion_format,
            hudless_format,
            ui_format, // 与C++桥的独立UI格式字段保持相同顺序。
        )
    }
} // set_enabled结束。

pub(crate) fn set_driver_frame(frame_index: u32) {
    // RenderApp在提交阶段前发布帧号。
    unsafe { bl5_fg_set_driver_frame(frame_index) } // 单个原子写，无阻塞。
} // set_driver_frame结束。

pub(crate) fn sleep(frame_index: u32) -> i32 {
    // 主线程Simulation开始前调用。
    unsafe { bl5_fg_sleep(frame_index) } // SDK内部按Reflex契约节流。
} // sleep结束。

pub(crate) fn mark(frame_index: u32, marker: u32) -> i32 {
    // 主线程只发送Simulation阶段标记。
    unsafe { bl5_fg_mark(frame_index, marker) } // marker由本crate固定常量提供。
} // mark结束。

pub(crate) fn set_constants(frame_index: u32, constants: &NativeConstants) -> i32 {
    // 同步复制相机常量。
    unsafe { bl5_fg_set_constants(frame_index, constants) } // 原生桥在返回前完成SDK调用。
} // set_constants结束。

pub(crate) fn tag_inputs(
    frame_index: u32,        // 当前Bevy渲染帧号。
    command_buffer: u64,     // 仍在记录的原生命令缓冲。
    depth: &NativeTexture,   // 当前深度。
    motion: &NativeTexture,  // 当前运动矢量。
    hudless: &NativeTexture, // 当前无HUD颜色副本。
    output_width: u32,       // 输出宽度。
    output_height: u32,      // 输出高度。
) -> i32 {
    unsafe {
        // 三个资源由RenderApp持有至少到本帧Present结束。
        bl5_fg_tag_inputs(
            frame_index,
            command_buffer,
            depth,
            motion,
            hudless,
            output_width,
            output_height,
        )
    }
} // tag_inputs结束。

pub(crate) fn clear_tags(frame_index: u32) -> i32 {
    // 菜单、加载、暂停或缺输入时撤销旧标签。
    unsafe { bl5_fg_clear_tags(frame_index) } // 空标签不引用任何wgpu资源。
} // clear_tags结束。

pub(crate) fn tag_ui(frame_index: u32, command_buffer: u64, ui: &NativeTexture) -> i32 {
    // 仅在真实透明层完整捕获后登记最后一项输入。
    unsafe { bl5_fg_tag_ui(frame_index, command_buffer, ui) } // SDK在当前命令缓冲中复制eOnlyValidNow图像，返回后不保留Rust指针。
} // UI标签窄桥结束。

pub(crate) fn status() -> NativeStatus {
    // 低频复制全部原子字段。
    let mut output = NativeStatus::default(); // 原生桥即使提前返回也保留安全零值。
    unsafe { bl5_fg_get_status(&mut output) } // 输出指针只在同步调用内有效。
    output // 返回主世界拥有的值快照。
} // status结束。

pub(crate) fn last_error() -> String {
    // 诊断刷新不在帧关键GPU提交路径执行。
    let mut buffer = [0_i8; 1024]; // 与原生固定错误缓冲容量一致。
    unsafe { bl5_fg_copy_last_error(buffer.as_mut_ptr(), buffer.len()) } // 原生桥加锁并保证NUL结尾。
    unsafe { CStr::from_ptr(buffer.as_ptr()) } // 同一栈缓冲已保证NUL结尾。
        .to_string_lossy()
        .into_owned() // 状态资源取得独立所有权，退出后不引用原生内存。
} // last_error结束。

pub(crate) fn shutdown() {
    // 应用退出时显式关闭，保持幂等。
    unsafe { bl5_fg_shutdown() } // 原生桥先关FG再关Streamline。
} // shutdown结束。

pub(crate) unsafe extern "C" fn shutdown_before_device_destroy() {
    // HAL最后一个设备资源释放后调用，VkDevice此时仍有效。
    unsafe { bl5_fg_shutdown() }; // 第一步同步关闭全部SL插件与其独立静态NGX loader，不能反转顺序。
    if status().initialized != 0 {
        // 原生层只有slShutdown成功才撤销初始化事实。
        bevy::log::error!(
            "SL关闭失败，不能在其插件仍存活时关闭共享宿主NGX：{}",
            last_error()
        ); // 保留明确的最终清理失败，不能伪装为无泄漏。
        return; // 不在未知生命周期上执行第二套loader关闭。
    } // 外部管理器完成门结束。
    for outcome in unsafe { dlss_wgpu::shutdown_deferred_ngx_devices() } {
        // 第二步才关闭宿主SR/RR loader，列表取走保证重复回调幂等。
        if outcome.succeeded {
            // NGX成功码并非Streamline的零值，使用宿主官方判定。
            bevy::log::info!(
                device = outcome.device,
                result = outcome.result,
                "SL关闭后宿主NGX延迟Shutdown1完成"
            ); // 记录真实原生结果用于泄漏实测审计。
        } else {
            // 最终设备销毁边界无法安全延后重试，必须明确暴露失败。
            bevy::log::error!(
                device = outcome.device,
                result = outcome.result,
                "SL关闭后宿主NGX延迟Shutdown1失败"
            ); // 不把失败伪装成已完成资源释放。
        } // 原生结果日志结束。
    } // 所有待关闭宿主设备结果处理结束。
} // HAL最终设备销毁回调结束。
