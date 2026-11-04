//! FidelityFX C++桥的唯一Rust FFI声明与安全窄封装。
//!
//! 【功能设计】跨ABI只传固定宽度POD和Vulkan句柄位模式；官方头、上下文及函数指针全部留在
//! C++侧。Rust资源由Bevy/wgpu继续拥有，桥只在同步调用和对应VkDevice生命周期内借用。

use std::{
    ffi::{CStr, OsStr},              // 原生诊断使用调用方缓冲，路径使用无损UTF-16。
    os::windows::ffi::OsStrExt as _, // Windows SDK私有目录编码。
    path::Path,                      // 启动入口只接受build.rs锁定的运行目录。
}; // 标准库导入结束。

use ash::vk; // provider回调必须与wgpu-hal的Vulkan ABI完全一致。

pub(crate) const RESULT_OK: i32 = 0; // `FFX_API_RETURN_OK`稳定值。
pub(crate) const RESULT_ERROR_PARAMETER: i32 = 6; // 宿主身份门失败使用稳定参数错误，不伪造Vulkan结果。

/// wgpu拥有的二维Vulkan图像描述；不携带内存或销毁权。
#[repr(C)] // 必须与C++ `Bl5Fsr3Image`逐字段一致。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeImage {
    pub(crate) image: u64,        // VkImage位模式。
    pub(crate) format: u32,       // VkFormat数值。
    pub(crate) width: u32,        // 基础级宽度。
    pub(crate) height: u32,       // 基础级高度。
    pub(crate) depth: u32,        // 二维纹理固定一。
    pub(crate) mip_levels: u32,   // 完整mip数。
    pub(crate) array_layers: u32, // 完整数组层数。
    pub(crate) sample_count: u32, // VkSampleCountFlagBits数值。
    pub(crate) usage: u32,        // VkImageUsageFlags位集合。
    pub(crate) flags: u32,        // VkImageCreateFlags位集合。
} // NativeImage结束。

/// 当前真实渲染帧提交给FSR FG Prepare的完整不可变快照。
#[repr(C)] // 必须与C++ `Bl5Fsr3PrepareDesc`逐字段、逐对齐一致。
#[derive(Clone, Copy, Debug)]
pub(crate) struct NativePrepareDesc {
    pub(crate) command_buffer: u64,         // 当前VkCommandBuffer位模式。
    pub(crate) depth: NativeImage,          // 反向Z深度。
    pub(crate) motion_vectors: NativeImage, // Bevy运动矢量。
    pub(crate) render_width: u32,           // 预通道有效宽度。
    pub(crate) render_height: u32,          // 预通道有效高度。
    pub(crate) jitter_x: f32,               // 当前像素抖动X。
    pub(crate) jitter_y: f32,               // 当前像素抖动Y。
    pub(crate) motion_scale_x: f32,         // 运动矢量像素恢复比例X。
    pub(crate) motion_scale_y: f32,         // 运动矢量像素恢复比例Y。
    pub(crate) frame_time_ms: f32,          // 真实帧间隔毫秒。
    pub(crate) camera_near: f32,            // 作者近裁面。
    pub(crate) camera_fov_y: f32,           // 垂直FOV弧度。
    pub(crate) camera_position: [f32; 3],   // 世界位置。
    pub(crate) camera_up: [f32; 3],         // 世界上方向。
    pub(crate) camera_right: [f32; 3],      // 世界右方向。
    pub(crate) camera_forward: [f32; 3],    // 世界前方向。
    pub(crate) requested: u8,               // 最终资格请求。
    pub(crate) reset: u8,                   // 时间历史断续。
    pub(crate) padding: [u8; 2],            // 显式ABI填充。
} // NativePrepareDesc结束。

/// 原生桥向主世界公开的低频运行状态快照。
#[repr(C)] // 必须与C++ `Bl5Fsr3NativeStatus`逐字段一致。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeStatus {
    pub(crate) last_result: i32,        // 最近FFX或Vulkan结果。
    pub(crate) available_queues: u32,   // 当前角色队列数量。
    pub(crate) last_present_count: u64, // 代理累计Present数。
    pub(crate) last_present_delta: u32, // 最近完整120宿主帧统计窗口的近似呈现倍率。
    pub(crate) display_width: u32,      // 当前交换链宽度。
    pub(crate) display_height: u32,     // 当前交换链高度。
    pub(crate) runtime_loaded: u8,      // 修订DLL已通过固化字节身份验证并加载。
    pub(crate) attached: u8,            // 设备和四队列已附着。
    pub(crate) swapchain_ready: u8,     // 代理拥有当前交换链。
    pub(crate) context_ready: u8,       // FG核心上下文可用。
    pub(crate) requested: u8,           // 最近业务请求。
    pub(crate) effective: u8,           // 连续统计窗口具有真实生成及近似2x呈现证据。
    pub(crate) padding: [u8; 2],        // 固定结构大小。
} // NativeStatus结束。

unsafe extern "C" {
    fn bl5_fsr3_fg_bootstrap(runtime_directory: *const u16) -> i32; // 修订DLL固化SHA、加载和导出解析。
    fn bl5_fsr3_fg_attach_vulkan(
        physical_device: u64,       // VkPhysicalDevice位模式。
        device: u64,                // VkDevice位模式。
        queues: *const u64,         // 调用期固定数组。
        queue_families: *const u32, // 每个角色对应的真实队列族。
        queue_count: u32,           // 实际取得数量。
    ) -> i32; // 设备附着结果。
    pub(crate) fn bl5_fsr3_fg_provider_is_available() -> u8; // wgpu Surface创建前能力查询。
    pub(crate) fn bl5_fsr3_fg_create_swapchain(
        device: vk::Device,
        create_info: *const vk::SwapchainCreateInfoKHR<'static>,
        allocator: *const vk::AllocationCallbacks<'static>,
        output: *mut vk::SwapchainKHR,
    ) -> vk::Result; // FFX代理创建或原生回退触发点。
    pub(crate) fn bl5_fsr3_fg_destroy_swapchain(
        device: vk::Device,
        swapchain: vk::SwapchainKHR,
        allocator: *const vk::AllocationCallbacks<'static>,
    ); // FFX配对销毁。
    pub(crate) fn bl5_fsr3_fg_get_swapchain_images(
        device: vk::Device,
        swapchain: vk::SwapchainKHR,
        count: *mut u32,
        images: *mut vk::Image,
    ) -> vk::Result; // FFX图像枚举替换。
    pub(crate) fn bl5_fsr3_fg_acquire_next_image(
        device: vk::Device,
        swapchain: vk::SwapchainKHR,
        timeout: u64,
        semaphore: vk::Semaphore,
        fence: vk::Fence,
        image_index: *mut u32,
    ) -> vk::Result; // FFX获取替换。
    pub(crate) fn bl5_fsr3_fg_queue_present(
        queue: vk::Queue,
        present_info: *const vk::PresentInfoKHR<'_>,
    ) -> vk::Result; // FFX呈现替换。
    pub(crate) fn bl5_fsr3_fg_set_hdr_metadata(
        device: vk::Device,
        swapchain_count: u32,
        swapchains: *const vk::SwapchainKHR,
        metadata: *const vk::HdrMetadataEXT<'_>,
    ); // 可选HDR元数据替换。
    pub(crate) fn bl5_fsr3_fg_get_last_present_count(swapchain: vk::SwapchainKHR) -> u64; // 累计倍数证据。
    fn bl5_fsr3_fg_prepare(input: *const NativePrepareDesc) -> i32; // 当前命令缓冲同步Prepare。
    fn bl5_fsr3_fg_set_ui_resources(ui: *const NativeImage) -> i32; // 只借用本帧真实预乘Alpha图像，场景继续走既有窗口输出。
    fn bl5_fsr3_fg_disable(); // 缺输入或关闭时撤销旧配置。
    pub(crate) fn bl5_fsr3_fg_on_present_start(); // wgpu Present前累计计数基线。
    pub(crate) fn bl5_fsr3_fg_on_present_end(); // wgpu Present后发布真实差值。
    pub(crate) fn bl5_fsr3_fg_on_present_failure(); // Surface失败撤销旧证据。
    pub(crate) fn bl5_fsr3_fg_shutdown(); // VkDevice销毁前释放全部FFX对象和DLL。
    pub(crate) fn bl5_fsr3_fg_quiesce_swapchain(); // Surface 等待设备前停止 FFX 后台 Present，保留代理句柄供配对销毁。
    fn bl5_fsr3_fg_get_status(output: *mut NativeStatus); // 无锁状态复制。
    fn bl5_fsr3_fg_copy_last_error(output: *mut std::ffi::c_char, capacity: usize); // 加锁诊断复制。
} // 原生符号声明结束。

pub(crate) fn bootstrap(runtime_directory: &Path) -> i32 {
    let mut wide = OsStr::new(runtime_directory.as_os_str())
        .encode_wide()
        .collect::<Vec<_>>(); // 启动期一次小分配，无损保留Windows路径。
    wide.push(0); // Win32绝对路径必须NUL结尾。
    unsafe { bl5_fsr3_fg_bootstrap(wide.as_ptr()) } // 原生层只在同步调用内读取路径。
} // bootstrap结束。

pub(crate) fn attach_vulkan(
    physical_device: u64,
    device: u64,
    queues: &[u64],
    queue_families: &[u32],
) -> i32 {
    if queues.len() != queue_families.len() {
        return RESULT_ERROR_PARAMETER; // 禁止跨ABI读取长度不一致的数组。
    }
    unsafe {
        bl5_fsr3_fg_attach_vulkan(
            physical_device,
            device,
            queues.as_ptr(),
            queue_families.as_ptr(),
            queues.len().try_into().unwrap_or(u32::MAX), // 数量有界于Vulkan族队列数。
        )
    } // 指针仅在同步调用内有效。
} // attach_vulkan结束。

pub(crate) fn prepare(input: &NativePrepareDesc) -> i32 {
    unsafe { bl5_fsr3_fg_prepare(input) } // 纹理和命令缓冲至少活到调用返回及本帧提交。
} // prepare结束。

pub(crate) fn disable() {
    unsafe { bl5_fsr3_fg_disable() } // 幂等关闭，不销毁代理交换链。
} // disable结束。

pub(crate) fn status() -> NativeStatus {
    let mut output = NativeStatus::default(); // 提前返回仍保留安全零值。
    unsafe { bl5_fsr3_fg_get_status(&mut output) } // 同步复制，不借用原生内存。
    output // 返回Rust拥有快照。
} // status结束。

pub(crate) fn last_error() -> String {
    let mut buffer = [0_i8; 1024]; // 与原生固定缓冲容量一致。
    unsafe { bl5_fsr3_fg_copy_last_error(buffer.as_mut_ptr(), buffer.len()) } // 原生保证NUL结尾。
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned() // 状态资源取得独立文本。
} // last_error结束。

unsafe extern "system" fn provider_is_available() -> u8 {
    unsafe { bl5_fsr3_fg_provider_is_available() } // Windows x64下转接C桥到Vulkan system ABI。
} // provider_is_available结束。

unsafe extern "system" fn create_swapchain(
    device: vk::Device,
    create_info: *const vk::SwapchainCreateInfoKHR<'static>,
    allocator: *const vk::AllocationCallbacks<'static>,
    output: *mut vk::SwapchainKHR,
) -> vk::Result {
    unsafe { bl5_fsr3_fg_create_swapchain(device, create_info, allocator, output) } // 同步转发全部借用指针。
} // create_swapchain结束。

unsafe extern "system" fn destroy_swapchain(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    allocator: *const vk::AllocationCallbacks<'static>,
) {
    unsafe { bl5_fsr3_fg_destroy_swapchain(device, swapchain, allocator) } // 与provider创建配对。
} // destroy_swapchain结束。

unsafe extern "system" fn get_swapchain_images(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    count: *mut u32,
    images: *mut vk::Image,
) -> vk::Result {
    unsafe { bl5_fsr3_fg_get_swapchain_images(device, swapchain, count, images) } // 保留Vulkan两阶段语义。
} // get_swapchain_images结束。

unsafe extern "system" fn acquire_next_image(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    timeout: u64,
    semaphore: vk::Semaphore,
    fence: vk::Fence,
    image_index: *mut u32,
) -> vk::Result {
    unsafe {
        bl5_fsr3_fg_acquire_next_image(device, swapchain, timeout, semaphore, fence, image_index)
    } // FFX代理负责图像获取同步。
} // acquire_next_image结束。

unsafe extern "system" fn queue_present(
    queue: vk::Queue,
    present_info: *const vk::PresentInfoKHR<'_>,
) -> vk::Result {
    unsafe { bl5_fsr3_fg_queue_present(queue, present_info) } // FFX在内部生成并呈现固定2x。
} // queue_present结束。

unsafe extern "system" fn set_hdr_metadata(
    device: vk::Device,
    swapchain_count: u32,
    swapchains: *const vk::SwapchainKHR,
    metadata: *const vk::HdrMetadataEXT<'_>,
) {
    unsafe { bl5_fsr3_fg_set_hdr_metadata(device, swapchain_count, swapchains, metadata) } // 可选扩展转发。
} // set_hdr_metadata结束。

unsafe extern "system" fn get_last_present_count(swapchain: vk::SwapchainKHR) -> u64 {
    unsafe { bl5_fsr3_fg_get_last_present_count(swapchain) } // 累计真实Present证据。
} // get_last_present_count结束。

pub(crate) fn provider_callbacks() -> wgpu_hal::vulkan::SwapchainProviderCallbacks {
    wgpu_hal::vulkan::SwapchainProviderCallbacks {
        is_available: provider_is_available, // 只有设备附着后为真。
        create_swapchain,                    // 首次失败由wgpu回退原生创建。
        destroy_swapchain,                   // 与代理创建严格配对。
        get_swapchain_images,                // FFX内部图像集合。
        acquire_next_image,                  // FFX同步获取路径。
        queue_present,                       // FFX固定2x Present路径。
        set_hdr_metadata,                    // 可选HDR转发。
        get_last_present_count,              // 真实输出计数。
        quiesce_swapchain: bl5_fsr3_fg_quiesce_swapchain, // 先停止后台队列调用，避免与宿主 vkDeviceWaitIdle 并发。
    } // 完整进程期回调值。
} // provider_callbacks结束。

pub(crate) fn queue_callbacks() -> wgpu_hal::vulkan::QueueBoundaryCallbacks {
    wgpu_hal::vulkan::QueueBoundaryCallbacks {
        render_submit_start: None, // FSR首版不需要Reflex/PCL提交标记。
        render_submit_end: None,   // 同上。
        present_start: Some(bl5_fsr3_fg_on_present_start), // 累计计数基线。
        present_end: Some(bl5_fsr3_fg_on_present_end), // 发布真实2x证据。
        present_failure: Some(bl5_fsr3_fg_on_present_failure), // 撤销旧证据。
        device_destroy_start: Some(bl5_fsr3_fg_shutdown), // VkDevice销毁前清理DLL。
    } // 完整队列边界回调。
} // queue_callbacks结束。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_struct_sizes_match_the_cpp_static_contract() {
        assert_eq!(std::mem::size_of::<NativeImage>(), 48);
        assert_eq!(std::mem::size_of::<NativePrepareDesc>(), 192);
        assert_eq!(std::mem::size_of::<NativeStatus>(), 40);
    }
}

pub(crate) fn set_ui_resources(ui: &NativeImage) -> i32 {
    // 原生桥只在最近成功Prepare的同帧登记，Present后必须失效。
    unsafe { bl5_fsr3_fg_set_ui_resources(ui) } // UI由RenderApp持久持有；SDK游戏队列复制后可异步合成。
} // 独立UI资源登记结束。
