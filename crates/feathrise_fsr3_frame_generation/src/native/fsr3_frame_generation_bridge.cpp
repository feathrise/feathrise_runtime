// 本文件是Bevy/wgpu与AMD FidelityFX FSR 3.1 Frame Generation之间唯一原生ABI边界。
// 【功能设计】代理交换链、FG上下文和官方DLL共享VkDevice生命周期；任何失败都撤销插帧事实，
// 由wgpu-hal回退普通Vulkan交换链，禁止向业务层暴露半初始化的厂商对象。

#include <ffx_api/ffx_api.h> // 动态解析的五个稳定C ABI入口。
#include <ffx_api/ffx_api_types.h> // FFX资源、尺寸和返回码。
#include <ffx_api/ffx_framegeneration.h> // FG上下文、配置与Prepare描述。
#include <ffx_api/vk/ffx_api_vk.h> // Vulkan后端和代理交换链描述。

#include <Windows.h> // 绝对路径加载与UTF-16运行库路径。
#include <vulkan/vulkan.h> // C ABI中的原生Vulkan句柄和函数指针。

#include <array> // 四条独立队列和固定错误缓冲。
#include <atomic> // Present线程向Rust发布无锁事实。
#include <cfloat> // 反向无限深度使用FLT_MAX近面约定。
#include <cstdint> // 稳定跨ABI整数宽度。
#include <cstdio> // 有界诊断格式化。
#include <cstring> // 固定相机向量复制。
#include <mutex> // 稀有生命周期变更和错误文本串行化。
#include <string> // 组合已锁定DLL绝对路径。
#include "fsr3_present_submit.h" // 仅修正固定SDK失败Acquire分支的无人消费binary signal。
#include "fsr3_runtime_identity.h" // 固化修订DLL哈希并防止校验与加载之间替换。
#include "fsr3_ui_resources.h" // 独立透明UI与当前成功Prepare帧身份配对。
#include "fsr3_present_images.h" // 保留Bevy的sRGB混合语义，按原始像素复制到SDK的UNORM后缓冲。

namespace { // 全部厂商对象限制在单一翻译单元，避免跨crate所有权漂移。

constexpr uint32_t kQueueCount = 4; // SDK Vulkan交换链实现明确要求四个不同VkQueue句柄。

struct Bl5Fsr3Image { // 与Rust NativeImage逐字段保持repr(C)一致。
    uint64_t image; // VkImage位模式。
    uint32_t format; // VkFormat数值。
    uint32_t width; // 基础级有效宽度。
    uint32_t height; // 基础级有效高度。
    uint32_t depth; // 纹理深度，二维图像固定一。
    uint32_t mipLevels; // 完整mip数量。
    uint32_t arrayLayers; // 数组层数量。
    uint32_t sampleCount; // VkSampleCountFlagBits数值。
    uint32_t usage; // VkImageUsageFlags位集合。
    uint32_t flags; // VkImageCreateFlags位集合。
}; // Bl5Fsr3Image结束。

struct Bl5Fsr3PrepareDesc { // 每个真实渲染帧的同步Prepare快照。
    uint64_t commandBuffer; // 当前仍在记录的VkCommandBuffer。
    Bl5Fsr3Image depth; // 反向Z深度预通道。
    Bl5Fsr3Image motionVectors; // Bevy Rg16Float运动矢量预通道。
    uint32_t renderWidth; // 深度和运动矢量的有效渲染宽度。
    uint32_t renderHeight; // 深度和运动矢量的有效渲染高度。
    float jitterX; // 当前相机像素抖动X。
    float jitterY; // 当前相机像素抖动Y。
    float motionScaleX; // 归一化运动矢量恢复到像素空间的X比例。
    float motionScaleY; // 归一化运动矢量恢复到像素空间的Y比例。
    float frameTimeMs; // 真实渲染帧间隔，毫秒。
    float cameraNear; // 作者透视近裁面。
    float cameraFovY; // 垂直视场角弧度。
    float cameraPosition[3]; // 世界空间相机位置。
    float cameraUp[3]; // 世界空间单位上方向。
    float cameraRight[3]; // 世界空间单位右方向。
    float cameraForward[3]; // 世界空间单位前方向。
    uint8_t requested; // 主世界资格解析后的最终固定2x请求。
    uint8_t reset; // 首帧、瞬移、投影或尺寸断续。
    uint8_t padding[2]; // 保持C/Rust自然对齐一致。
}; // Bl5Fsr3PrepareDesc结束。

struct Bl5Fsr3NativeStatus { // 低频状态快照，不泄露FFX内部类型。
    int32_t lastResult; // 最近一次FFX或Vulkan结果。
    uint32_t availableQueues; // 当前逻辑设备实际取得的角色队列数。
    uint64_t lastPresentCount; // 代理交换链累计Present计数。
    uint32_t lastPresentDelta; // 最近完整统计窗口确认的近似呈现倍率。
    uint32_t displayWidth; // 当前代理交换链宽度。
    uint32_t displayHeight; // 当前代理交换链高度。
    uint8_t runtimeLoaded; // 官方DLL已验签、加载且五个导出齐全。
    uint8_t attached; // 设备与四条独立队列已附着。
    uint8_t swapchainReady; // 当前窗口由FFX代理交换链拥有。
    uint8_t contextReady; // FG核心上下文已创建。
    uint8_t requested; // 最近Prepare收到的业务请求。
    uint8_t effective; // 连续120宿主帧窗口具有生成和近似2x呈现证据。
    uint8_t padding[2]; // 固定结构大小。
}; // Bl5Fsr3NativeStatus结束。

static_assert(sizeof(Bl5Fsr3Image) == 48, "Bl5Fsr3Image ABI changed"); // Rust NativeImage固定48字节。
static_assert(sizeof(Bl5Fsr3PrepareDesc) == 192, "Bl5Fsr3PrepareDesc ABI changed"); // Rust逐帧描述固定192字节。
static_assert(sizeof(Bl5Fsr3NativeStatus) == 40, "Bl5Fsr3NativeStatus ABI changed"); // Rust状态快照固定40字节。

HMODULE g_fidelityfxModule = nullptr; // 必须活到VkDevice销毁前最后一次代理调用完成。
PfnFfxCreateContext g_createContext = nullptr; // 动态C ABI创建入口。
PfnFfxConfigure g_configure = nullptr; // 动态配置入口。
PfnFfxQuery g_query = nullptr; // 动态查询代理函数入口。
PfnFfxDispatch g_dispatch = nullptr; // Prepare、生成回调和等待Present入口。
PfnFfxDestroyContext g_destroyContext = nullptr; // 动态销毁入口。
PFN_vkGetDeviceProcAddr g_vkGetDeviceProcAddr = nullptr; // FFX Vulkan后端解析设备函数。
PFN_vkGetPhysicalDeviceMemoryProperties g_vkGetMemoryProperties = nullptr; // 只在交换链创建时查询前缓冲内存类型。
PFN_vkGetPhysicalDeviceSurfaceFormatsKHR g_vkGetSurfaceFormats = nullptr; // 校验SDK实际使用的UNORM与颜色空间配对。
PFN_vkGetPhysicalDeviceImageFormatProperties g_vkGetImageProperties = nullptr; // 校验SDK假后缓冲所需的采样、存储和附件用途。
PFN_vkQueueSubmit g_vkQueueSubmit = nullptr; // SDK公开Present队列回调转发到同一Vulkan设备入口。
Bl5FsrPresentImages g_presentImages; // 与当前代理交换链同寿命的有界sRGB前缓冲。

Bl5FsrUiResources g_uiResources; // 只保留SDK公开UI注册状态，不拥有Rust纹理。
ffxContext g_swapchainContext = nullptr; // 拥有代理交换链实现和替换函数。
ffxContext g_frameGenerationContext = nullptr; // 拥有Prepare与生成计算资源。
ffxQueryDescSwapchainReplacementFunctionsVK g_replacements{}; // 首次查询后直到DLL卸载都稳定。
VkPhysicalDevice g_physicalDevice = VK_NULL_HANDLE; // 不取得宿主句柄所有权。
VkDevice g_device = VK_NULL_HANDLE; // 不取得宿主句柄所有权。
std::array<VkQueue, kQueueCount> g_queues{}; // game/compute/present/acquire严格不同。
std::array<uint32_t, kQueueCount> g_queueFamilies{}; // 角色可跨族，present固定使用已知可呈现的主图形族。
VkSwapchainKHR g_swapchain = VK_NULL_HANDLE; // 当前FFX代理句柄。
VkExtent2D g_displayExtent{}; // FG上下文按交换链重建。
VkFormat g_backBufferFormat = VK_FORMAT_UNDEFINED; // 当前交换链格式。
bool g_warmupComplete = false; // 首帧只准备历史，不生成未知中间帧。
uint64_t g_frameId = 0; // 只在成功Prepare后加一，满足SDK精确+1契约。
uint64_t g_presentStartCount = 0; // 当前120个宿主Present观测窗口起点；SDK的累计数由异步线程更新。
uint64_t g_generationStartCount = 0; // 观测窗口的成功生成张数起点。
uint32_t g_measurementFrames = 0; // 固定120帧窗口，避免一次异步计数跳动被误称1x/2x。
bool g_measurementActive = false; // reset、关闭和错误立即撤销观测窗口。
bool g_measurementInitialized = false; // 连续窗口共用端点，禁止遗漏两个宿主调用之间发生的异步Present。
std::atomic<uint64_t> g_generatedCount{0}; // 成功回调产生的张数，必须结合真实Present累计量才可判定2x。

std::atomic<int32_t> g_lastResult{FFX_API_RETURN_ERROR}; // 默认明确未初始化。
std::atomic<uint32_t> g_availableQueues{0}; // 队列不足时向设置页提供结构化证据。
std::atomic<uint64_t> g_lastPresentCount{0}; // 最近代理累计计数。
std::atomic<uint32_t> g_lastPresentDelta{1}; // 普通宿主Present基线为一。
std::atomic<uint32_t> g_displayWidth{0}; // 状态线程无锁读取的交换链宽度副本。
std::atomic<uint32_t> g_displayHeight{0}; // 状态线程无锁读取的交换链高度副本。
std::atomic<bool> g_runtimeLoaded{false}; // DLL身份与导出验证事实。
std::atomic<bool> g_attached{false}; // 设备附着事实。
std::atomic<bool> g_swapchainReady{false}; // 当前代理交换链事实。
std::atomic<bool> g_contextReady{false}; // 当前FG核心上下文事实。
std::atomic<bool> g_requested{false}; // 业务请求事实。
std::atomic<bool> g_effective{false}; // 真实Present倍数事实。
std::atomic<bool> g_tracedGetImages{false}; // 调试构建只记录第一次图像枚举边界，避免稳定帧输出噪声。
std::atomic<bool> g_tracedAcquire{false}; // 调试构建只记录第一次Acquire边界。
std::atomic<bool> g_tracedPresent{false}; // 调试构建只记录第一次Present边界。
std::atomic<bool> g_tracedPrepare{false}; // 调试构建只记录第一次完整Prepare。
std::atomic<bool> g_tracedUi{false}; // 首次实际登记独立UI时留下证据，稳定帧零额外日志。
std::atomic<bool> g_tracedGeneration{false}; // 调试构建只记录第一次真实生成回调。
std::array<char, 1024> g_error{}; // 固定诊断缓冲避免返回悬垂字符串。
std::mutex g_errorMutex; // 只保护稀有错误文本。
std::mutex g_lifecycleMutex; // 保护创建、重建、Prepare和销毁的上下文身份。

void trace_native_boundary(const char* stage) { // 调试构建把原生空调用缩小到最后一个已进入的SDK边界。
#ifndef NDEBUG // 正式发行不产生额外控制台I/O或字符串成本。
    std::fprintf(stderr, "[fsr3-fg-native] %s\n", stage); // stderr即使Rust日志尚未初始化也可见。
    std::fflush(stderr); // 立即落地，确保访问冲突不会吞掉最后一个阶段标记。
#else // release构建保持零行为。
    (void)stage; // 消除未使用参数警告。
#endif // 调试边界跟踪结束。
} // trace_native_boundary结束。
VkResult submit_present_queue(uint32_t count, const VkSubmitInfo* submits, VkFence fence) { // 固定game合成模式的公开SDK提交回调。
    bool filtered = false; // 只有精确识别的Acquire失败提交才去掉无消费者binary signal。
    const VkResult result = bl5_fsr3_present_submit::submit(g_vkQueueSubmit, g_queues[2], count, submits, fence, &filtered); // 正常命令、全部wait与timeline进度保持原语义。
    if (filtered) trace_native_boundary("failed acquire: preserved timeline completion without binary present signal"); // 稀有窗口失效事件留下实际命中证据，稳定帧零日志。
    return result; // Vulkan失败原样上报给SDK恢复流程。
} // submit_present_queue结束。

void set_error(const char* stage, int64_t result) { // 统一发布原生阶段与精确结果。
    g_lastResult.store(static_cast<int32_t>(result), std::memory_order_release); // Rust先读取结构化结果。
    g_effective.store(false, std::memory_order_release); // 任何错误都撤销旧2x证据。
#ifndef NDEBUG // 调试运行直接保留SDK失败边界，访问冲突后无需依赖下一帧Rust状态轮询。
    std::fprintf(stderr, "[fsr3-fg-native] error: %s (result=%lld)\n", stage, static_cast<long long>(result)); // 稀有失败允许同步诊断I/O。
    std::fflush(stderr); // 保证紧随其后的厂商崩溃不会吞掉错误证据。
#endif // 正式发行仍只通过固定状态缓冲暴露错误。
    const std::scoped_lock lock(g_errorMutex); // 失败路径允许一次短锁。
    std::snprintf(g_error.data(), g_error.size(), "%s (result=%lld)", stage, static_cast<long long>(result)); // 自动截断并NUL结尾。
} // set_error结束。

void clear_error() { // 成功生命周期边界清理过期诊断。
    const std::scoped_lock lock(g_errorMutex); // 与Rust复制互斥。
    g_error[0] = '\0'; // 固定缓冲无需重新分配。
} // clear_error结束。


template <typename T> bool load_export(T& target, const char* name) { // 从已验签模块解析稳定导出。
    target = reinterpret_cast<T>(GetProcAddress(g_fidelityfxModule, name)); // 不经过外部搜索路径。
    if (target != nullptr) { // 指针有效即可继续。
        return true; // 单一导出成功。
    } // 导出判断结束。
    set_error(name, GetLastError()); // ABI不完整时保留Win32诊断。
    return false; // 禁止调用空入口。
} // load_export结束。

ffxApiHeader make_header(uint64_t type, ffxApiHeader* next = nullptr) { // 所有动态描述显式设置类型链。
    ffxApiHeader header{}; // 先清零保留字段。
    header.type = type; // 使用当前SDK头编译出的准确类型值。
    header.pNext = next; // 链接可选后端或相机描述。
    return header; // 返回独立POD值。
} // make_header结束。

VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL get_ffx_device_proc_addr(VkDevice device, const char* name) { // 统一适配FFX 1.1.4仍按KHR名称查询的Vulkan核心入口。
    if (g_vkGetDeviceProcAddr == nullptr || name == nullptr) { // SDK不得借由残缺宿主解析器进入空调用。
        return nullptr; // 调用方的创建前预检会把缺失入口转成安全回退。
    } // 解析器存在性判断结束。
    PFN_vkVoidFunction function = g_vkGetDeviceProcAddr(device, name); // 其他入口严格保留wgpu设备实际暴露结果。
    if (function == nullptr && std::strcmp(name, "vkGetBufferMemoryRequirements2KHR") == 0) { // Vulkan 1.1已把KHR入口提升为同ABI核心命令。
        function = g_vkGetDeviceProcAddr(device, "vkGetBufferMemoryRequirements2"); // wgpu可只启用核心版本而不启用KHR扩展名。
    } // 唯一兼容别名结束；不伪造任何未启用扩展能力。
    return function; // FFX保存并调用该准确设备入口。
} // get_ffx_device_proc_addr结束。

FfxApiResource make_resource(const Bl5Fsr3Image& image, uint32_t state) { // 把wgpu图像描述转换为FFX值。
    if (image.image == 0 || image.width == 0 || image.height == 0) { // 残缺输入返回官方空资源。
        return FfxApiResource{}; // 不构造伪句柄。
    } // 输入存在性判断结束。
    VkImageCreateInfo createInfo{}; // helper只读取格式、范围和用途。
    createInfo.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO; // 完整Vulkan描述身份。
    createInfo.flags = static_cast<VkImageCreateFlags>(image.flags); // 保留可变格式等事实。
    createInfo.imageType = VK_IMAGE_TYPE_2D; // 首期只接受二维玩法窗口输入。
    createInfo.format = static_cast<VkFormat>(image.format); // 真实底层格式。
    createInfo.extent = {image.width, image.height, image.depth == 0 ? 1u : image.depth}; // 有限范围。
    createInfo.mipLevels = image.mipLevels == 0 ? 1u : image.mipLevels; // 当前均为一。
    createInfo.arrayLayers = image.arrayLayers == 0 ? 1u : image.arrayLayers; // 当前均为一。
    createInfo.samples = static_cast<VkSampleCountFlagBits>(image.sampleCount == 0 ? VK_SAMPLE_COUNT_1_BIT : image.sampleCount); // 租约保证单采样。
    createInfo.tiling = VK_IMAGE_TILING_OPTIMAL; // wgpu GPU纹理。
    createInfo.usage = static_cast<VkImageUsageFlags>(image.usage); // 精确复制/采样/附件用途。
    const VkImage rawImage = reinterpret_cast<VkImage>(static_cast<uintptr_t>(image.image)); // 只借用原生句柄。
    return ffxApiGetResourceVK(reinterpret_cast<void*>(rawImage), ffxApiGetImageResourceDescriptionVK(rawImage, createInfo, 0), state); // FFX不取得所有权。
} // make_resource结束。

ffxReturnCode_t dispatch_frame_generation(ffxDispatchDescFrameGeneration* params, void*) { // 代理Present请求同步录制一张中间帧。
    if (params == nullptr || g_dispatch == nullptr || g_frameGenerationContext == nullptr) { // 生命周期缺失时拒绝回调。
        return FFX_API_RETURN_ERROR_PARAMETER; // 代理安全回到原始帧。
    } // 回调前置条件结束。
    const ffxReturnCode_t result = g_dispatch(&g_frameGenerationContext, &params->header); // 使用SDK提供的命令列表和输出目标。
    if (result == FFX_API_RETURN_OK) { // SDK可以成功返回但选择不生成，不能只数回调次数。
        g_generatedCount.fetch_add(params->numGeneratedFrames, std::memory_order_relaxed); // 只计本次SDK确认的输出张数。
    } else { // 生成回调的失败必须立即向主世界公开。
        set_error("ffxDispatch(frame generation callback)", result); // 不把失败隐藏在成功Prepare后面。
    } // 回调结果统计结束。
    if (result == FFX_API_RETURN_OK && !g_tracedGeneration.exchange(true, std::memory_order_acq_rel)) { // 首次成功即可证明Present进入了生成回调。
        trace_native_boundary("frame generation: first dispatch callback succeeded"); // 与仅创建上下文严格区分。
    } // 首次生成回调跟踪结束。
    return result; // 代理根据SDK结果决定是否呈现生成帧。
} // dispatch_frame_generation结束。

void wait_for_presents_locked() { // 销毁/重建前等待代理Present工作完整退出。
    if (g_swapchainContext == nullptr || g_swapchain == VK_NULL_HANDLE || g_dispatch == nullptr) { // Surface已先销毁时，上下文仍为下次重建保留，但其内部已没有可等待的交换链。
        return; // 幂等退出。
    } // 代理交换链所有权判断结束。
    ffxDispatchDescFrameGenerationSwapChainWaitForPresentsVK wait{}; // 官方Vulkan等待描述。
    wait.header = make_header(FFX_API_DISPATCH_DESC_TYPE_FGSWAPCHAIN_WAIT_FOR_PRESENTS_VK); // 准确类型。
    const ffxReturnCode_t result = g_dispatch(&g_swapchainContext, &wait.header); // 只在生命周期边界阻塞。
    if (result != FFX_API_RETURN_OK) { // 退出仍记录失败但继续尽力清理。
        set_error("ffxDispatch(wait for presents)", result); // 不让错误中断资源释放。
    } // 等待结果结束。
} // wait_for_presents_locked结束。

void disable_frame_generation_locked() { // 代理仍有效时建立明确关闭配置。
    const ffxReturnCode_t uiResult = g_uiResources.clear(&g_swapchainContext, g_configure); // 普通完整输出不能再次叠加上一帧UI。
    if (uiResult != FFX_API_RETURN_OK) set_error("ffxConfigure(clear independent UI)", uiResult); // 保留注销失败的真实诊断。
    g_requested.store(false, std::memory_order_release); // 先撤销业务请求事实。
    g_effective.store(false, std::memory_order_release); // 立即撤销旧倍率证据。
    g_warmupComplete = false; // 恢复后至少重新积累一帧。
    g_measurementActive = false; // 旧稳定窗口不可跨关闭或输入中断复用。
    g_measurementInitialized = false; // 只在真实关闭边界重新取累计起点。
    g_measurementFrames = 0; // 恢复后重新收集完整窗口。
    g_lastPresentDelta.store(1, std::memory_order_release); // 立即撤销旧近似倍率。
    if (g_frameGenerationContext == nullptr || g_configure == nullptr) { // 无核心上下文无需调用。
        return; // 幂等关闭。
    } // 核心存在性判断结束。
    ffxConfigureDescFrameGeneration config{}; // 每次提交完整关闭快照。
    config.header = make_header(FFX_API_CONFIGURE_DESC_TYPE_FRAMEGENERATION); // 描述身份。
    config.swapChain = reinterpret_cast<void*>(g_swapchain); // 当前代理句柄。
    config.frameGenerationCallback = dispatch_frame_generation; // 保留稳定回调但关闭开关。
    config.frameGenerationCallbackUserContext = nullptr; // 回调使用静态上下文。
    config.frameGenerationEnabled = false; // 关键关闭位。
    config.allowAsyncWorkloads = false; // 首版同步模式固定不变。
    config.frameID = g_frameId; // 保持最近成功帧身份。
    const ffxReturnCode_t result = g_configure(&g_frameGenerationContext, &config.header); // 在销毁前停止新生成工作。
    if (result != FFX_API_RETURN_OK) { // 仍继续销毁以免泄漏。
        set_error("ffxConfigure(frame generation off)", result); // 保留诊断。
    } // 关闭配置结果结束。
} // disable_frame_generation_locked结束。

void destroy_core_locked() { // 核心上下文只和当前交换链规格同寿命。
    disable_frame_generation_locked(); // 先禁止Present发起新生成任务。
    wait_for_presents_locked(); // 官方要求在销毁FG上下文前等待已排队的生成/Present回调退出。
    if (g_frameGenerationContext != nullptr && g_destroyContext != nullptr) { // 只销毁已创建上下文。
        const ffxReturnCode_t result = g_destroyContext(&g_frameGenerationContext, nullptr); // SDK清空句柄。
        if (result != FFX_API_RETURN_OK) { // 退出路径记录但不重复销毁。
            set_error("ffxDestroyContext(frame generation)", result); // 发布官方错误。
        } // 销毁结果结束。
        g_frameGenerationContext = nullptr; // 即使SDK失败也禁止继续借用旧对象。
    } // 核心上下文销毁结束。
    g_contextReady.store(false, std::memory_order_release); // 对外撤销就绪事实。
} // destroy_core_locked结束。

bool create_core_locked(const VkSwapchainCreateInfoKHR& swapchainInfo) { // 为当前显示尺寸创建同步FG上下文。
    trace_native_boundary("create FG core: begin"); // 区分交换链代理成功后进入核心创建。
    destroy_core_locked(); // 规格重建绝不复用旧内部资源。
    ffxCreateBackendVKDesc backend{}; // 当前wgpu Vulkan设备后端描述。
    backend.header = make_header(FFX_API_CREATE_CONTEXT_DESC_TYPE_BACKEND_VK); // 后端类型。
    backend.vkDevice = g_device; // 借用宿主逻辑设备。
    backend.vkPhysicalDevice = g_physicalDevice; // 借用匹配物理设备。
    backend.vkDeviceProcAddr = get_ffx_device_proc_addr; // 使用宿主loader，并适配已提升为核心命令的KHR别名。
    if (get_ffx_device_proc_addr(g_device, "vkGetBufferMemoryRequirements2KHR") == nullptr) { // 官方后端会无条件调用该入口且自身不判空。
        set_error("missing vkGetBufferMemoryRequirements2/KHR", VK_ERROR_EXTENSION_NOT_PRESENT); // 两种标准名称都缺失时禁止进入厂商空调用。
        return false; // 代理交换链仍以普通Present安全运行。
    } // FFX必需Vulkan入口预检结束。

    ffxCreateContextDescFrameGeneration create{}; // 固定2x核心描述。
    create.header = make_header(FFX_API_CREATE_CONTEXT_DESC_TYPE_FRAMEGENERATION, &backend.header); // 完整链首。
    create.flags = FFX_FRAMEGENERATION_ENABLE_DEPTH_INVERTED | FFX_FRAMEGENERATION_ENABLE_DEPTH_INFINITE; // 交换链颜色已完成色调映射，只有深度使用反向无限Z。
    create.displaySize = {swapchainInfo.imageExtent.width, swapchainInfo.imageExtent.height}; // 生成输出尺寸。
    create.maxRenderSize = create.displaySize; // 支持原生、DLSS或FSR任意不超过显示尺寸的输入。
    create.backBufferFormat = ffxApiGetSurfaceFormatVK(swapchainInfo.imageFormat); // 代理交换链真实格式。
    if (create.displaySize.width == 0 || create.displaySize.height == 0 || create.backBufferFormat == FFX_API_SURFACE_FORMAT_UNKNOWN) { // 最小化或未知格式回退。
        set_error("unsupported FSR frame-generation swapchain format/extent", FFX_API_RETURN_ERROR_PARAMETER); // 结构化阻断。
        return false; // 代理仍可透传普通Present。
    } // 规格判断结束。
    trace_native_boundary("create FG core: before ffxCreateContext"); // 空调用若发生在SDK内部，日志会停在此处。
    const ffxReturnCode_t result = g_createContext(&g_frameGenerationContext, &create.header, nullptr); // 同步创建GPU资源。
    trace_native_boundary("create FG core: after ffxCreateContext"); // 证明SDK核心构造已经安全返回。
    if (result != FFX_API_RETURN_OK || g_frameGenerationContext == nullptr) { // 半创建上下文必须立即清理。
        set_error("ffxCreateContext(frame generation)", result); // 发布精确阶段。
        if (g_frameGenerationContext != nullptr) { // SDK若返回句柄仍尝试销毁。
            g_destroyContext(&g_frameGenerationContext, nullptr); // 不覆盖主失败结果。
            g_frameGenerationContext = nullptr; // 清除悬垂身份。
        } // 半创建清理结束。
        return false; // 普通Present继续。
    } // 创建结果结束。
    g_frameId = 0; // 新上下文从连续零号帧开始。
    g_warmupComplete = false; // 首帧只准备时间历史。
    g_contextReady.store(true, std::memory_order_release); // 对外发布核心就绪。
    trace_native_boundary("create FG core: before initial disable configure"); // 首次关闭配置也是独立外部调用边界。
    disable_frame_generation_locked(); // 首次Present前建立明确关闭态。
    trace_native_boundary("create FG core: complete"); // 核心与初始关闭态均已建立。
    clear_error(); // 清除旧规格阻断。
    g_lastResult.store(FFX_API_RETURN_OK, std::memory_order_release); // 发布成功。
    return true; // 核心可接收Prepare。
} // create_core_locked结束。

bool replacement_functions_valid() { // 防止查询成功却返回残缺ABI。
    return g_replacements.pOutCreateSwapchainFFXAPI != nullptr &&
           g_replacements.pOutDestroySwapchainFFXAPI != nullptr &&
           g_replacements.pOutGetSwapchainImagesKHR != nullptr &&
           g_replacements.pOutAcquireNextImageKHR != nullptr &&
           g_replacements.pOutQueuePresentKHR != nullptr &&
           g_replacements.pOutGetLastPresentCountFFXAPI != nullptr; // HDR元数据函数允许设备不支持扩展。
} // replacement_functions_valid结束。

VkResult validate_sdk_swapchain_format(const VkSwapchainCreateInfoKHR& info) { // 修改宿主格式后必须重新验证目标Surface与GPU能力。
    std::array<VkSurfaceFormatKHR, 32> formats{}; // 固定上限，只在交换链创建期查询。
    uint32_t count = 0; // 先询问驱动实际返回多少种组合。
    VkResult result = g_vkGetSurfaceFormats(g_physicalDevice, info.surface, &count, nullptr); // 原sRGB可用不代表对应UNORM也可用。
    if (result != VK_SUCCESS) return result; // Surface丢失等错误直接回退。
    if (count == 0 || count > formats.size()) return VK_ERROR_FORMAT_NOT_SUPPORTED; // 拒绝无支持或超出固定枚举预算的Surface。
    result = g_vkGetSurfaceFormats(g_physicalDevice, info.surface, &count, formats.data()); // 查询完整格式和颜色空间配对。
    if (result != VK_SUCCESS) return result; // 枚举期间Surface变化不能采用部分结果。
    bool supported = false; // 不按厂商品牌猜支持能力。
    for (uint32_t index = 0; index < count; ++index) { // 最多32个常量大小条目。
        supported |= formats[index].colorSpace == info.imageColorSpace && (formats[index].format == info.imageFormat || formats[index].format == VK_FORMAT_UNDEFINED); // Vulkan的UNDEFINED格式表示Surface允许任意格式。
    } // Surface组合核验结束。
    if (!supported) return VK_ERROR_FORMAT_NOT_SUPPORTED; // 不把错误UNORM描述传入SDK内部创建。
    VkImageFormatProperties properties{}; // SDK的假后缓冲是普通图像，必须另查完整用途。
    const VkImageUsageFlags usage = info.imageUsage | VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_STORAGE_BIT | VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT; // 对齐SDK 1.1.4的实际创建组合。
    result = g_vkGetImageProperties(g_physicalDevice, info.imageFormat, VK_IMAGE_TYPE_2D, VK_IMAGE_TILING_OPTIMAL, usage, 0, &properties); // SDK假图像不继承MUTABLE/EXTENDED_USAGE标志。
    if (result != VK_SUCCESS) return result; // 格式用途组合不可用时保持原生Present。
    if (info.imageExtent.width > properties.maxExtent.width || info.imageExtent.height > properties.maxExtent.height || properties.maxArrayLayers < 1 || properties.maxMipLevels < 1 || (properties.sampleCounts & VK_SAMPLE_COUNT_1_BIT) == 0) return VK_ERROR_FORMAT_NOT_SUPPORTED; // 所需范围与采样数量也必须真实支持。
    return VK_SUCCESS; // 只有完整格式能力通过才允许进入SDK。
} // validate_sdk_swapchain_format结束。

void destroy_swapchain_context_locked() { // VkDevice仍有效时释放全部FFX设备对象。
    destroy_core_locked(); // 内部先关闭、等待Present，再释放生成计算上下文。
    g_presentImages.destroy(); // 宿主已在释放Surface前停止SDK并等待设备空闲；创建失败则尚无复制提交。
    if (g_swapchain != VK_NULL_HANDLE && g_replacements.pOutDestroySwapchainFFXAPI != nullptr && g_device != VK_NULL_HANDLE && g_swapchainContext != nullptr) { // 当前代理句柄仍归FFX所有。
        g_replacements.pOutDestroySwapchainFFXAPI(g_device, g_swapchain, nullptr, g_swapchainContext); // 通过同一上下文销毁内部真实交换链。
        g_swapchain = VK_NULL_HANDLE; // 防止上下文销毁重复借用。
    } // 代理句柄销毁结束。
    if (g_swapchainContext != nullptr && g_destroyContext != nullptr) { // 最后释放代理上下文。
        const ffxReturnCode_t result = g_destroyContext(&g_swapchainContext, nullptr); // SDK清理队列同步对象。
        if (result != FFX_API_RETURN_OK) { // 继续归零本地状态。
            set_error("ffxDestroyContext(frame-generation swapchain)", result); // 发布诊断。
        } // 代理上下文销毁结果结束。
        g_swapchainContext = nullptr; // 不再允许替换函数取得旧上下文。
    } // 代理上下文销毁结束。
    g_uiResources.reset(); // SDK上下文已释放，清除独立UI注册借用。
    g_replacements = {}; // DLL卸载前撤销全部函数指针。
    g_swapchainReady.store(false, std::memory_order_release); // 对外发布普通Present事实。
    g_displayExtent = {}; // 清除旧尺寸诊断。
    g_displayWidth.store(0, std::memory_order_release); // 无锁状态副本同步失效。
    g_displayHeight.store(0, std::memory_order_release); // 无锁状态副本同步失效。
    g_backBufferFormat = VK_FORMAT_UNDEFINED; // 清除旧格式。
} // destroy_swapchain_context_locked结束。

} // 匿名命名空间结束。

extern "C" int32_t bl5_fsr3_fg_bootstrap(const wchar_t* runtimeDirectory) { // 必须在DefaultPlugins和任何Vulkan调用前执行。
    if (g_runtimeLoaded.load(std::memory_order_acquire)) { // 重复安装插件保持幂等。
        return FFX_API_RETURN_OK; // 不重复加载同一模块。
    } // 重复初始化判断结束。
    if (runtimeDirectory == nullptr || runtimeDirectory[0] == L'\0') { // 私有目录不可缺失。
        set_error("missing FidelityFX runtime directory", FFX_API_RETURN_ERROR_PARAMETER); // 明确启动阻断。
        return g_lastResult.load(std::memory_order_acquire); // 返回结构化结果。
    } // 路径判断结束。
    const std::wstring dllPath = std::wstring(runtimeDirectory) + L"\\amd_fidelityfx_vk.dll"; // 不允许系统PATH或工作目录搜索。
    Bl5VerifiedRuntimeFile verifiedFile; // 源码修订产物独立命名身份，不能冒称AMD签名原件。
    const DWORD trustResult = verifiedFile.verify(dllPath.c_str()); // 与build.rs固化SHA完全一致，守卫一直持有到加载结束。
    if (trustResult != ERROR_SUCCESS) { // DLL字节必须与当前可执行文件绑定的修订产物一致。
        set_error("FidelityFX patched runtime SHA-256 verification failed", trustResult); // 禁止加载身份不明模块。
        return g_lastResult.load(std::memory_order_acquire); // 普通Vulkan仍可启动。
    } // 修订产物固定字节身份判断结束。
    g_fidelityfxModule = LoadLibraryExW(dllPath.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32); // 依赖只从私有目录和System32解析。
    if (g_fidelityfxModule == nullptr) { // 加载失败记录Win32错误。
        set_error("LoadLibraryExW(amd_fidelityfx_vk.dll)", GetLastError()); // 便于定位依赖问题。
        return g_lastResult.load(std::memory_order_acquire); // 不解析空模块。
    } // 模块加载结束。
    if (!load_export(g_createContext, "ffxCreateContext") || !load_export(g_configure, "ffxConfigure") || !load_export(g_query, "ffxQuery") || !load_export(g_dispatch, "ffxDispatch") || !load_export(g_destroyContext, "ffxDestroyContext")) { // 五个稳定导出缺一不可。
        FreeLibrary(g_fidelityfxModule); // 未向Vulkan登记任何指针，当前可安全卸载。
        g_fidelityfxModule = nullptr; // 清除模块身份。
        return g_lastResult.load(std::memory_order_acquire); // 保留缺失导出诊断。
    } // 导出解析结束。
    HMODULE vulkanModule = GetModuleHandleW(L"vulkan-1.dll"); // 启动早期可能尚未由ash加载。
    if (vulkanModule == nullptr) { // 只从System32/已配置Vulkan runtime加载官方loader。
        vulkanModule = LoadLibraryExW(L"vulkan-1.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS); // 不改变ash后续入口身份。
    } // Vulkan loader取得结束。
    if (vulkanModule == nullptr) { // Vulkan本身不可用时正常渲染也无法继续。
        set_error("LoadLibraryExW(vulkan-1.dll)", GetLastError()); // 发布启动错误。
        FreeLibrary(g_fidelityfxModule); // 清理FFX模块。
        g_fidelityfxModule = nullptr; // 清除模块身份。
        return g_lastResult.load(std::memory_order_acquire); // 返回失败。
    } // Vulkan模块判断结束。
    g_vkGetDeviceProcAddr = reinterpret_cast<PFN_vkGetDeviceProcAddr>(GetProcAddress(vulkanModule, "vkGetDeviceProcAddr")); // FFX后端必需入口。
    g_vkGetMemoryProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceMemoryProperties>(GetProcAddress(vulkanModule, "vkGetPhysicalDeviceMemoryProperties")); // 与ash使用相同官方loader。
    g_vkGetSurfaceFormats = reinterpret_cast<PFN_vkGetPhysicalDeviceSurfaceFormatsKHR>(GetProcAddress(vulkanModule, "vkGetPhysicalDeviceSurfaceFormatsKHR")); // 真实窗口支持范围查询。
    g_vkGetImageProperties = reinterpret_cast<PFN_vkGetPhysicalDeviceImageFormatProperties>(GetProcAddress(vulkanModule, "vkGetPhysicalDeviceImageFormatProperties")); // 假后缓冲完整用途查询。
    if (g_vkGetDeviceProcAddr == nullptr || g_vkGetMemoryProperties == nullptr || g_vkGetSurfaceFormats == nullptr || g_vkGetImageProperties == nullptr) { // 缺任何预检入口均不得创建半套代理。
        set_error("vkGetDeviceProcAddr", GetLastError()); // 发布诊断。
        FreeLibrary(g_fidelityfxModule); // 当前无外部FFX指针使用。
        g_fidelityfxModule = nullptr; // 清除模块身份。
        return g_lastResult.load(std::memory_order_acquire); // 返回失败。
    } // Vulkan入口判断结束。
    g_runtimeLoaded.store(true, std::memory_order_release); // 对外发布完整启动身份。
    g_lastResult.store(FFX_API_RETURN_OK, std::memory_order_release); // 清理未初始化结果。
    clear_error(); // 清空旧诊断。
    return FFX_API_RETURN_OK; // 允许Rust请求额外队列。
} // bl5_fsr3_fg_bootstrap结束。

extern "C" int32_t bl5_fsr3_fg_attach_vulkan(uint64_t physicalDevice, uint64_t device, const uint64_t* queues, const uint32_t* queueFamilies, uint32_t queueCount) { // RenderPlugin finish阶段附着wgpu设备。
    if (!g_runtimeLoaded.load(std::memory_order_acquire) || physicalDevice == 0 || device == 0 || queues == nullptr || queueFamilies == nullptr) { // 生命周期和句柄必须完整。
        set_error("invalid FidelityFX Vulkan attachment", FFX_API_RETURN_ERROR_PARAMETER); // 普通交换链继续。
        return g_lastResult.load(std::memory_order_acquire); // 返回阻断。
    } // 参数存在性结束。
    g_availableQueues.store(queueCount, std::memory_order_release); // 即使不足也发布实际数量。
    if (queueCount < kQueueCount) { // 官方同步模式仍要求四个不同句柄。
        set_error("FSR 3 frame generation requires four Vulkan queues", FFX_API_RETURN_ERROR_RUNTIME_ERROR); // 结构化队列阻断。
        return g_lastResult.load(std::memory_order_acquire); // 不登记代理可用。
    } // 队列数量判断结束。
    std::array<VkQueue, kQueueCount> parsed{}; // 先在局部验证再原子采用。
    std::array<uint32_t, kQueueCount> parsedFamilies{}; // 与角色句柄同步采用。
    for (uint32_t index = 0; index < kQueueCount; ++index) { // 只消费game/compute/present/acquire四个已排序角色。
        parsed[index] = reinterpret_cast<VkQueue>(static_cast<uintptr_t>(queues[index])); // 借用原始句柄。
        parsedFamilies[index] = queueFamilies[index]; // SDK需要每个句柄自己的真实族号。
        if (parsed[index] == VK_NULL_HANDLE) { // 空队列无效。
            set_error("FSR 3 frame generation received a null Vulkan queue", FFX_API_RETURN_ERROR_PARAMETER); // 明确阻断。
            return g_lastResult.load(std::memory_order_acquire); // 不写全局身份。
        } // 单队列判断结束。
        for (uint32_t prior = 0; prior < index; ++prior) { // SDK实现会拒绝任意重复句柄，宿主提前解释。
            if (parsed[index] == parsed[prior]) { // 同一队列不能承担两个角色。
                set_error("FSR 3 frame generation requires distinct Vulkan queue handles", FFX_API_RETURN_ERROR_PARAMETER); // 明确阻断。
                return g_lastResult.load(std::memory_order_acquire); // 普通交换链继续。
            } // 重复判断结束。
        } // 已见队列遍历结束。
    } // 四队列验证结束。
    const std::scoped_lock lock(g_lifecycleMutex); // 采用完整设备身份。
    g_physicalDevice = reinterpret_cast<VkPhysicalDevice>(static_cast<uintptr_t>(physicalDevice)); // 借用物理设备。
    g_device = reinterpret_cast<VkDevice>(static_cast<uintptr_t>(device)); // 借用逻辑设备。
    g_vkQueueSubmit = reinterpret_cast<PFN_vkQueueSubmit>(g_vkGetDeviceProcAddr(g_device, "vkQueueSubmit")); // 设备附着时缓存核心入口，回调不逐帧解析函数。
    if (g_vkQueueSubmit == nullptr) { set_error("missing Vulkan queue submit", VK_ERROR_INITIALIZATION_FAILED); return VK_ERROR_INITIALIZATION_FAILED; } // 不向SDK登记无法转发的回调。
    g_queues = parsed; // 一次采用验证后的句柄。
    g_queueFamilies = parsedFamilies; // 与角色顺序一并采用。
    g_attached.store(true, std::memory_order_release); // 之后provider is_available可返回真。
    g_lastResult.store(FFX_API_RETURN_OK, std::memory_order_release); // 清理旧阻断。
    clear_error(); // 清理旧文本。
    return FFX_API_RETURN_OK; // 允许创建代理交换链。
} // bl5_fsr3_fg_attach_vulkan结束。

extern "C" uint8_t bl5_fsr3_fg_provider_is_available() { // wgpu配置Surface前查询一次。
    return g_attached.load(std::memory_order_acquire) ? 1 : 0; // 只有完整四队列设备可接管。
} // bl5_fsr3_fg_provider_is_available结束。

extern "C" VkResult bl5_fsr3_fg_create_swapchain(VkDevice device, const VkSwapchainCreateInfoKHR* createInfo, const VkAllocationCallbacks* allocator, VkSwapchainKHR* output) { // wgpu-hal provider入口。
    trace_native_boundary("proxy swapchain: enter create callback"); // 证明wgpu已经选择FidelityFX provider。
    if (!g_attached.load(std::memory_order_acquire) || device != g_device || createInfo == nullptr || output == nullptr) { // 拒绝未知设备或残缺创建描述。
        set_error("invalid FSR 3 proxy swapchain creation", VK_ERROR_INITIALIZATION_FAILED); // 触发wgpu原生回退。
        return VK_ERROR_INITIALIZATION_FAILED; // Vulkan错误语义。
    } // 参数判断结束。
    const std::scoped_lock lock(g_lifecycleMutex); // Surface重建与Prepare不能交错。
    *output = VK_NULL_HANDLE; // 失败绝不返回旧代理句柄。
    VkSwapchainCreateInfoKHR sdkCreateInfo = *createInfo; // SDK使用UNORM存储，wgpu看到的原规格保持sRGB。
    if (createInfo->imageFormat == VK_FORMAT_B8G8R8A8_SRGB) { // Windows默认BGRA字节顺序。
        sdkCreateInfo.imageFormat = VK_FORMAT_B8G8R8A8_UNORM; // 允许SDK的STORAGE用途和UNORM视图。
    } else if (createInfo->imageFormat == VK_FORMAT_R8G8B8A8_SRGB) { // 支持同精度的RGBA sRGB窗口。
        sdkCreateInfo.imageFormat = VK_FORMAT_R8G8B8A8_UNORM; // 只去除传输函数标志，绝不重排像素通道。
    } else { // 其他HDR/格式仍需独立实现和实测，不伪造可用状态。
        set_error("unsupported FSR sRGB presentation format", VK_ERROR_FORMAT_NOT_SUPPORTED); // 普通wgpu交换链可继续创建。
        return VK_ERROR_FORMAT_NOT_SUPPORTED; // 在进入SDK之前安全回退。
    } // 可复制的同精度格式配对结束。
    const VkResult formatResult = validate_sdk_swapchain_format(sdkCreateInfo); // 对修改后的真实描述重新核验，不能沿用wgpu原sRGB能力结论。
    if (formatResult != VK_SUCCESS) { // 不支持的显卡/显示器无需进入SDK半初始化路径。
        set_error("unsupported FSR UNORM surface/image capabilities", formatResult); // 保存具体回退原因。
        g_attached.store(false, std::memory_order_release); // 本进程后续稳定保持原生交换链，避免反复失败创建。
        return formatResult; // 调用方使用原来的sRGB描述完成普通Present回退。
    } // SDK图像预检结束。
    VkResult vkResult = VK_ERROR_INITIALIZATION_FAILED; // 默认安全失败。
    if (g_swapchainContext == nullptr) { // 首次创建同时建立FFX代理上下文。
        ffxCreateContextDescFrameGenerationSwapChainModeVK mode{}; // 源码修订保持SDK公开的同步合成配置ABI。
        mode.header = make_header(FFX_API_CREATE_CONTEXT_DESC_TYPE_FGSWAPCHAIN_MODE_VK); // Vulkan合成模式扩展。
        mode.composeOnPresentQueue = false; // 游戏队列路径按实际图像索引使用Present信号量，避开1.1.4默认路径固定复用索引0的缺陷。
        ffxCreateContextDescFrameGenerationSwapChainVK create{}; // 官方Vulkan代理描述。
        create.header = make_header(FFX_API_CREATE_CONTEXT_DESC_TYPE_FGSWAPCHAIN_VK, &mode.header); // 首版allowAsync=false与合成队列策略一致。
        create.physicalDevice = g_physicalDevice; // 匹配wgpu适配器。
        create.device = g_device; // 匹配wgpu逻辑设备。
        create.swapchain = output; // 输入为空、成功后写代理句柄。
        create.allocator = const_cast<VkAllocationCallbacks*>(allocator); // SDK签名沿用非const指针但不取得所有权。
        create.createInfo = sdkCreateInfo; // SDK图像不能使用sRGB加STORAGE；真实窗口仍保持SRGB_NONLINEAR颜色空间。
        create.gameQueue = {g_queues[0], g_queueFamilies[0], nullptr}; // wgpu主图形队列。
        create.asyncComputeQueue = {g_queues[1], g_queueFamilies[1], nullptr}; // 优先使用专用计算族。
        create.presentQueue = {g_queues[2], g_queueFamilies[2], submit_present_queue}; // 主图形族中的第二条可Present队列。
        create.imageAcquireQueue = {g_queues[3], g_queueFamilies[3], nullptr}; // 优先使用低能力独立族。
        trace_native_boundary("proxy swapchain: before ffxCreateContext"); // 代理上下文是首个可能调用Vulkan替换入口的阶段。
        const ffxReturnCode_t result = g_createContext(&g_swapchainContext, &create.header, nullptr); // SDK内部创建代理及真实交换链。
        trace_native_boundary("proxy swapchain: after ffxCreateContext"); // 证明代理与真实交换链构造已经返回。
        if (result != FFX_API_RETURN_OK || g_swapchainContext == nullptr || *output == VK_NULL_HANDLE) { // 任何半成功都清理并回退。
            set_error("ffxCreateContext(frame-generation swapchain)", result); // 发布SDK结果。
            if (g_swapchainContext != nullptr) { // 尽力销毁半创建上下文。
                g_destroyContext(&g_swapchainContext, nullptr); // SDK负责其内部交换链。
                g_swapchainContext = nullptr; // 清除身份。
            } // 半创建清理结束。
            *output = VK_NULL_HANDLE; // 防止wgpu误用半创建句柄。
            g_attached.store(false, std::memory_order_release); // 本进程后续Surface重建不再尝试代理。
            return VK_ERROR_INITIALIZATION_FAILED; // wgpu同次调用创建原生交换链。
        } // 首次创建结果结束。
        g_replacements = {}; // 查询输出先清零。
        g_replacements.header = make_header(FFX_API_QUERY_DESC_TYPE_FGSWAPCHAIN_FUNCTIONS_VK); // 查询类型。
        trace_native_boundary("proxy swapchain: before replacement query"); // 函数表查询单独标记。
        const ffxReturnCode_t queryResult = g_query(&g_swapchainContext, &g_replacements.header); // 取得所有替换函数。
        trace_native_boundary("proxy swapchain: after replacement query"); // 查询返回后再做完整空指针审计。
        if (queryResult != FFX_API_RETURN_OK || !replacement_functions_valid()) { // 缺任一核心入口均不可接管。
            set_error("ffxQuery(frame-generation swapchain functions)", queryResult); // 发布查询错误。
            destroy_swapchain_context_locked(); // 销毁SDK刚创建的交换链和上下文。
            *output = VK_NULL_HANDLE; // 明确触发原生回退。
            g_attached.store(false, std::memory_order_release); // 禁止重复尝试。
            return VK_ERROR_INITIALIZATION_FAILED; // wgpu回退。
        } // 查询结果结束。
        vkResult = VK_SUCCESS; // 首次交换链已由CreateContext创建。
    } else { // resize/present mode变化只重建代理内部交换链，不重建代理上下文。
        if (g_replacements.pOutCreateSwapchainFFXAPI == nullptr) { // 不允许调用残缺指针。
            set_error("missing FSR 3 proxy create replacement", VK_ERROR_INITIALIZATION_FAILED); // 触发回退。
            return VK_ERROR_INITIALIZATION_FAILED; // 普通交换链继续。
        } // 替换入口判断结束。
        vkResult = g_replacements.pOutCreateSwapchainFFXAPI(device, &sdkCreateInfo, allocator, output, g_swapchainContext); // 重建继续使用相同UNORM契约。
        if (vkResult != VK_SUCCESS || *output == VK_NULL_HANDLE) { // 重建失败不保留旧规格。
            set_error("FSR 3 replacement vkCreateSwapchainKHR", vkResult); // 发布Vulkan错误。
            *output = VK_NULL_HANDLE; // 明确失败。
            destroy_swapchain_context_locked(); // 禁止下一次把原生回退交换链误交给残留代理上下文。
            g_attached.store(false, std::memory_order_release); // 本进程后续Surface配置稳定保持wgpu原生路径。
            return vkResult == VK_SUCCESS ? VK_ERROR_INITIALIZATION_FAILED : vkResult; // 防止成功空句柄。
        } // 重建结果结束。
    } // 首次/重建分支结束。
    g_swapchain = *output; // 采用当前代理句柄。
    std::array<VkImage, Bl5FsrPresentImages::kMaxImages> proxyImages{}; // 固定上限，不随异常SDK计数无界分配。
    uint32_t imageCount = 0; // 首先查询完整代理图像数量。
    vkResult = g_replacements.pOutGetSwapchainImagesKHR(device, g_swapchain, &imageCount, nullptr); // 只从SDK枚举UNORM目标。
    if (vkResult == VK_SUCCESS && imageCount > 0 && imageCount <= proxyImages.size()) { // 容量合法才允许SDK写数组。
        vkResult = g_replacements.pOutGetSwapchainImagesKHR(device, g_swapchain, &imageCount, proxyImages.data()); // 获取稳定的逐索引目标。
        if (vkResult == VK_SUCCESS) { // 全部UNORM目标齐备后建立成对sRGB图像。
            VkPhysicalDeviceMemoryProperties properties{}; // 交换链生命周期内只查询一次内存类型。
            g_vkGetMemoryProperties(g_physicalDevice, &properties); // 与当前设备匹配的物理内存类型。
            vkResult = g_presentImages.create(device, g_vkGetDeviceProcAddr, properties, *createInfo, g_queueFamilies[0], imageCount, proxyImages.data()); // 初始化图像和预录复制命令，不在逐帧分配。
        } // 前缓冲创建结束。
    } else if (vkResult == VK_SUCCESS) { // 数量不合法时不能把成功查询当作成功创建。
        vkResult = VK_ERROR_INITIALIZATION_FAILED; // 以完整安全回退收口。
    } // 枚举与有界初始化结束。
    if (vkResult != VK_SUCCESS) { // 失败不向wgpu暴露半套图像或错误格式。
        destroy_swapchain_context_locked(); // 前缓冲和代理一起释放。
        *output = VK_NULL_HANDLE; // 调用方改用普通Vulkan交换链。
        g_attached.store(false, std::memory_order_release); // 本进程不重复失败分配。
        set_error("FSR sRGB presentation images", vkResult); // 清理后保留主失败阶段。
        return vkResult; // 传递准确的Vulkan错误。
    } // 完整前缓冲才能进入就绪状态。
#ifndef NDEBUG // 创建边界记录实测驻留与固定逐帧复制量。
    std::fprintf(stderr, "[fsr3-fg-native] sRGB bridge: images=%u allocated_bytes=%llu copy_bytes_per_frame=%llu\n", imageCount, static_cast<unsigned long long>(g_presentImages.memory_bytes()), static_cast<unsigned long long>(createInfo->imageExtent.width) * createInfo->imageExtent.height * 4); // 不把颜色修复描述为零GPU成本。
#endif // 正式发行不增加控制台I/O。
    g_displayExtent = createInfo->imageExtent; // 发布当前显示尺寸。
    g_displayWidth.store(createInfo->imageExtent.width, std::memory_order_release); // 状态线程只读原子副本。
    g_displayHeight.store(createInfo->imageExtent.height, std::memory_order_release); // 状态线程只读原子副本。
    g_backBufferFormat = sdkCreateInfo.imageFormat; // FG核心必须匹配SDK实际UNORM图像。
    g_swapchainReady.store(true, std::memory_order_release); // 代理已拥有当前Surface。
    g_lastPresentCount.store(0, std::memory_order_release); // 新交换链累计计数重新开始。
    g_lastPresentDelta.store(1, std::memory_order_release); // 尚无真实2x证据。
    g_effective.store(false, std::memory_order_release); // 首帧前不宣称生效。
    trace_native_boundary("proxy swapchain: before FG core"); // 区分代理创建与FG核心创建。
    create_core_locked(sdkCreateInfo); // 核心失败仍通过sRGB前缓冲复制透传原始帧。
    trace_native_boundary("proxy swapchain: create callback complete"); // wgpu随后将枚举代理图像。
    return vkResult; // 代理交换链成功交给wgpu。
} // bl5_fsr3_fg_create_swapchain结束。

extern "C" void bl5_fsr3_fg_destroy_swapchain(VkDevice device, VkSwapchainKHR swapchain, const VkAllocationCallbacks* allocator) { // wgpu Surface释放或重建入口。
    const std::scoped_lock lock(g_lifecycleMutex); // 与Prepare及设备销毁串行。
    if (device != g_device || swapchain == VK_NULL_HANDLE || swapchain != g_swapchain) { // 未知句柄绝不交给FFX。
        return; // 保持幂等。
    } // 句柄身份判断结束。
    destroy_core_locked(); // 当前规格时间历史先释放。
    g_presentImages.destroy(); // wgpu的release_resources已经等待设备空闲，复制命令和信号量均可配对释放。
    // destroy_core_locked已经按官方顺序等待代理Present，下面可安全销毁真实交换链。
    if (g_replacements.pOutDestroySwapchainFFXAPI != nullptr && g_swapchainContext != nullptr) { // 使用查询出的配对销毁函数。
        g_replacements.pOutDestroySwapchainFFXAPI(device, swapchain, allocator, g_swapchainContext); // 上下文保留供下一次创建。
    } // 代理销毁调用结束。
    g_swapchain = VK_NULL_HANDLE; // 清除当前句柄。
    g_swapchainReady.store(false, std::memory_order_release); // 设置页显示等待重建。
    g_effective.store(false, std::memory_order_release); // 旧2x证据失效。
} // bl5_fsr3_fg_destroy_swapchain结束。

extern "C" VkResult bl5_fsr3_fg_get_swapchain_images(VkDevice device, VkSwapchainKHR swapchain, uint32_t* count, VkImage* images) { // wgpu图像枚举替换入口。
    const auto function = g_replacements.pOutGetSwapchainImagesKHR; // 生命周期由VkDevice销毁串行保证。
    if (function == nullptr || device != g_device || swapchain != g_swapchain) { // 查询未完成或设备/交换链身份过期时拒绝调用。
        return VK_ERROR_INITIALIZATION_FAILED; // wgpu会报告Surface错误而不解引用空指针。
    } // 函数存在性结束。
    const bool trace = !g_tracedGetImages.exchange(true, std::memory_order_acq_rel); // 仅首次调用输出成对边界。
    if (trace) { // 首次图像枚举判断结束。
        trace_native_boundary("proxy swapchain: before first get images"); // 访问冲突前保留准确入口。
    } // 首次调用前标记结束。
    const VkResult result = g_presentImages.get_images(count, images); // wgpu只取得自有sRGB图像，SDK的UNORM图像保持私有。
    if (trace) { // 与同一次首次调用配对。
        trace_native_boundary("proxy swapchain: after first get images"); // 证明替换函数成功返回。
    } // 首次调用后标记结束。
    return result; // 保留原始Vulkan结果。
} // bl5_fsr3_fg_get_swapchain_images结束。

extern "C" VkResult bl5_fsr3_fg_acquire_next_image(VkDevice device, VkSwapchainKHR swapchain, uint64_t timeout, VkSemaphore semaphore, VkFence fence, uint32_t* imageIndex) { // wgpu获取替换入口。
    const auto function = g_replacements.pOutAcquireNextImageKHR; // 稳定函数指针只读复制。
    if (function == nullptr) { // 代理不可用时安全失败。
        return VK_ERROR_INITIALIZATION_FAILED; // 不调用未知入口。
    } // 函数存在性结束。
    const bool trace = !g_tracedAcquire.exchange(true, std::memory_order_acq_rel); // 只跟踪第一次Acquire。
    if (trace) { // 首次Acquire判断结束。
        trace_native_boundary("proxy swapchain: before first acquire"); // 区分Surface创建与首帧获取。
    } // 首次调用前标记结束。
    const VkResult result = function(device, swapchain, timeout, semaphore, fence, imageIndex); // 由FFX维护插帧交换链同步。
    if (trace) { // 与第一次Acquire配对。
        trace_native_boundary("proxy swapchain: after first acquire"); // 证明SDK获取路径安全返回。
    } // 首次调用后标记结束。
    return result; // 保留原始Vulkan结果。
} // bl5_fsr3_fg_acquire_next_image结束。

extern "C" VkResult bl5_fsr3_fg_queue_present(VkQueue queue, const VkPresentInfoKHR* presentInfo) { // wgpu Present替换入口。
    const auto function = g_replacements.pOutQueuePresentKHR; // 不持生命周期锁，避免FFX回调重入死锁。
    if (function == nullptr) { // 代理不可用时安全失败。
        return VK_ERROR_INITIALIZATION_FAILED; // wgpu触发Surface恢复。
    } // 函数存在性结束。
    const bool trace = !g_tracedPresent.exchange(true, std::memory_order_acq_rel); // 只跟踪第一次Present。
    if (trace) { // 首次Present判断结束。
        trace_native_boundary("proxy swapchain: before first present"); // 空调用若位于代理Present内部会停在此处。
    } // 首次调用前标记结束。
    if (presentInfo == nullptr || queue != g_queues[0]) { // 复制与SDK gameQueue必须严格同队列，不向复制桥传空描述。
        return VK_ERROR_INITIALIZATION_FAILED; // 保留明确失败语义。
    } // 参数判断结束。
    { // SDK回调前结束生命周期锁，避免生成回调重入时死锁。
        const std::scoped_lock lock(g_lifecycleMutex); // 只保护同帧资格及必要的关闭配置。
        if (!g_uiResources.ready_for(g_frameId)) disable_frame_generation_locked(); // 捕获未完成时front保留完整普通画面，不准对其继续插帧或叠旧UI。
    } // 同帧UI资格判断结束。
    VkPresentInfoKHR copiedPresent{}; // 替换等待信号后仍保留同一个代理交换链和图像索引。
    const VkResult copyResult = g_presentImages.prepare_copy(queue, g_swapchain, *presentInfo, copiedPresent); // 同一游戏队列上复制编码像素，不等待CPU。
    if (copyResult != VK_SUCCESS) { // 不呈现未复制的SDK旧图像。
        { const std::scoped_lock lock(g_lifecycleMutex); g_uiResources.invalidate(); } // 失败也撤销同帧UI资格，与Prepare/注册保持同一锁约定。
        set_error("FSR sRGB presentation copy", copyResult); // 交由wgpu恢复Surface。
        return copyResult; // 明确停止当前失败Present。
    } // 完整复制提交后才允许SDK消费图像。
    const VkResult result = function(queue, &copiedPresent); // SDK等待scene-only复制完成，给真实帧和生成帧分别合成独立UI。
    { const std::scoped_lock lock(g_lifecycleMutex); g_uiResources.invalidate(); } // 下一真实帧必须重新捕获；与Prepare/关闭的所有状态访问使用同一锁。
    if (trace) { // 与第一次Present配对。
        trace_native_boundary("proxy swapchain: after first present"); // 证明首帧代理Present安全返回。
    } // 首次调用后标记结束。
    return result; // 保留原始Vulkan结果。
} // bl5_fsr3_fg_queue_present结束。

extern "C" int32_t bl5_fsr3_fg_set_ui_resources(const Bl5Fsr3Image* ui) { // Core3d所有UI绘制完成后只提交透明层，场景沿用现有前缓冲路径。
    const std::scoped_lock lock(g_lifecycleMutex); // 不允许与窗口重建或核心关闭交错采用。
    if (ui == nullptr || !g_requested.load(std::memory_order_acquire) || !g_warmupComplete || g_frameGenerationContext == nullptr || g_swapchain == VK_NULL_HANDLE) return FFX_API_RETURN_ERROR_PARAMETER; // 无本帧成功Prepare时Rust保留普通完整UI输出。
    const VkImageUsageFlags required = VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT; // SDK内部UI双缓冲必须能读取并复制透明纹理。
    if (ui->sampleCount != VK_SAMPLE_COUNT_1_BIT || (ui->usage & required) != required) return FFX_API_RETURN_ERROR_PARAMETER; // 多采样或缺复制用途不能交给SDK推测。
    const FfxApiResource uiResource = make_resource(*ui, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ); // Rust已登记颜色附件写入到采样/复制读取的转换。
    const ffxReturnCode_t result = g_uiResources.publish(&g_swapchainContext, g_configure, uiResource, g_backBufferFormat, g_displayExtent, g_frameId); // UI与当前交换链规格及刚完成的Prepare编号精确配对。
    if (result != FFX_API_RETURN_OK) set_error("ffxConfigure(independent premultiplied UI)", result); // 失败不发布独立UI事实，Rust必须恢复完整普通画面。
    if (result == FFX_API_RETURN_OK && !g_tracedUi.exchange(true, std::memory_order_acq_rel)) trace_native_boundary("independent UI: current-frame premultiplied sRGB layer registered"); // 证明真实渲染纹理已通过SDK登记，而非仅编译了UI模块。
    return result; // 成功后Rust才允许只把场景送入现有upscaling输出。
} // bl5_fsr3_fg_set_ui_resources结束。
extern "C" void bl5_fsr3_fg_set_hdr_metadata(VkDevice device, uint32_t swapchainCount, const VkSwapchainKHR* swapchains, const VkHdrMetadataEXT* metadata) { // 可选HDR扩展替换入口。
    const auto function = g_replacements.pOutSetHdrMetadataEXT; // 设备不支持时允许为空。
    if (function != nullptr) { // 只有扩展入口存在才转发。
        function(device, swapchainCount, swapchains, metadata); // 保持官方代理的HDR元数据路径。
    } // 可选转发结束。
} // bl5_fsr3_fg_set_hdr_metadata结束。

extern "C" uint64_t bl5_fsr3_fg_get_last_present_count(VkSwapchainKHR swapchain) { // wgpu/诊断读取代理累计Present数。
    const auto function = g_replacements.pOutGetLastPresentCountFFXAPI; // 稳定只读复制。
    return function != nullptr && swapchain != VK_NULL_HANDLE ? function(swapchain) : 0; // 缺代理时返回无证据。
} // bl5_fsr3_fg_get_last_present_count结束。

extern "C" int32_t bl5_fsr3_fg_prepare(const Bl5Fsr3PrepareDesc* input) { // Core3d UI前节点同步录制Prepare。
    if (input == nullptr || input->commandBuffer == 0 || input->renderWidth == 0 || input->renderHeight == 0) { // 拒绝残缺帧。
        set_error("invalid FSR 3 frame-generation inputs", FFX_API_RETURN_ERROR_PARAMETER); // 普通Present继续。
        return g_lastResult.load(std::memory_order_acquire); // 返回失败。
    } // 基础参数判断结束。
    const std::scoped_lock lock(g_lifecycleMutex); // 与resize、关闭和设备销毁串行。
    g_uiResources.invalidate(); // 每个Prepare必须等本帧UI重新捕获，旧注册不能证明新帧完整。
    const bool requested = input->requested != 0; // 复制业务请求避免跨ABI布尔差异。
    g_requested.store(requested, std::memory_order_release); // 对外发布玩家请求。
    if (!requested || g_frameGenerationContext == nullptr || g_swapchain == VK_NULL_HANDLE) { // 无资格或核心未就绪时明确关闭。
        disable_frame_generation_locked(); // 不复用上一帧资源。
        return requested ? FFX_API_RETURN_ERROR_RUNTIME_ERROR : FFX_API_RETURN_OK; // 主动关闭不是错误。
    } // 生命周期判断结束。
    const bool reset = input->reset != 0; // 断续帧只预热历史。
    const bool enableThisPresent = g_warmupComplete && !reset; // 至少一帧完整Prepare后才生成。
    g_measurementActive = enableThisPresent; // 只对连续启用的宿主帧采集实际呈现证据。
    if (!enableThisPresent) { // 首帧和切镜帧不纳入稳定倍率窗口。
        g_measurementInitialized = false; // 切镜后从新连续窗口起点开始。
        g_measurementFrames = 0; // 下次完整启用从新的累计数起点开始。
        g_lastPresentDelta.store(1, std::memory_order_release); // 清除旧窗口倍率。
        g_effective.store(false, std::memory_order_release); // 不把预热称为有效2x。
    } // 预热统计边界结束。
    FfxApiResource depth = make_resource(input->depth, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ); // 当前深度预通道只读。
    FfxApiResource motion = make_resource(input->motionVectors, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ); // 当前运动预通道只读。
    if (depth.resource == nullptr || motion.resource == nullptr || depth.description.width != motion.description.width || depth.description.height != motion.description.height) { // 深度与运动矢量必须逐帧完整且尺寸一致。
        disable_frame_generation_locked(); // 撤销上一成功配置。
        set_error("incomplete FSR 3 frame-generation resources", FFX_API_RETURN_ERROR_PARAMETER); // 结构化输入阻断。
        return g_lastResult.load(std::memory_order_acquire); // 返回失败。
    } // 资源判断结束。

    ffxConfigureDescFrameGeneration config{}; // 当前Present完整配置快照。
    config.header = make_header(FFX_API_CONFIGURE_DESC_TYPE_FRAMEGENERATION); // 描述身份。
    config.swapChain = reinterpret_cast<void*>(g_swapchain); // 当前代理句柄。
    config.frameGenerationCallback = dispatch_frame_generation; // 同步回调录制生成工作。
    config.frameGenerationCallbackUserContext = nullptr; // 回调使用静态上下文。
    config.frameGenerationEnabled = enableThisPresent; // 首帧/断续帧固定关闭。
    config.allowAsyncWorkloads = false; // 首版同步模式，降低跨队列时序风险。
    config.HUDLessColor = FfxApiResource{}; // 注册独立UI成功后前缓冲本身只有场景，无需从完整画面推断UI。
    config.flags = 0; // 关闭调试覆盖和特殊Present模式。
    config.onlyPresentGenerated = false; // 原始帧和一张生成帧共同呈现为2x。
    config.generationRect = {0, 0, static_cast<int32_t>(g_displayExtent.width), static_cast<int32_t>(g_displayExtent.height)}; // 完整窗口区域。
    config.frameID = g_frameId; // 与Prepare严格一致。
    ffxReturnCode_t result = g_configure(&g_frameGenerationContext, &config.header); // Present消费配置前同步提交。
    if (result != FFX_API_RETURN_OK) { // 配置失败立即回退。
        disable_frame_generation_locked(); // 不保留旧启用态。
        set_error("ffxConfigure(frame generation)", result); // 发布精确错误。
        return result; // 返回SDK结果。
    } // 配置结果结束。

    ffxDispatchDescFrameGenerationPrepareCameraInfo camera{}; // 3.1.4最佳质量必需扩展。
    camera.header = make_header(FFX_API_DISPATCH_DESC_TYPE_FRAMEGENERATION_PREPARE_CAMERAINFO); // 扩展身份。
    std::memcpy(camera.cameraPosition, input->cameraPosition, sizeof(camera.cameraPosition)); // 世界位置。
    std::memcpy(camera.cameraUp, input->cameraUp, sizeof(camera.cameraUp)); // 单位上方向。
    std::memcpy(camera.cameraRight, input->cameraRight, sizeof(camera.cameraRight)); // 单位右方向。
    std::memcpy(camera.cameraForward, input->cameraForward, sizeof(camera.cameraForward)); // 单位前方向。

    ffxDispatchDescFrameGenerationPrepare prepare{}; // 当前真实渲染帧的时间输入。
    prepare.header = make_header(FFX_API_DISPATCH_DESC_TYPE_FRAMEGENERATION_PREPARE, &camera.header); // 主描述链接相机扩展。
    prepare.frameID = g_frameId; // 与Configure精确相同。
    prepare.flags = 0; // 无调试覆盖。
    prepare.commandList = reinterpret_cast<void*>(static_cast<uintptr_t>(input->commandBuffer)); // 借用当前VkCommandBuffer。
    prepare.renderSize = {input->renderWidth, input->renderHeight}; // 预通道有效范围。
    prepare.jitterOffset = {input->jitterX, input->jitterY}; // Bevy实际TemporalJitter像素值。
    prepare.motionVectorScale = {input->motionScaleX, input->motionScaleY}; // 与Bevy归一化UV编码匹配。
    prepare.frameTimeDelta = input->frameTimeMs; // SDK单位毫秒。
    prepare.unused_reset = reset; // 3.1.4仍保留字段，业务同时通过一帧禁用清历史。
    prepare.cameraNear = FLT_MAX; // 官方反向无限Z约定：near字段使用无限值。
    prepare.cameraFar = input->cameraNear; // far字段携带实际近裁面。
    prepare.cameraFovAngleVertical = input->cameraFovY; // 垂直FOV弧度。
    prepare.viewSpaceToMetersFactor = 1.0f; // Bevy世界单位按一米解释。
    prepare.depth = depth; // 当前反向Z深度。
    prepare.motionVectors = motion; // 当前运动矢量。
    const auto barrier = reinterpret_cast<PFN_vkCmdPipelineBarrier>(g_vkGetDeviceProcAddr(g_device, "vkCmdPipelineBarrier")); // Vulkan核心屏障入口，使用宿主同一设备。
    VkImageMemoryBarrier depthBarrier{}; // wgpu深度只读布局与FFX统一采样布局不同，必须成对转换。
    depthBarrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER; // 核心Vulkan屏障描述。
    depthBarrier.srcAccessMask = VK_ACCESS_SHADER_READ_BIT | VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_READ_BIT; // 对应wgpu RESOURCE深度只读状态。
    depthBarrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT; // FFX Prepare的Compute采样。
    depthBarrier.oldLayout = VK_IMAGE_LAYOUT_DEPTH_STENCIL_READ_ONLY_OPTIMAL; // Rust已通过transition_resources建立此布局。
    depthBarrier.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL; // FFX 1.1.4注册深度时也声明统一采样布局。
    depthBarrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; // 同一游戏队列内录制，不转移队列所有权。
    depthBarrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; // 不错误地指定零号队列族。
    depthBarrier.image = reinterpret_cast<VkImage>(static_cast<uintptr_t>(input->depth.image)); // 只借用当前深度。
    depthBarrier.subresourceRange = {VK_IMAGE_ASPECT_DEPTH_BIT, 0, 1, 0, 1}; // 当前单mip单层预通道。
    const VkCommandBuffer commandBuffer = reinterpret_cast<VkCommandBuffer>(static_cast<uintptr_t>(input->commandBuffer)); // 与Prepare共用同一缓冲。
    barrier(commandBuffer, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, 0, 0, nullptr, 0, nullptr, 1, &depthBarrier); // GPU依赖，不在CPU等待。
    result = g_dispatch(&g_frameGenerationContext, &prepare.header); // 在同一wgpu命令缓冲录制Prepare。
    depthBarrier.srcAccessMask = VK_ACCESS_SHADER_READ_BIT; // SDK已消费深度。
    depthBarrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT | VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_READ_BIT; // 恢复wgpu跟踪的可读用途。
    depthBarrier.oldLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL; // FFX注销资源后保留传入的采样布局。
    depthBarrier.newLayout = VK_IMAGE_LAYOUT_DEPTH_STENCIL_READ_ONLY_OPTIMAL; // 成功和失败都恢复，避免污染后续Bevy命令。
    barrier(commandBuffer, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, nullptr, 0, nullptr, 1, &depthBarrier); // 固定第二道屏障，无复制或同步等待。
    if (result != FFX_API_RETURN_OK) { // 本帧Prepare失败不得推进身份。
        disable_frame_generation_locked(); // 下一Present只透传原始帧。
        set_error("ffxDispatch(frame-generation prepare)", result); // 发布精确阶段。
        return result; // 返回SDK结果。
    } // Prepare结果结束。
    if (!g_tracedPrepare.exchange(true, std::memory_order_acq_rel)) { // 只记录首个完整GPU输入提交。
        trace_native_boundary("frame generation: first prepare succeeded"); // 证明深度、运动矢量与相机参数已被SDK接受。
    } // 首次Prepare跟踪结束。
    ++g_frameId; // 只有完整成功帧推进精确+1身份。
    g_warmupComplete = true; // 当前输入可作为下一帧历史。
    g_lastResult.store(FFX_API_RETURN_OK, std::memory_order_release); // 清理旧错误码。
    clear_error(); // 清理旧输入诊断。
    return FFX_API_RETURN_OK; // 当前原始帧可安全Present。
} // bl5_fsr3_fg_prepare结束。

extern "C" void bl5_fsr3_fg_disable() { // 请求关闭、缺输入或切换页面时撤销旧配置。
    const std::scoped_lock lock(g_lifecycleMutex); // 与Prepare和重建串行。
    disable_frame_generation_locked(); // 幂等关闭并清预热。
} // bl5_fsr3_fg_disable结束。

extern "C" void bl5_fsr3_fg_quiesce_swapchain() { // 宿主DeviceWaitIdle前先停止SDK的Present生产线程。
    const std::scoped_lock lock(g_lifecycleMutex); // 只在销毁或重建边界串行化。
    disable_frame_generation_locked(); // 官方关闭配置停止生成与异步呈现线程。
    wait_for_presents_locked(); // 等已排队Present完成后，宿主才可独占设备队列执行WaitIdle。
} // bl5_fsr3_fg_quiesce_swapchain结束。

extern "C" void bl5_fsr3_fg_on_present_start() { // 紧贴代理vkQueuePresentKHR前记录累计计数。
    const auto function = g_replacements.pOutGetLastPresentCountFFXAPI; // 不持锁以避免Present回调重入。
    const VkSwapchainKHR swapchain = g_swapchain; // 读取当前稳定句柄。
    if (g_measurementActive && !g_measurementInitialized) { // 只在连续启用阶段的首帧初始化；后续窗口必须无缝相接。
        g_presentStartCount = function != nullptr && swapchain != VK_NULL_HANDLE ? function(swapchain) : 0; // 记录真实累计Present起点。
        g_generationStartCount = g_generatedCount.load(std::memory_order_relaxed); // 生成计数使用同一窗口。
        g_measurementInitialized = true; // 后续窗口复用上一窗口的结束累计值。
    } // 稳定帧保留起点。
} // bl5_fsr3_fg_on_present_start结束。

extern "C" void bl5_fsr3_fg_on_present_end() { // 成功Present后在有界窗口内观测实际平均倍率。
    const auto function = g_replacements.pOutGetLastPresentCountFFXAPI; // 稳定只读复制。
    const VkSwapchainKHR swapchain = g_swapchain; // 当前代理句柄。
    const uint64_t after = function != nullptr && swapchain != VK_NULL_HANDLE ? function(swapchain) : 0; // Present后累计数。
    g_lastPresentCount.store(after, std::memory_order_release); // 发布累计证据。
    if (!g_measurementActive || ++g_measurementFrames < 120) { // 120帧固定窗口内只做常量时间计数。
        return; // 未收满不伪造证据，连续有效窗口之间沿用上一完整测量。
    } // 完整窗口边界。
    const uint64_t difference = after >= g_presentStartCount ? after - g_presentStartCount : 0; // 防止交换链重建回绕。
    const uint64_t generated = g_generatedCount.load(std::memory_order_relaxed) - g_generationStartCount; // 检查实际生成输出数量。
    const bool measured2x = difference >= 236 && difference <= 244 && generated == 120; // 允许异步窗口端点最多两宿主帧偏差，但生成不能漏帧。
    g_lastPresentDelta.store(measured2x ? 2u : 1u, std::memory_order_release); // 公开窗口近似倍率，绝不声称单次Present精确帧数。
    g_effective.store(g_requested.load(std::memory_order_acquire) && measured2x, std::memory_order_release); // 请求、生成和真实Present三项证据同时成立。
#ifndef NDEBUG // 每120帧一条聚合诊断，保留连续验证证据。
    std::fprintf(stderr, "[fsr3-fg-native] present window: host=120 generated=%llu presented=%llu measured2x=%u\n", static_cast<unsigned long long>(generated), static_cast<unsigned long long>(difference), measured2x ? 1u : 0u); // 可据此发现间歇失败，不再只记录首次成功。
#endif // 发行路径不写逐窗口控制台。
    g_measurementFrames = 0; // 下一个窗口独立取基线。
    g_presentStartCount = after; // 紧邻窗口共享结束/起点，不丢失宿主调用间隙的异步Present。
    g_generationStartCount += generated; // 与同一结束时刻的成功生成累计数保持一致。
} // bl5_fsr3_fg_on_present_end结束。

extern "C" void bl5_fsr3_fg_on_present_failure() { // Surface丢失/过期时撤销旧倍率证据。
    g_measurementActive = false; // 失败不能跨过窗口继续累计。
    g_measurementInitialized = false; // 恢复后重新建立累计起点。
    g_measurementFrames = 0; // 恢复后重新观察完整窗口。
    g_lastPresentDelta.store(1, std::memory_order_release); // 回到普通宿主帧基线。
    g_effective.store(false, std::memory_order_release); // 不沿用上一次成功证据。
    set_error("FSR 3 proxy queue present failed", VK_ERROR_OUT_OF_DATE_KHR); // 设置页显示当前Surface阻断。
} // bl5_fsr3_fg_on_present_failure结束。

extern "C" void bl5_fsr3_fg_shutdown() { // wgpu最后一个设备资源释放后、VkDevice销毁前调用。
    const std::scoped_lock lock(g_lifecycleMutex); // 与所有厂商生命周期入口串行。
    destroy_swapchain_context_locked(); // 先等待Present并释放设备对象。
    g_attached.store(false, std::memory_order_release); // 阻止后续provider查询。
    g_physicalDevice = VK_NULL_HANDLE; // 清除宿主句柄借用。
    g_device = VK_NULL_HANDLE; // 清除宿主句柄借用。
    g_queues = {}; // 清除队列借用。
    g_queueFamilies = {}; // 清除队列族身份。
    g_requested.store(false, std::memory_order_release); // 清理公开请求。
    g_effective.store(false, std::memory_order_release); // 清理公开倍率。
    if (g_fidelityfxModule != nullptr) { // 所有FFX上下文和回调都已退出才卸载DLL。
        FreeLibrary(g_fidelityfxModule); // 释放官方运行库。
        g_fidelityfxModule = nullptr; // 清除模块身份。
    } // DLL卸载结束。
    g_createContext = nullptr; // 清除全部动态函数指针。
    g_configure = nullptr; // 清除配置入口。
    g_query = nullptr; // 清除查询入口。
    g_dispatch = nullptr; // 清除调度入口。
    g_destroyContext = nullptr; // 清除销毁入口。
    g_runtimeLoaded.store(false, std::memory_order_release); // 生命周期关闭完成。
} // bl5_fsr3_fg_shutdown结束。

extern "C" void bl5_fsr3_fg_get_status(Bl5Fsr3NativeStatus* output) { // Rust低频复制无锁状态。
    if (output == nullptr) { // 防御空指针。
        return; // 不写未知内存。
    } // 输出判断结束。
    output->lastResult = g_lastResult.load(std::memory_order_acquire); // 最近结果。
    output->availableQueues = g_availableQueues.load(std::memory_order_acquire); // 实际队列数。
    output->lastPresentCount = g_lastPresentCount.load(std::memory_order_acquire); // 累计代理Present。
    output->lastPresentDelta = g_lastPresentDelta.load(std::memory_order_acquire); // 最近倍率。
    output->displayWidth = g_displayWidth.load(std::memory_order_acquire); // 避免状态线程与Surface重建形成C++数据竞争。
    output->displayHeight = g_displayHeight.load(std::memory_order_acquire); // 同上。
    output->runtimeLoaded = g_runtimeLoaded.load(std::memory_order_acquire) ? 1 : 0; // DLL事实。
    output->attached = g_attached.load(std::memory_order_acquire) ? 1 : 0; // 设备事实。
    output->swapchainReady = g_swapchainReady.load(std::memory_order_acquire) ? 1 : 0; // 代理事实。
    output->contextReady = g_contextReady.load(std::memory_order_acquire) ? 1 : 0; // 核心事实。
    output->requested = g_requested.load(std::memory_order_acquire) ? 1 : 0; // 请求事实。
    output->effective = g_effective.load(std::memory_order_acquire) ? 1 : 0; // 真实2x事实。
    output->padding[0] = 0; // 明确初始化ABI填充。
    output->padding[1] = 0; // 明确初始化ABI填充。
} // bl5_fsr3_fg_get_status结束。

extern "C" void bl5_fsr3_fg_copy_last_error(char* output, size_t capacity) { // Rust通过自有缓冲取得稳定诊断。
    if (output == nullptr || capacity == 0) { // 防御无效范围。
        return; // 不写未知内存。
    } // 输出范围判断结束。
    const std::scoped_lock lock(g_errorMutex); // 与错误写入互斥，稳定Present无锁。
    std::snprintf(output, capacity, "%s", g_error.data()); // 始终NUL结尾并自动截断。
} // bl5_fsr3_fg_copy_last_error结束。
