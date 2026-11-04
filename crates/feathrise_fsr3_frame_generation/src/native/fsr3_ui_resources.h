// 独立UI资源只属于当前成功Prepare的真实帧；SDK内部双缓冲负责异步Present期间的像素寿命。
#pragma once // 固定小型状态只在原生桥内定义一次。
#include <ffx_api/ffx_framegeneration.h> // 公开预乘Alpha及SDK内部UI双缓冲标志。
#include <ffx_api/vk/ffx_api_vk.h> // 公开RegisterUiResource配置，不访问SDK私有对象。

class Bl5FsrUiResources { // 调用方在现有生命周期锁内配置；稳定帧只更新固定POD身份。
public: // 不拥有Bevy纹理或Vulkan设备。
    bool ready_for(uint64_t frame) const { return fresh_ && frame_ == frame; } // 旧UI绝不能用于下一个真实帧。
    void invalidate() { fresh_ = false; } // Present完成、失败或新Prepare开始时撤销同帧证据。
    ffxReturnCode_t clear(ffxContext* context, PfnFfxConfigure configure) { // 无独立UI时SDK必须接收空资源，避免把旧HUD叠到完整普通画面。
        fresh_ = false; // 先撤销资格，失败也不复用旧资源。
        if (!registered_) return FFX_API_RETURN_OK; // 稳定关闭期不重复调用SDK。
        if (context == nullptr || *context == nullptr || configure == nullptr) { registered_ = false; return FFX_API_RETURN_OK; } // 整个代理已释放时只清本地状态。
        ffxConfigureDescFrameGenerationSwapChainRegisterUiResourceVK config{}; // 零资源表达正式注销，不伪造透明图像。
        config.header.type = FFX_API_CONFIGURE_DESC_TYPE_FGSWAPCHAIN_REGISTERUIRESOURCE_VK; // Vulkan交换链UI注册入口。
        const ffxReturnCode_t result = configure(context, &config.header); // 注册使用真实代理上下文。
        if (result == FFX_API_RETURN_OK) registered_ = false; // 失败保留注销待办，下一生命周期边界继续清理。
        return result; // 让桥保留真实失败原因。
    } // UI注销结束。
    ffxReturnCode_t publish(ffxContext* context, PfnFfxConfigure configure, const FfxApiResource& ui, VkFormat backbufferFormat, VkExtent2D extent, uint64_t frame) { // 只登记独立UI，场景沿用既有ViewTarget/upscaling/前缓冲链。
        fresh_ = false; // 新注册失败时旧帧即刻失效。
        const uint32_t expected = backbufferFormat == VK_FORMAT_B8G8R8A8_UNORM ? FFX_API_SURFACE_FORMAT_B8G8R8A8_SRGB : backbufferFormat == VK_FORMAT_R8G8B8A8_UNORM ? FFX_API_SURFACE_FORMAT_R8G8B8A8_SRGB : FFX_API_SURFACE_FORMAT_UNKNOWN; // 独立UI与当前SDR窗口保持同通道排列及sRGB读取。
        if (context == nullptr || *context == nullptr || configure == nullptr || expected == FFX_API_SURFACE_FORMAT_UNKNOWN || ui.resource == nullptr || extent.width == 0 || extent.height == 0) return FFX_API_RETURN_ERROR_PARAMETER; // 不接受空UI、无效窗口、HDR猜测或半初始化代理。
        const auto& description = ui.description; // 只核验实际交给SDK读取的透明UI，不为证明场景规格额外分配图像。
        if (description.type != FFX_API_RESOURCE_TYPE_TEXTURE2D || description.format != expected || description.width != extent.width || description.height != extent.height || description.depth != 1 || description.mipCount != 1 || ui.state != FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ) return FFX_API_RETURN_ERROR_PARAMETER; // UI必须与当前窗口格式/尺寸精确匹配，且完成单层单mip读取转换。
        ffxConfigureDescFrameGenerationSwapChainRegisterUiResourceVK config{}; // 每帧注册实际透明UI快照。
        config.header.type = FFX_API_CONFIGURE_DESC_TYPE_FGSWAPCHAIN_REGISTERUIRESOURCE_VK; // 使用SDK正式Vulkan配置。
        config.uiResource = ui; // UI来自独立透明层，不由场景颜色相减推断。
        config.flags = FFX_FRAMEGENERATION_UI_COMPOSITION_FLAG_USE_PREMUL_ALPHA | FFX_FRAMEGENERATION_UI_COMPOSITION_FLAG_ENABLE_INTERNAL_UI_DOUBLE_BUFFERING; // SDK在游戏队列复制UI，避免异步呈现读取下一帧覆盖的纹理。
        const ffxReturnCode_t result = configure(context, &config.header); // 调用成功后Rust才允许把scene-only送到窗口前缓冲。
        if (result == FFX_API_RETURN_OK) { registered_ = true; fresh_ = true; frame_ = frame; } // 完整注册结果与成功Prepare帧身份原子采用。
        return result; // 失败由Rust合回普通完整UI，再关闭生成帧。
    } // 独立UI同帧提交结束。
    void reset() { registered_ = false; fresh_ = false; frame_ = 0; } // 整个代理上下文销毁后清除本地借用身份。
private: // 不跨帧拥有或销毁Bevy资源。
    bool registered_ = false; // SDK仍持有UI注册，需要关闭时明确注销。
    bool fresh_ = false; // 本真实帧已完成透明UI捕获与成功注册。
    uint64_t frame_ = 0; // 与原生成功Prepare后的g_frameId配对。
}; // Bl5FsrUiResources结束。
