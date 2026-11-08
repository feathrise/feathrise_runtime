// 本文件是Bevy/wgpu与NVIDIA Streamline之间唯一原生ABI边界，所有调用均从已验签DLL动态解析。
#include <sl.h> // Streamline核心类型、特性标识和动态函数签名。
#include <sl_dlss_g.h> // DLSS Frame Generation选项与运行状态。
#include <sl_pcl.h> // Reflex统计使用的Simulation/Render/Present标记。
#include <sl_reflex.h> // Reflex低延迟sleep与模式配置。
#include <sl_security.h> // NVIDIA官方嵌入签名与签名者校验。

#include <Windows.h> // 安全加载DLL及Windows UTF-16路径。
#include <array> // 固定运行时白名单与错误缓冲。
#include <atomic> // Present线程向Rust发布无锁状态快照。
#include <chrono> // 仅显式诊断统计真实宿主Present节奏，不读取或修改游戏时间。
#include <cstdint> // 稳定C ABI整数宽度。
#include <cstdio> // 有界格式化诊断文本。
#include <cstring> // 把Rust提供的行主序矩阵复制到Streamline固定矩阵结构。
#include <iterator> // `std::size`计算固定功能数组长度。
#include <mutex> // 错误文本仅在异常路径加锁，避免Present线程与主线程数据竞争。
#include <string> // 组合明确的运行时文件绝对路径。
#include "nvidia_authenticode.h" // 普通NVIDIA依赖使用可信Authenticode，SL模块仍要求官方双签。
#include "presentation_counter.h" // SDK增量只累计一次，供主世界真实呈现FPS采样。

namespace { // 所有Streamline状态仅由本翻译单元持有，避免外部误用代理接口。

constexpr std::array<const wchar_t*, 7> kRuntimeFiles = { // 运行时再次验证与build.rs相同的精确白名单。
    L"sl.interposer.dll", // Vulkan入口与Present代理。
    L"sl.common.dll", // Streamline公共插件。
    L"sl.dlss_g.dll", // DLSS-G插件。
    L"sl.reflex.dll", // Reflex插件。
    L"sl.pcl.dll", // PC Latency插件。
    L"NvLowLatencyVk.dll", // Vulkan Reflex桥。
    L"nvngx_dlssg.dll", // NGX帧生成实现。
}; // 签名验证列表结束。

HMODULE g_interposer = nullptr; // 模块必须至少存活到Vulkan实例和设备全部销毁。
std::atomic<bool> g_initialized{false}; // slInit成功事实跨主线程与状态刷新安全发布。
std::atomic<bool> g_attached{false}; // 设备附着事实由队列回调无锁读取。
std::array<char, 1024> g_error{}; // Rust可随时复制的稳定错误文本。
std::mutex g_error_mutex; // 只保护稀有诊断文本，不进入正常Present路径。
std::atomic<uint64_t> g_complete_frame{~uint64_t{0}}; // 只有同帧独立UI成功标签才发布完成；64位哨兵不与u32帧号冲突。
std::atomic<uint32_t> g_driver_frame{0}; // wgpu提交和Present回调对应的真实渲染帧号。
std::atomic<int32_t> g_last_result{static_cast<int32_t>(sl::Result::eErrorNotInitialized)}; // 最近一次Streamline结果。
std::atomic<uint32_t> g_dlssg_status{0}; // DLSSGState位标志。
std::atomic<uint32_t> g_frames_presented{1}; // 上次宿主Present实际产生的总帧数。
Bl5PresentationCounter g_presentation; // Render/Present串行写入，主世界非阻塞复制一致快照。
std::atomic<uint32_t> g_max_generated_frames{0}; // 当前GPU支持的最大插帧数。
std::atomic<uint32_t> g_min_dimension{0}; // 当前插件要求的最小交换链边长。
std::atomic<uint64_t> g_estimated_vram{0}; // 只在显式状态查询返回时更新的估算。
std::atomic<bool> g_requested{false}; // 当前宿主是否请求2x。
std::atomic<bool> g_effective{false}; // 最近Present是否处于无错误的2x状态。

PFun_slInit* g_sl_init = nullptr; // 核心初始化函数。
PFun_slShutdown* g_sl_shutdown = nullptr; // 核心关闭函数。
PFun_slIsFeatureSupported* g_sl_is_feature_supported = nullptr; // 物理设备能力查询。
PFun_slGetFeatureFunction* g_sl_get_feature_function = nullptr; // 插件函数解析入口。
PFun_slGetNewFrameToken* g_sl_get_new_frame_token = nullptr; // 统一帧编号令牌。
PFun_slSetConstants* g_sl_set_constants = nullptr; // 每帧无抖动相机常量。
PFun_slSetTagForFrame* g_sl_set_tag_for_frame = nullptr; // 带帧身份的资源标签。
PFun_slDLSSGSetOptions* g_dlssg_set_options = nullptr; // 2x开关和输入规格。
PFun_slDLSSGGetState* g_dlssg_get_state = nullptr; // Present线程一次性状态读取。
PFun_slReflexSetOptions* g_reflex_set_options = nullptr; // Reflex模式配置。
PFun_slReflexGetState* g_reflex_get_state = nullptr; // 开启时核实实际低延迟能力，不能只依赖通用支持查询。
PFun_slReflexSleep* g_reflex_sleep = nullptr; // Simulation前低延迟sleep。
PFun_slPCLSetMarker* g_pcl_set_marker = nullptr; // Simulation/Render/Present时序标记。
sl::ViewportHandle g_viewport{0}; // 首期唯一玩法窗口固定使用viewport 0。
sl::DLSSGOptions g_options{}; // 最近一次提交给插件的2x配置。
bool g_diagnostic = false; // 引导完成前设置，此后各线程只读。
std::atomic<uint32_t> g_diagnostic_submits{0}; // 显式诊断下累计两个宿主Present之间的真实提交次数。

void diagnostic_log(sl::LogType type, const char* message) { // 显式烟雾诊断把SDK真实失败原因写入同一验证日志。
    if (message == nullptr) { return; } // SDK空消息不能交给字符串检查。
    if (type == sl::LogType::eInfo && (std::strstr(message, "[manageVRAM]") || std::strstr(message, "[collectGarbage]") || (std::strstr(message, "[ngxLog]") && !std::strstr(message, "error") && !std::strstr(message, "Error") && !std::strstr(message, "fail") && !std::strstr(message, "Fail")))) { return; } // 跳过逐资源分配噪声，保留所有SDK警告/错误及NGX失败文本。
    static std::atomic<uint32_t> count{0}; // 每次进程最多输出4096条，覆盖NGX启动枚举后仍保持有界。
    if (message != nullptr && count.fetch_add(1, std::memory_order_relaxed) < 4096) { std::fprintf(stderr, "[Streamline:%u] %.4096s\n", static_cast<uint32_t>(type), message); } // 文本长度和总条数都有固定上限。
} // 仅在显式环境开关下注册，不改变正式运行日志。

void set_error(const char* message, int32_t result) { // 统一发布稳定错误码与文本。
    g_last_result.store(result, std::memory_order_release); // 先记录精确原生结果。
    const std::scoped_lock lock(g_error_mutex); // 错误路径序列化文本写入与Rust复制。
    std::snprintf(g_error.data(), g_error.size(), "%s (Streamline result=%d)", message, result); // 文本始终NUL结尾。
} // set_error结束。

void on_api_error(const sl::APIError& error) { // SDK异步Present/Acquire的失败不会可靠地出现在宿主VkResult中。
    char message[192]{}; std::snprintf(message, sizeof(message), "DLSS-G asynchronous Vulkan present/acquire failed: VkResult=%d", static_cast<int32_t>(error.vkRes)); // 保留具体Vulkan失败值。
    set_error(message, static_cast<int32_t>(sl::Result::eErrorVulkanAPI)); // 结构化状态可读取异步错误。
    g_effective.store(false, std::memory_order_release); // 异步失败后不能继续宣称2x。
    g_requested.store(false, std::memory_order_release); // RenderApp下一轮撤销标签并重新预热。
    if (g_diagnostic) { diagnostic_log(sl::LogType::eError, message); } // 显式烟雾保留不会被后续成功覆盖的原始诊断。
} // API异步错误回调结束。

void clear_error() { // 成功边界统一清理旧诊断且不与状态刷新竞争。
    const std::scoped_lock lock(g_error_mutex); // 正常稳定帧不会调用本函数。
    g_error[0] = '\0'; // 保留固定缓冲，其余字节无需重写。
} // clear_error结束。

template <typename T> bool load_core(T*& target, const char* name) { // 从已验签interposer解析核心导出。
    target = reinterpret_cast<T*>(GetProcAddress(g_interposer, name)); // GetProcAddress不执行外部路径搜索。
    if (target != nullptr) { // 导出存在即可继续。
        return true; // 返回成功。
    } // 导出存在性判断结束。
    set_error(name, static_cast<int32_t>(sl::Result::eErrorMissingOrInvalidAPI)); // SDK结构不匹配必须硬阻断。
    return false; // 阻止调用空指针。
} // load_core结束。

template <typename T> bool load_feature(sl::Feature feature, T*& target, const char* name) { // 设备创建后解析插件函数。
    void* function = nullptr; // Streamline通过无类型指针返回功能入口。
    const sl::Result result = g_sl_get_feature_function(feature, name, function); // 只查询本次slInit请求的插件。
    if (result == sl::Result::eOk && function != nullptr) { // 结果与指针必须同时有效。
        target = reinterpret_cast<T*>(function); // 转成SDK头声明的精确签名。
        return true; // 功能入口可用。
    } // 功能函数结果判断结束。
    set_error(name, static_cast<int32_t>(result)); // 缺失入口保留官方结果码。
    return false; // 阻止半初始化。
} // load_feature结束。

sl::FrameToken* frame_token(uint32_t frame_index) { // 用显式宿主帧号取得所有阶段共享的Streamline令牌。
    if (!g_attached.load(std::memory_order_acquire) || g_sl_get_new_frame_token == nullptr) { // 设备未附着时不能请求令牌。
        return nullptr; // 安全跳过标记。
    } // 附着状态判断结束。
    sl::FrameToken* token = nullptr; // 令牌内存由Streamline持有。
    const sl::Result result = g_sl_get_new_frame_token(token, &frame_index); // 同一帧各阶段传同一显式编号。
    if (result != sl::Result::eOk || token == nullptr) { // 任一失败都保留状态但不崩溃宿主。
        set_error("slGetNewFrameToken", static_cast<int32_t>(result)); // 发布诊断。
        return nullptr; // 调用者跳过该阶段。
    } // 令牌结果判断结束。
    if (static_cast<uint32_t>(*token) != frame_index) { set_error("Streamline frame token identity mismatch", static_cast<int32_t>(sl::Result::eErrorInvalidState)); return nullptr; } // 防止令牌换代或ABI不符静默提交错帧。
    return token; // 返回只读SDK令牌。
} // frame_token结束。

void mark_current(sl::PCLMarker marker) { // wgpu底层回调用当前真实提交帧发送标记。
    if (g_pcl_set_marker == nullptr) { // PCL未解析时安全跳过。
        return; // 普通渲染不受影响。
    } // PCL入口判断结束。
    const uint32_t frame_index = g_driver_frame.load(std::memory_order_acquire); // 读取RenderApp在提交前发布的帧号。
    if (sl::FrameToken* token = frame_token(frame_index)) { // 只在令牌有效时调用插件。
        const sl::Result result = g_pcl_set_marker(marker, *token); // 标记紧贴真实wgpu调用边界。
        if (result != sl::Result::eOk) { // 标记失败不能隐瞒。
            set_error("slPCLSetMarker", static_cast<int32_t>(result)); // 发布官方结果。
        } // 标记结果判断结束。
    } // 令牌有效范围结束。
} // mark_current结束。

void query_present_state() { // 每个宿主Present后严格只查询一次DLSS-G状态。
    if (g_dlssg_get_state == nullptr) { // 功能未附着时保持关闭事实。
        g_presentation.invalidate(); // 没有可信计数来源时撤销呈现FPS。
        g_effective.store(false, std::memory_order_release); // 不宣称插帧生效。
        return; // 无插件调用。
    } // 状态入口判断结束。
    sl::DLSSGState state{}; // SDK结构默认初始化全部版本字段。
    const sl::Result result = g_dlssg_get_state(g_viewport, state, nullptr); // 常规每帧查询不请求昂贵VRAM估算。
    g_last_result.store(static_cast<int32_t>(result), std::memory_order_release); // Rust状态显示官方调用结果。
    if (result != sl::Result::eOk) { // 状态调用失败时立即停止宣称有效。
        g_presentation.invalidate(); // 不把未读到的SDK帧数补成一帧或沿用旧倍率。
        g_effective.store(false, std::memory_order_release); // 请求可保留但实际关闭。
        set_error("slDLSSGGetState", static_cast<int32_t>(result)); // 发布诊断。
        return; // 不读取无效state字段。
    } // 状态结果判断结束。
    const uint32_t status = static_cast<uint32_t>(state.status); // 位标志原样传给结构化Rust状态。
    g_dlssg_status.store(status, std::memory_order_release); // 发布运行时质量错误。
    g_frames_presented.store(state.numFramesActuallyPresented, std::memory_order_release); // 保留SDK原始计数；零表示尚无呈现证据，不能伪造成一帧。
    g_presentation.record(g_options.mode == sl::DLSSGMode::eOff ? 1 : state.numFramesActuallyPresented); // 此入口仅在vkQueuePresent成功后调用；确认Off时统计该次普通Present，On时消费SDK自上次查询以来的实际增量。
    g_max_generated_frames.store(state.numFramesToGenerateMax, std::memory_order_release); // 首期只消费>=1能力。
    g_min_dimension.store(state.minWidthOrHeight, std::memory_order_release); // 供设置页解释低分辨率阻断。
    g_effective.store(g_requested.load(std::memory_order_acquire) && status == 0 && state.numFramesActuallyPresented >= 2, std::memory_order_release); // 只有真实Present计数证明2x生效。
    if (g_diagnostic) { // 正式运行没有额外计时、累计或格式化。
        static auto start = std::chrono::steady_clock::now(); // 首个有效样本之前允许被关闭态重置。
        static uint32_t hosts = 0, displayed = 0, max_submits = 0; // 固定120帧窗口，不保存逐帧数组。
        const auto now = std::chrono::steady_clock::now(); // 一次读取单调时钟。
        const uint32_t submits = g_diagnostic_submits.exchange(0, std::memory_order_relaxed); // 只计宿主，不混入SL内部pacer提交。
        if (!g_requested.load(std::memory_order_acquire)) { start = now; hosts = displayed = max_submits = 0; } // 加载/关闭时间不污染有效窗口。
        else { // 只在真实On阶段记录SDK是否丢弃或未生成额外帧。
            ++hosts; displayed += state.numFramesActuallyPresented; if (submits > max_submits) { max_submits = submits; } // 记录真实计数，不设置倍率下限。
            if (hosts == 120) { // 固定低频输出，不逐帧刷日志。
                const double seconds = std::chrono::duration<double>(now - start).count(); // 实际宿主呈现耗时。
                char message[256]{}; std::snprintf(message, sizeof(message), "Host cadence frame=%u appFPS=%.2f sdkRatio=%.3f maxSubmits=%u", g_driver_frame.load(), seconds > 0 ? hosts / seconds : 0.0, static_cast<double>(displayed) / hosts, max_submits); // 区分设置上限和真实帧率。
                diagnostic_log(sl::LogType::eInfo, message); // 与SDK和烟雾证据保存在同一日志。
                start = now; hosts = displayed = max_submits = 0; // 下一有界窗口独立计数。
            } // 窗口输出结束。
        } // 请求状态分支结束。
    } // 显式诊断结束。
} // query_present_state结束。

sl::float4x4 matrix_from_rows(const float* rows) { // Rust已把Glam列主序转换成SDK要求的连续行主序。
    sl::float4x4 matrix{}; // 固定四行四列结构不含指针。
    std::memcpy(matrix.row, rows, sizeof(matrix.row)); // 复制全部十六个有限f32。
    return matrix; // 返回独立SDK矩阵值。
} // matrix_from_rows结束。

struct Bl5FgNativeTexture { // 与Rust `NativeTexture`逐字段一致的Vulkan标签描述。
    uint64_t image; // VkImage位模式。
    uint64_t view; // VkImageView位模式。
    uint32_t layout; // 当前VkImageLayout。
    uint32_t width; // 有效宽度。
    uint32_t height; // 有效高度。
    uint32_t format; // VkFormat数值。
    uint32_t mipLevels; // 完整mip数量。
    uint32_t arrayLayers; // 完整数组层数。
    uint32_t usage; // VkImageUsageFlags。
    uint32_t aspectMask; // VkImageAspectFlags。
}; // Bl5FgNativeTexture结束。

struct Bl5FgNativeConstants { // 与Rust `NativeConstants`逐字段一致的相机快照。
    float cameraViewToClip[16]; // 行主序当前投影。
    float clipToCameraView[16]; // 行主序逆投影。
    float clipToPrevClip[16]; // 行主序当前到上一帧裁剪变换。
    float prevClipToClip[16]; // 行主序上一帧到当前裁剪变换。
    float jitterOffset[2]; // 像素抖动。
    float mvecScale[2]; // 运动矢量归一化比例。
    float cameraPos[3]; // 世界位置。
    float cameraUp[3]; // 世界上方向。
    float cameraRight[3]; // 世界右方向。
    float cameraForward[3]; // 世界前方向。
    float cameraNear; // 近裁面。
    float cameraFar; // 远裁面。
    float cameraFov; // 垂直FOV弧度。
    float cameraAspectRatio; // 宽高比。
    uint8_t reset; // 是否重置历史。
    uint8_t padding[3]; // 显式ABI填充。
}; // Bl5FgNativeConstants结束。

sl::Resource make_texture_resource(const Bl5FgNativeTexture& texture) { // 把只读wgpu句柄描述转换成SDK资源值。
    sl::Resource resource{ // Streamline不会取得Vulkan对象所有权。
        sl::ResourceType::eTex2d, // 三项输入均为二维图像。
        reinterpret_cast<void*>(static_cast<uintptr_t>(texture.image)), // VkImage按官方void*字段传递。
        nullptr, // wgpu内存分配不向外暴露，普通已绑定图像允许为空。
        reinterpret_cast<void*>(static_cast<uintptr_t>(texture.view)), // Vulkan路径必须提供匹配视图。
        texture.layout // 标签命令处的真实VkImageLayout。
    }; // 基础资源构造结束。
    resource.width = texture.width; // 插件资源描述宽度。
    resource.height = texture.height; // 插件资源描述高度。
    resource.nativeFormat = texture.format; // VkFormat原值。
    resource.mipLevels = texture.mipLevels; // 当前首期为一。
    resource.arrayLayers = texture.arrayLayers; // 当前首期为一。
    resource.usage = texture.usage; // VkImageUsageFlags原值。
    return resource; // 返回同步SDK调用期间使用的栈值。
} // make_texture_resource结束。

} // 匿名命名空间结束。

extern "C" void bl5_fg_copy_last_error(char* output, size_t capacity) { // Rust通过自有缓冲取得线程安全诊断副本。
    if (output == nullptr || capacity == 0) { // 拒绝无效输出范围。
        return; // 不写入未知内存。
    } // 输出范围判断结束。
    const std::scoped_lock lock(g_error_mutex); // 与错误写入互斥，正常帧无锁。
    std::snprintf(output, capacity, "%s", g_error.data()); // 始终写入NUL结尾且自动截断。
} // bl5_fg_copy_last_error结束。

extern "C" int32_t bl5_fg_bootstrap(const wchar_t* runtime_dir, const wchar_t* upscaling_dir, uint32_t application_id, const char* project_id, bool verbose) { // 必须在任何Vulkan调用前提供完整组合目录。
    if (g_initialized.load(std::memory_order_acquire)) { // 重复安装插件不应再次初始化Streamline。
        return static_cast<int32_t>(sl::Result::eOk); // 幂等成功。
    } // 重复初始化判断结束。
    if (runtime_dir == nullptr || runtime_dir[0] == L'\0' || upscaling_dir == nullptr || upscaling_dir[0] == L'\0' || project_id == nullptr || project_id[0] == '\0') { // 项目身份与两个隔离目录均不可缺失。
        set_error("missing Streamline runtime/application identity", static_cast<int32_t>(sl::Result::eErrorInvalidParameter)); // 不借用样例ID。
        return g_last_result.load(std::memory_order_acquire); // 返回结构化阻断。
    } // 参数合法性判断结束。
    const std::wstring directory(runtime_dir); // 保存绝对UTF-16插件目录。
    for (const wchar_t* file_name : kRuntimeFiles) { // 运行时逐个验证NVIDIA签名与签名者。
        const std::wstring path = directory + L"\\" + file_name; // 不接受外部搜索路径。
        const bool standard_nvidia = std::wcscmp(file_name, L"NvLowLatencyVk.dll") == 0 || std::wcscmp(file_name, L"nvngx_dlssg.dll") == 0; // 只有这两项普通厂商依赖不采用SL专属双签。
        const bool signed_correctly = standard_nvidia ? bl5_security::verify_nvidia_authenticode(path.c_str()) : sl::security::verifyEmbeddedSignature(path.c_str()); // 每项都验证可信签名，五个sl.*保持官方专属公钥检查。
        if (!signed_correctly) { // 验证策略由固定文件名决定，不能在双签失败后回退普通签名。
            char stage[192]{}; // 固定启动期缓冲，明确是哪项官方运行库失败。
            std::snprintf(stage, sizeof(stage), "NVIDIA signature verification failed: %ls", file_name); // 白名单文件名均为ASCII，不输出用户路径。
            set_error(stage, static_cast<int32_t>(sl::Result::eErrorInvalidIntegration)); // 保留严格校验并公开具体文件。
            return g_last_result.load(std::memory_order_acquire); // 禁止半套模块继续。
        } // 单文件验签结束。
    } // 运行时签名白名单遍历结束。
    for (const wchar_t* file_name : {L"nvngx_dlss.dll", L"nvngx_dlssd.dll"}) { // 首次NGX初始化同时看见SR/RR，两项仍由独立目录持有。
        const std::wstring path = std::wstring(upscaling_dir) + L"\\" + file_name; // 不搜索整个SDK。
        if (!bl5_security::verify_nvidia_authenticode(path.c_str())) { // 标准NVIDIA签名与Rust固定哈希共同保护组合身份。
            char stage[192]{}; // 保留具体失败文件。
            std::snprintf(stage, sizeof(stage), "NVIDIA upscaling signature verification failed: %ls", file_name); // 白名单ASCII名称。
            set_error(stage, static_cast<int32_t>(sl::Result::eErrorInvalidIntegration)); // 不用不完整组合初始化NGX。
            return g_last_result.load(std::memory_order_acquire); // 普通Vulkan回退。
        } // 单项组合依赖验签结束。
    } // SR/RR完整身份检查结束。
    const std::wstring interposer_path = directory + L"\\sl.interposer.dll"; // Vulkan代理绝对路径。
    g_interposer = LoadLibraryExW(interposer_path.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32); // 依赖只从已验签DLL目录和系统目录解析。
    if (g_interposer == nullptr) { // 加载失败时保留Win32错误码。
        set_error("LoadLibraryExW(sl.interposer.dll)", static_cast<int32_t>(GetLastError())); // 便于区分依赖缺失。
        return g_last_result.load(std::memory_order_acquire); // 不继续解析空模块。
    } // interposer加载判断结束。
    if (!load_core(g_sl_init, "slInit") || !load_core(g_sl_shutdown, "slShutdown") || !load_core(g_sl_is_feature_supported, "slIsFeatureSupported") || !load_core(g_sl_get_feature_function, "slGetFeatureFunction") || !load_core(g_sl_get_new_frame_token, "slGetNewFrameToken") || !load_core(g_sl_set_constants, "slSetConstants") || !load_core(g_sl_set_tag_for_frame, "slSetTagForFrame")) { // 核心ABI必须一次完整解析。
        return g_last_result.load(std::memory_order_acquire); // load_core已经发布具体缺失导出。
    } // 核心函数解析结束。
    const sl::Feature features[] = {sl::kFeaturePCL, sl::kFeatureReflex, sl::kFeatureDLSS_G}; // Reflex/PCL是DLSS-G必需的同帧契约。
    const wchar_t* plugin_paths[] = {directory.c_str(), upscaling_dir}; // NGX首次初始化必须能发现FG与SR/RR；每个目录仍保持独立白名单。
    sl::Preferences preferences{}; // 从SDK默认结构开始，再显式移除OTA路径。
    preferences.showConsole = verbose; // 开发构建可见官方日志，release保持关闭。
    preferences.logLevel = verbose ? sl::LogLevel::eVerbose : sl::LogLevel::eDefault; // 不在稳定帧执行自有格式化。
    preferences.pathsToPlugins = plugin_paths; // 禁止从可执行目录或下载缓存猜插件。
    preferences.numPathsToPlugins = 2; // 不新增SL插件，只提供同一项目实际使用的NGX模块路径。
    preferences.pathToLogsAndData = nullptr; // 默认不在用户目录留下无界日志。
    wchar_t diagnostic[2]{}; // 只接受当前进程显式指定的1，不持久化用户环境。
    g_diagnostic = verbose && GetEnvironmentVariableW(L"BL5_STREAMLINE_DIAGNOSTIC", diagnostic, 2) == 1 && diagnostic[0] == L'1'; // 固定启动期开关，稳定帧不查询环境。
    if (g_diagnostic) { preferences.logMessageCallback = diagnostic_log; } // 开发烟雾需要SDK原始日志才能判断未插帧原因。
    preferences.flags = sl::PreferenceFlags::eDisableCLStateTracking | sl::PreferenceFlags::eUseFrameBasedResourceTagging; // 全部Vulkan入口已由官方代理接管，不再声明另一套手工挂钩；仍禁用OTA并使用帧标签。
    preferences.featuresToLoad = features; // 只加载首期三项功能。
    preferences.numFeaturesToLoad = static_cast<uint32_t>(std::size(features)); // 固定功能数量。
    preferences.applicationId = application_id; // 有NVIDIA分配的数字ID时优先使用，否则为零并走Project ID。
    preferences.engine = sl::EngineType::eCustom; // Bevy/wgpu属于自定义引擎接入。
    preferences.engineVersion = application_id == 0 ? "bevy-0.20.1-wgpu-30.0.1" : nullptr; // 数字ID路径必须清空版本，避免SDK优先选择Project ID。
    preferences.projectId = application_id == 0 ? project_id : nullptr; // 缺省数字ID时使用已有唯一项目UUID；绝不借用样例身份。
    preferences.renderAPI = sl::RenderAPI::eVulkan; // 能力需求必须按Vulkan解析。
    const sl::Result result = g_sl_init(preferences, sl::kSDKVersion); // 初始化发生在ash加载任何Vulkan函数之前。
    g_last_result.store(static_cast<int32_t>(result), std::memory_order_release); // 发布官方初始化结果。
    if (result != sl::Result::eOk) { // 插件或身份失败时不登记Vulkan代理。
        set_error("slInit", static_cast<int32_t>(result)); // 保留结果码与阶段。
        return static_cast<int32_t>(result); // Rust将安全回到原生Vulkan入口。
    } // 初始化结果判断结束。
    g_initialized.store(true, std::memory_order_release); // 只有完整slInit成功才允许wgpu使用interposer入口。
    clear_error(); // 清除旧诊断。
    return static_cast<int32_t>(sl::Result::eOk); // 前置引导成功。
} // bl5_fg_bootstrap结束。

extern "C" int32_t bl5_fg_attach_vulkan(uint64_t physical_device) { // wgpu设备创建后完成能力与插件函数绑定。
    if (!g_initialized.load(std::memory_order_acquire) || physical_device == 0) { // 前置引导或原生句柄无效时拒绝附着。
        set_error("Streamline attach before initialization", static_cast<int32_t>(sl::Result::eErrorNotInitialized)); // 明确生命周期错误。
        return g_last_result.load(std::memory_order_acquire); // 返回阻断。
    } // 前置条件判断结束。
    sl::AdapterInfo adapter{}; // Vulkan直接提供物理设备句柄，不依赖LUID转换。
    adapter.vkPhysicalDevice = reinterpret_cast<void*>(static_cast<uintptr_t>(physical_device)); // 保持VkPhysicalDevice位模式。
    const sl::Result reflex_support = g_sl_is_feature_supported(sl::kFeatureReflex, adapter); // DLSS-G开启前Reflex必须可用。
    if (reflex_support != sl::Result::eOk) { // 不支持时仍允许游戏继续普通渲染。
        set_error("Reflex unsupported", static_cast<int32_t>(reflex_support)); // 设置页显示驱动/硬件原因。
        return static_cast<int32_t>(reflex_support); // 不解析半套功能。
    } // Reflex能力判断结束。
    const sl::Result dlssg_support = g_sl_is_feature_supported(sl::kFeatureDLSS_G, adapter); // 包含GPU、驱动、OS与HAGS检查。
    if (dlssg_support != sl::Result::eOk) { // 能力不足时保留请求但实际回退。
        set_error("DLSS Frame Generation unsupported", static_cast<int32_t>(dlssg_support)); // 官方结果码透传。
        return static_cast<int32_t>(dlssg_support); // 不宣称支持。
    } // DLSS-G能力判断结束。
    if (!load_feature(sl::kFeatureDLSS_G, g_dlssg_set_options, "slDLSSGSetOptions") || !load_feature(sl::kFeatureDLSS_G, g_dlssg_get_state, "slDLSSGGetState") || !load_feature(sl::kFeatureReflex, g_reflex_set_options, "slReflexSetOptions") || !load_feature(sl::kFeatureReflex, g_reflex_sleep, "slReflexSleep") || !load_feature(sl::kFeaturePCL, g_pcl_set_marker, "slPCLSetMarker")) { // 五个运行入口缺一不可。
        return g_last_result.load(std::memory_order_acquire); // load_feature已发布具体错误。
    } // 插件函数解析结束。
    if (!load_feature(sl::kFeatureReflex, g_reflex_get_state, "slReflexGetState")) { return g_last_result.load(std::memory_order_acquire); } // 独立读取真正驱动状态。
    sl::ReflexOptions reflex_options{}; // 首期固定低延迟，不默认Boost或帧率上限。
    reflex_options.mode = sl::ReflexMode::eLowLatency; // DLSS-G运行时硬依赖Reflex激活。
    const sl::Result reflex_result = g_reflex_set_options(reflex_options); // 设备就绪后设置一次，稳定帧不重复写。
    if (reflex_result != sl::Result::eOk) { // 设置失败不得继续开FG。
        set_error("slReflexSetOptions", static_cast<int32_t>(reflex_result)); // 发布官方结果。
        return static_cast<int32_t>(reflex_result); // 回退普通Present。
    } // Reflex设置结果结束。
    g_options = sl::DLSSGOptions{}; // 首次附着保持关闭，等待玩家持久设置请求。
    g_options.mode = sl::DLSSGMode::eOff; // DLL加载不等于插帧生效。
    g_options.numFramesToGenerate = 1; // 首期只允许2x，不启用RTX 50多帧生成。
    g_options.queueParallelismMode = sl::DLSSGQueueParallelismMode::eBlockPresentingClientQueue; // 避免宿主额外跨队列fence所有权。
    const sl::Result options_result = g_dlssg_set_options(g_viewport, g_options); // 在Present前建立明确关闭状态。
    if (options_result != sl::Result::eOk) { // 关闭态配置失败同样说明集成不可用。
        set_error("slDLSSGSetOptions(off)", static_cast<int32_t>(options_result)); // 发布具体阶段。
        return static_cast<int32_t>(options_result); // 禁止半初始化。
    } // 初始选项结果结束。
    g_attached.store(true, std::memory_order_release); // 能力、函数、Reflex和关闭态选项全部成功。
    g_last_result.store(static_cast<int32_t>(sl::Result::eOk), std::memory_order_release); // 清理旧能力错误。
    clear_error(); // 清空诊断文本。
    return static_cast<int32_t>(sl::Result::eOk); // 设备附着完成。
} // bl5_fg_attach_vulkan结束。

extern "C" int32_t bl5_fg_set_enabled(bool enabled, uint32_t width, uint32_t height, uint32_t color_format, uint32_t depth_width, uint32_t depth_height, uint32_t depth_format, uint32_t motion_format, uint32_t hudless_format, uint32_t ui_format) { // 独立UI格式与其他完整输入一起参与选项换代。
    g_presentation.invalidate(); // 仅在启停或规格变更调用，禁止跨窗口/模式代次采样。
    if (!g_attached.load(std::memory_order_acquire) || g_dlssg_set_options == nullptr) { // 能力未就绪时不能修改插件状态。
        set_error("DLSS-G options before attach", static_cast<int32_t>(sl::Result::eErrorNotInitialized)); // 保留请求但实际回退。
        return g_last_result.load(std::memory_order_acquire); // 返回生命周期错误。
    } // 附着状态判断结束。
    sl::DLSSGOptions options{}; // 每次尺寸或开关变化重新提交完整快照。
    options.mode = enabled ? sl::DLSSGMode::eOn : sl::DLSSGMode::eOff; // 只使用固定On/Off。
    options.numFramesToGenerate = 1; // 1个中间帧等于2x总呈现倍率。
    options.colorWidth = width; // 最终交换链宽度。
    options.colorHeight = height; // 最终交换链高度。
    options.colorBufferFormat = color_format; // VkFormat原值。
    options.mvecDepthWidth = depth_width; // 内部运动/深度宽度。
    options.mvecDepthHeight = depth_height; // 内部运动/深度高度。
    options.depthBufferFormat = depth_format; // 深度VkFormat。
    options.mvecBufferFormat = motion_format; // 运动矢量VkFormat。
    options.hudLessBufferFormat = hudless_format; // HUD-less颜色VkFormat。
    options.uiBufferFormat = ui_format; // 来自同尺寸真实预乘UI纹理，不从最终颜色差猜测Alpha。
    options.onErrorCallback = on_api_error; // 捕获SDK后台队列错误，不能仅看同步setOptions返回值。
    options.queueParallelismMode = sl::DLSSGQueueParallelismMode::eBlockPresentingClientQueue; // 首期选择安全同步模式。
    options.enableUserInterfaceRecomposition = sl::Boolean::eTrue; // 完整HUD-less与UI分别插值后重组；缺任一输入的帧由宿主关闭FG。
    const sl::Result result = g_dlssg_set_options(g_viewport, options); // 选项在下一次Present生效。
    g_last_result.store(static_cast<int32_t>(result), std::memory_order_release); // 发布调用结果。
    if (result != sl::Result::eOk) { // 失败时不得保留假有效状态。
        g_requested.store(false, std::memory_order_release); // 实际插件请求归零。
        g_effective.store(false, std::memory_order_release); // 状态页显示回退。
        set_error("slDLSSGSetOptions", static_cast<int32_t>(result)); // 保存具体错误。
        return static_cast<int32_t>(result); // 调用者维持普通Present。
    } // 选项设置结果结束。
    g_options = options; // 保存最近成功快照供关闭与诊断。
    if (enabled && g_diagnostic && g_reflex_get_state) { sl::ReflexState reflex{}; const auto result = g_reflex_get_state(reflex); char message[192]{}; std::snprintf(message, sizeof(message), "Host Reflex result=%d lowLatencyAvailable=%u", static_cast<int32_t>(result), reflex.lowLatencyAvailable); diagnostic_log(sl::LogType::eInfo, message); } // 只在启用边界诊断，避免逐帧查询完整延迟报告。
    if (g_diagnostic) { char message[256]{}; std::snprintf(message, sizeof(message), "Host set_enabled=%u frame=%u complete=%llu outputFormat=%u uiFormat=%u", enabled, g_driver_frame.load(), g_complete_frame.load(), color_format, ui_format); diagnostic_log(sl::LogType::eInfo, message); } // 仅显式诊断记录稀有选项变更，捕获启停的真实来源。
    g_requested.store(enabled, std::memory_order_release); // 发布真实插件请求。
    if (!enabled) { // 关闭无需等待下一帧状态确认。
        g_effective.store(false, std::memory_order_release); // 立即停止宣称插帧。
    } // 关闭态处理结束。
    clear_error(); // 清理旧错误。
    return static_cast<int32_t>(sl::Result::eOk); // 配置成功。
} // bl5_fg_set_enabled结束。

extern "C" int32_t bl5_fg_set_constants(uint32_t frame_index, const Bl5FgNativeConstants* input) { // RenderApp每个有效玩法帧提交公共常量。
    if (!g_attached.load(std::memory_order_acquire) || g_sl_set_constants == nullptr || input == nullptr) { // 生命周期与输入缺一不可。
        set_error("Streamline constants before attach", static_cast<int32_t>(sl::Result::eErrorNotInitialized)); // 明确拒绝假输入。
        return g_last_result.load(std::memory_order_acquire); // 普通Present继续。
    } // 前置条件判断结束。
    sl::FrameToken* token = frame_token(frame_index); // 与资源、Reflex和Present共用显式帧号。
    if (token == nullptr) { // 令牌错误已由辅助函数发布。
        return g_last_result.load(std::memory_order_acquire); // 跳过本帧FG输入。
    } // 令牌判断结束。
    sl::Constants constants{}; // 从SDK版本化默认结构开始填满所有必需字段。
    constants.cameraViewToClip = matrix_from_rows(input->cameraViewToClip); // 无抖动投影。
    constants.clipToCameraView = matrix_from_rows(input->clipToCameraView); // 无抖动逆投影。
    constants.clipToPrevClip = matrix_from_rows(input->clipToPrevClip); // 时间变换。
    constants.prevClipToClip = matrix_from_rows(input->prevClipToClip); // 逆时间变换。
    constants.jitterOffset = {input->jitterOffset[0], input->jitterOffset[1]}; // 当前Bevy预通道为零抖动运动。
    constants.mvecScale = {input->mvecScale[0], input->mvecScale[1]}; // 已归一化UV位移使用一比一。
    constants.cameraPos = {input->cameraPos[0], input->cameraPos[1], input->cameraPos[2]}; // 世界位置。
    constants.cameraUp = {input->cameraUp[0], input->cameraUp[1], input->cameraUp[2]}; // 世界上方向。
    constants.cameraRight = {input->cameraRight[0], input->cameraRight[1], input->cameraRight[2]}; // 世界右方向。
    constants.cameraFwd = {input->cameraForward[0], input->cameraForward[1], input->cameraForward[2]}; // 世界前方向。
    constants.cameraNear = input->cameraNear; // 近裁面。
    constants.cameraFar = input->cameraFar; // 远裁面。
    constants.cameraFOV = input->cameraFov; // 垂直视场角。
    constants.cameraAspectRatio = input->cameraAspectRatio; // 视图宽高比。
    constants.cameraPinholeOffset = {0.0f, 0.0f}; // 普通居中透视相机无偏轴位移，不能遗留SDK无效哨兵。
    constants.depthInverted = sl::Boolean::eTrue; // Bevy 3D使用反向Z，近处值更大。
    constants.cameraMotionIncluded = sl::Boolean::eTrue; // 背景预通道补齐旋转，网格预通道包含相机与对象运动。
    constants.motionVectors3D = sl::Boolean::eFalse; // Rg16Float保存二维UV位移。
    constants.reset = input->reset != 0 ? sl::Boolean::eTrue : sl::Boolean::eFalse; // 首帧和断续帧清历史。
    if (g_diagnostic) { static uint32_t previous_reset = ~uint32_t{0}; if (previous_reset != input->reset) { previous_reset = input->reset; char message[128]{}; std::snprintf(message, sizeof(message), "Host constants reset=%u frame=%u driver=%u", input->reset, frame_index, g_driver_frame.load()); diagnostic_log(sl::LogType::eInfo, message); } } // 只记录reset变化，识别持续清历史导致不插帧。
    constants.orthographicProjection = sl::Boolean::eFalse; // 首期严格只接玩法透视相机。
    constants.motionVectorsDilated = sl::Boolean::eFalse; // Bevy预通道未执行厂商膨胀。
    constants.motionVectorsJittered = sl::Boolean::eFalse; // 着色器显式使用unjittered矩阵。
    const sl::Result result = g_sl_set_constants(constants, *token, g_viewport); // 常量在资源标签和Present前提交。
    g_last_result.store(static_cast<int32_t>(result), std::memory_order_release); // 发布官方结果。
    if (result != sl::Result::eOk) { // 不允许缺常量时继续宣称FG有效。
        g_effective.store(false, std::memory_order_release); // 本帧回退事实。
        set_error("slSetConstants", static_cast<int32_t>(result)); // 记录明确阶段。
    } // 常量结果判断结束。
    return static_cast<int32_t>(result); // 返回官方结果。
} // bl5_fg_set_constants结束。

extern "C" int32_t bl5_fg_tag_inputs(uint32_t frame_index, uint64_t command_buffer, const Bl5FgNativeTexture* depth_input, const Bl5FgNativeTexture* motion_input, const Bl5FgNativeTexture* hudless_input, uint32_t output_width, uint32_t output_height) { // 同一命令缓冲标签完整FG输入。
    if (!g_attached.load(std::memory_order_acquire) || g_sl_set_tag_for_frame == nullptr || command_buffer == 0 || depth_input == nullptr || motion_input == nullptr || hudless_input == nullptr || output_width == 0 || output_height == 0) { // 严格拒绝残缺帧。
        set_error("invalid DLSS-G frame inputs", static_cast<int32_t>(sl::Result::eErrorMissingInputParameter)); // 不复用上一帧标签。
        return g_last_result.load(std::memory_order_acquire); // 调用方会立即关闭本帧FG。
    } // 输入合法性判断结束。
    sl::FrameToken* token = frame_token(frame_index); // 与本帧常量和Reflex使用同一令牌。
    if (token == nullptr) { // 令牌失败已发布诊断。
        return g_last_result.load(std::memory_order_acquire); // 不触碰资源。
    } // 令牌判断结束。
    sl::Resource depth = make_texture_resource(*depth_input); // 当前反向Z深度。
    sl::Resource motion = make_texture_resource(*motion_input); // 当前归一化运动矢量。
    sl::Resource hudless = make_texture_resource(*hudless_input); // 当前UI前最终颜色副本。
    const sl::Extent depth_extent{0, 0, depth_input->width, depth_input->height}; // 深度有效子矩形。
    const sl::Extent motion_extent{0, 0, motion_input->width, motion_input->height}; // 运动矢量有效子矩形。
    const sl::Extent output_extent{0, 0, output_width, output_height}; // HUD-less颜色覆盖完整输出范围。
    sl::ResourceTag tags[] = { // 统一一次调用，避免同帧标签身份漂移。
        {&depth, sl::kBufferTypeDepth, sl::ResourceLifecycle::eOnlyValidNow, &depth_extent}, // 让Streamline在当前命令缓冲内取得稳定副本。
        {&motion, sl::kBufferTypeMotionVectors, sl::ResourceLifecycle::eOnlyValidNow, &motion_extent}, // wgpu后续可安全复用缓存纹理。
        {&hudless, sl::kBufferTypeHUDLessColor, sl::ResourceLifecycle::eOnlyValidNow, &output_extent}, // UI随后写主目标，不污染该副本；全屏输出无需额外Backbuffer子区域标签。
    }; // 完整标签集合结束。
    auto* native_command_buffer = reinterpret_cast<sl::CommandBuffer*>(static_cast<uintptr_t>(command_buffer)); // VkCommandBuffer是分发句柄。
    const sl::Result result = g_sl_set_tag_for_frame(*token, g_viewport, tags, static_cast<uint32_t>(std::size(tags)), native_command_buffer); // 插件在当前记录流维护必要状态与副本。
    g_last_result.store(static_cast<int32_t>(result), std::memory_order_release); // 发布官方结果。
    if (result != sl::Result::eOk) { // 任何标签失败都阻止假有效状态。
        g_effective.store(false, std::memory_order_release); // 本帧普通Present。
        set_error("slSetTagForFrame", static_cast<int32_t>(result)); // 保存精确阶段。
    } // 标签结果判断结束。
    return static_cast<int32_t>(result); // 返回官方结果。
} // bl5_fg_tag_inputs结束。

extern "C" int32_t bl5_fg_tag_ui(uint32_t frame_index, uint64_t command_buffer, const Bl5FgNativeTexture* input) { // 在全部UI绘制完成后追加同帧最后一项标签。
    if (!g_attached.load(std::memory_order_acquire) || g_sl_set_tag_for_frame == nullptr || command_buffer == 0 || input == nullptr || input->width != g_options.colorWidth || input->height != g_options.colorHeight || input->format != g_options.uiBufferFormat) { // 标签必须匹配当前已预热选项的完整输出规格。
        set_error("invalid DLSS-G UI input", static_cast<int32_t>(sl::Result::eErrorMissingInputParameter)); // 不允许SDK复用上一帧UI。
        return g_last_result.load(std::memory_order_acquire); // 宿主负责关闭FG并恢复真实UI。
    } // 完整规格检查结束。
    sl::FrameToken* token = frame_token(frame_index); // 与深度、运动和HUD-less共用当前令牌。
    if (token == nullptr) { return g_last_result.load(std::memory_order_acquire); } // 无令牌时拒绝借用。
    sl::Resource ui = make_texture_resource(*input); // 独立预乘RGBA快照只读。
    const sl::Extent extent{0, 0, input->width, input->height}; // UI覆盖完整输出范围。
    sl::ResourceTag tag{&ui, sl::kBufferTypeUIColorAndAlpha, sl::ResourceLifecycle::eOnlyValidNow, &extent}; // 当前命令缓冲立即复制，后续真实帧还原不污染SDK输入。
    const sl::Result result = g_sl_set_tag_for_frame(*token, g_viewport, &tag, 1, reinterpret_cast<sl::CommandBuffer*>(command_buffer)); // 无主线程等待或跨帧原始资源指针。
    if (result == sl::Result::eOk && frame_index == g_driver_frame.load(std::memory_order_acquire)) { g_complete_frame.store(frame_index, std::memory_order_release); } // SDK最后一项输入成功且仍属于当前driver帧才完成。
    else { g_complete_frame.store(~uint64_t{0}, std::memory_order_release); } // 迟到或失败输入不得许可Present。
    if (result != sl::Result::eOk) { set_error("slSetTagForFrame(UI)", static_cast<int32_t>(result)); } // 暴露具体UI登记失败。
    return static_cast<int32_t>(result); // 只有成功才允许当前帧开启插值。
} // 独立UI标签结束。

extern "C" int32_t bl5_fg_clear_tags(uint32_t frame_index) { // 菜单、加载或输入缺失时撤销上一玩法帧资源。
    g_complete_frame.store(~uint64_t{0}, std::memory_order_release); // 即使SDK清理失败也撤销宿主完整帧事实。
    if (!g_attached.load(std::memory_order_acquire) || g_sl_set_tag_for_frame == nullptr) { // 未附着时无需SDK调用。
        return g_last_result.load(std::memory_order_acquire); // 保留现有阻断。
    } // 生命周期判断结束。
    sl::FrameToken* token = frame_token(frame_index); // 清理同样属于明确宿主帧。
    if (token == nullptr) { // 令牌错误已发布。
        return g_last_result.load(std::memory_order_acquire); // 安全返回。
    } // 令牌判断结束。
    sl::ResourceTag tags[] = { // 空资源标签明确撤销全部DLSS-G输入类型。
        {nullptr, sl::kBufferTypeDepth, sl::ResourceLifecycle::eValidUntilPresent}, // 撤销深度。
        {nullptr, sl::kBufferTypeMotionVectors, sl::ResourceLifecycle::eValidUntilPresent}, // 撤销运动矢量。
        {nullptr, sl::kBufferTypeHUDLessColor, sl::ResourceLifecycle::eValidUntilPresent}, // 撤销无HUD颜色。
        {nullptr, sl::kBufferTypeUIColorAndAlpha, sl::ResourceLifecycle::eValidUntilPresent}, // 失败或切图不得复用旧UI。
    }; // 空标签集合结束。
    const sl::Result result = g_sl_set_tag_for_frame(*token, g_viewport, tags, static_cast<uint32_t>(std::size(tags)), nullptr); // 空标签不需要命令缓冲。
    if (result != sl::Result::eOk) { // 清理失败仍需公开，避免后续误判。
        set_error("slSetTagForFrame(clear)", static_cast<int32_t>(result)); // 发布官方结果。
    } // 清理结果判断结束。
    g_effective.store(false, std::memory_order_release); // 没有有效输入时绝不宣称插帧。
    return static_cast<int32_t>(result); // 返回官方结果。
} // bl5_fg_clear_tags结束。

extern "C" void bl5_fg_set_driver_frame(uint32_t frame_index) { // RenderApp在真正提交前发布帧号。
    g_complete_frame.store(~uint64_t{0}, std::memory_order_release); // 每次driver帧从未完成开始，缺主视图不能沿用旧UI。
    g_driver_frame.store(frame_index, std::memory_order_release); // wgpu底层回调随后读取同一编号。
} // bl5_fg_set_driver_frame结束。

extern "C" int32_t bl5_fg_sleep(uint32_t frame_index) { // Simulation开始前执行Reflex低延迟sleep。
    sl::FrameToken* token = frame_token(frame_index); // 所有阶段使用相同显式编号。
    if (token == nullptr || g_reflex_sleep == nullptr) { // 未附着时安全跳过并返回当前阻断。
        return g_last_result.load(std::memory_order_acquire); // 不影响普通游戏循环。
    } // Reflex入口判断结束。
    const sl::Result result = g_reflex_sleep(*token); // SDK要求即使Reflex关闭也持续调用；本项目固定低延迟开启。
    if (result != sl::Result::eOk) { // sleep失败保留诊断但不阻塞主线程。
        set_error("slReflexSleep", static_cast<int32_t>(result)); // 发布错误。
    } // sleep结果结束。
    return static_cast<int32_t>(result); // 返回官方结果。
} // bl5_fg_sleep结束。

extern "C" int32_t bl5_fg_mark(uint32_t frame_index, uint32_t marker) { // 主世界提交Simulation标记。
    sl::FrameToken* token = frame_token(frame_index); // 获取同一帧令牌。
    if (token == nullptr || g_pcl_set_marker == nullptr || marker >= static_cast<uint32_t>(sl::PCLMarker::eMaximum)) { // 拒绝无效阶段。
        return g_last_result.load(std::memory_order_acquire); // 安全跳过。
    } // 标记前置条件判断结束。
    const sl::Result result = g_pcl_set_marker(static_cast<sl::PCLMarker>(marker), *token); // 调用SDK线程安全标记入口。
    if (result != sl::Result::eOk) { // 标记失败记录一次最新状态。
        set_error("slPCLSetMarker", static_cast<int32_t>(result)); // 发布错误。
    } // 标记结果结束。
    return static_cast<int32_t>(result); // 返回官方结果。
} // bl5_fg_mark结束。

extern "C" void bl5_fg_on_render_submit_start() { if (g_diagnostic) { g_diagnostic_submits.fetch_add(1, std::memory_order_relaxed); } mark_current(sl::PCLMarker::eRenderSubmitStart); } // 紧贴wgpu vkQueueSubmit前，显式诊断同时核实每帧提交次数。
extern "C" void bl5_fg_on_render_submit_end() { mark_current(sl::PCLMarker::eRenderSubmitEnd); } // 紧贴wgpu vkQueueSubmit后。
extern "C" void bl5_fg_on_present_start() { // 紧贴Streamline代理vkQueuePresent前，覆盖主视图根本没有运行的缺帧情况。
    const uint32_t frame = g_driver_frame.load(std::memory_order_acquire); // RenderApp已发布当前提交身份。
    if (g_requested.load(std::memory_order_acquire) && g_complete_frame.load(std::memory_order_acquire) != frame) { // 预热本就Off，不把合法预热当作缺帧错误。
        g_presentation.invalidate(); // 缺帧引发自动关闭也必须撤销此前的呈现采样区间。
        if (g_diagnostic) { char message[192]{}; std::snprintf(message, sizeof(message), "Host Present denied frame=%u complete=%llu", frame, g_complete_frame.load()); diagnostic_log(sl::LogType::eWarn, message); } // 避免下一帧成功覆盖错误后丢失最初缺帧身份。
        sl::DLSSGOptions options = g_options; // Host Render系统和队列Present回调按提交边界串行，复用最近完整规格。
        options.mode = sl::DLSSGMode::eOff; // 必须先关闭SDK再进行本帧Present。
        const sl::Result result = g_dlssg_set_options != nullptr ? g_dlssg_set_options(g_viewport, options) : sl::Result::eErrorNotInitialized; // 不通过未解析入口调用。
        if (result == sl::Result::eOk) { g_options = options; } // 仅保存SDK实际接受的关闭快照。
        g_requested.store(false, std::memory_order_release); // Rust可从原生requested=false识别安全门自动关闭并重新预热。
        g_effective.store(false, std::memory_order_release); // 不宣称缺输入时仍插帧。
        g_frames_presented.store(1, std::memory_order_release); // 撤销上一帧倍率事实。
        bl5_fg_clear_tags(frame); // 四项空标签共同清理；即使关闭失败也不复用旧帧资源。
        set_error("DLSS-G Present blocked: current frame has no complete UI input", static_cast<int32_t>(result == sl::Result::eOk ? sl::Result::eErrorMissingInputParameter : result)); // 记录缺帧或更具体SDK关闭失败。
    } // 当前帧完整性安全门结束。
    mark_current(sl::PCLMarker::ePresentStart); // 安全门完成后才进入正式Present标记。
} // Present开始回调结束。
extern "C" void bl5_fg_on_present_end() { mark_current(sl::PCLMarker::ePresentEnd); query_present_state(); } // Present后标记并读取一次真实插帧数。
extern "C" void bl5_fg_on_present_failure() { // Surface丢失、过期或设备错误时不能继续显示上一帧2x证据。
    g_presentation.invalidate(); // 失败Present不累计，恢复后重新建立采样基线。
    g_effective.store(false, std::memory_order_release); // 立刻回到普通Present事实。
    g_frames_presented.store(1, std::memory_order_release); // 只保留宿主真实帧基线。
    set_error("Vulkan queue present failed", static_cast<int32_t>(sl::Result::eErrorVulkanAPI)); // 下一成功Present会覆盖官方结果。
} // bl5_fg_on_present_failure结束。

extern "C" void bl5_fg_shutdown() { // 必须在Vulkan设备与实例销毁前由Bevy退出系统调用。
    g_presentation.invalidate(); // 设备退出后不保留有效FPS来源。
    if (!g_initialized.load(std::memory_order_acquire) || g_sl_shutdown == nullptr) { // 未初始化时保持幂等。
        return; // 无SDK调用。
    } // 初始化状态判断结束。
    if (g_dlssg_set_options != nullptr) { // 已附着时先关闭插帧。
        sl::DLSSGOptions off{}; // 构造明确关闭配置。
        off.mode = sl::DLSSGMode::eOff; // 停止异步Present工作。
        g_dlssg_set_options(g_viewport, off); // 退出路径不覆盖先前诊断。
    } // DLSS-G关闭结束。
    const sl::Result shutdown_result = g_sl_shutdown(); // Streamline在宿主Vulkan对象仍有效时释放插件资源。
    if (shutdown_result != sl::Result::eOk) { set_error("slShutdown", static_cast<int32_t>(shutdown_result)); return; } // 失败时保留initialized，禁止外层误认FG已释放并提前关闭宿主NGX。
    g_attached.store(false, std::memory_order_release); // 阻止后续标记。
    g_initialized.store(false, std::memory_order_release); // 记录关闭完成。
    g_requested.store(false, std::memory_order_release); // 清理公开请求事实。
    g_effective.store(false, std::memory_order_release); // 清理公开有效事实。
} // bl5_fg_shutdown结束。

struct Bl5FgNativeStatus { // 与Rustrepr(C)状态快照保持逐字段一致。
    int32_t last_result; // 最近一次官方结果码。
    uint32_t dlssg_status; // DLSSGStatus位集合。
    uint32_t frames_presented; // 最近宿主Present实际输出帧数。
    uint32_t max_generated_frames; // 能力上限中的插帧数。
    uint32_t min_dimension; // 最小支持边长。
    uint64_t estimated_vram; // 可选估算，默认0。
    uint8_t initialized; // slInit是否成功。
    uint8_t attached; // 物理设备是否完成附着。
    uint8_t requested; // 插件是否接受开启请求。
    uint8_t effective; // 最近Present是否真实产生至少2帧。
    uint64_t total_present_count; // 所有可信Present的实际累计帧数。
    uint64_t presentation_epoch; // 启停、规格切换和失败的采样代次。
    uint8_t presentation_ready; // 一次非阻塞读取取得完整可信计数。
}; // C ABI状态结构结束。
static_assert(sizeof(Bl5FgNativeStatus) == 64); // 与Rust固定状态快照大小互证，防止原生写越界。
static_assert(offsetof(Bl5FgNativeStatus, total_present_count) == 40); // 新增64位字段必须保持自然对齐。

extern "C" void bl5_fg_get_status(Bl5FgNativeStatus* output) { // Rust低频复制无锁原生状态。
    if (output == nullptr) { // 防御空输出指针。
        return; // 不写内存。
    } // 输出指针判断结束。
    output->last_result = g_last_result.load(std::memory_order_acquire); // 官方结果码。
    output->dlssg_status = g_dlssg_status.load(std::memory_order_acquire); // 运行时质量位。
    output->frames_presented = g_frames_presented.load(std::memory_order_acquire); // 真实Present倍数证据。
    output->max_generated_frames = g_max_generated_frames.load(std::memory_order_acquire); // 硬件能力上限。
    output->min_dimension = g_min_dimension.load(std::memory_order_acquire); // 分辨率下限。
    output->estimated_vram = g_estimated_vram.load(std::memory_order_acquire); // 当前估算。
    output->initialized = g_initialized.load(std::memory_order_acquire) ? 1 : 0; // 生命周期状态。
    output->attached = g_attached.load(std::memory_order_acquire) ? 1 : 0; // 设备状态。
    output->requested = g_requested.load(std::memory_order_acquire) ? 1 : 0; // 插件请求状态。
    output->effective = g_effective.load(std::memory_order_acquire) ? 1 : 0; // 真实插帧状态。
    const auto presentation = g_presentation.snapshot(); // 单次读取不等待Render线程。
    output->total_present_count = presentation.count; // 与代次同时验证的累计值。
    output->presentation_epoch = presentation.epoch; // 消费者据此拒绝旧采样边界。
    output->presentation_ready = presentation.ready ? 1 : 0; // 混合快照只报告不可用。
} // bl5_fg_get_status结束。
