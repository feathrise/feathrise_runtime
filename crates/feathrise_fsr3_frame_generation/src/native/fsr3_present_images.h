// 本文件为签名FSR交换链提供有限数量的sRGB前缓冲，保留Bevy颜色编码与线性混合语义。
// 【功能设计】wgpu渲染普通sRGB图像；同一游戏队列按原始像素复制到SDK的UNORM图像，再把完成信号交给SDK Present。
// 图像与命令仅在交换链创建时分配；Present不分配、不等待CPU fence，也不改动AMD DLL。
#pragma once // 一个原生翻译单元只定义一次桥接类型。
#include <array> // 所有资源槽位和提交等待阶段均采用固定上限。
#include <cstdint> // 固定大小的图像索引。
#include <vulkan/vulkan.h> // 只使用Vulkan类型和动态函数指针，不引入静态loader依赖。

class Bl5FsrPresentImages { // 由bridge生命周期锁保护；destroy必须在设备工作完成后调用。
public: // bridge调用的有限互操作接口。
    static constexpr uint32_t kMaxImages = 8; // SDK和窗口图像数量必须落在固定预算内。
    static constexpr uint32_t kMaxWaits = 16; // 普通wgpu单窗口Present只需少量二进制等待信号。
    static constexpr VkDeviceSize kMaxMemory = 256ull * 1024 * 1024; // 前缓冲总驻留上限256MiB，超预算直接回退普通交换链。
    VkDeviceSize memory_bytes() const { return memoryBytes_; } // 只读实际分配量，便于记录目标硬件成本。
    VkResult create(VkDevice device, PFN_vkGetDeviceProcAddr resolver, const VkPhysicalDeviceMemoryProperties& properties, const VkSwapchainCreateInfoKHR& original, uint32_t gameQueueFamily, uint32_t count, const VkImage* proxyImages) { // 原规格属于wgpu的sRGB窗口，proxyImages属于SDK的UNORM交换链。
        if (device_ != VK_NULL_HANDLE || device == VK_NULL_HANDLE || resolver == nullptr || proxyImages == nullptr || count == 0 || count > kMaxImages) return VK_ERROR_INITIALIZATION_FAILED; // 拒绝重入、缺入口与无界图像数量。
        if ((original.imageFormat != VK_FORMAT_B8G8R8A8_SRGB && original.imageFormat != VK_FORMAT_R8G8B8A8_SRGB) || original.imageArrayLayers != 1 || original.imageExtent.width == 0 || original.imageExtent.height == 0 || original.imageSharingMode != VK_SHARING_MODE_EXCLUSIVE || (original.imageUsage & VK_IMAGE_USAGE_STORAGE_BIT) != 0 || (original.flags & VK_SWAPCHAIN_CREATE_PROTECTED_BIT_KHR) != 0) return VK_ERROR_FORMAT_NOT_SUPPORTED; // 首期只接普通单层sRGB窗口，不能伪装成HDR、受保护或存储图像。
#define BL5_LOAD_PRESENT_PROC(name) name##_ = reinterpret_cast<PFN_##name>(resolver(device, #name)); if (name##_ == nullptr) return VK_ERROR_INITIALIZATION_FAILED // 加载全部入口后才取得设备和资源所有权。
        BL5_LOAD_PRESENT_PROC(vkCreateImage); // 创建自有sRGB图像。
        BL5_LOAD_PRESENT_PROC(vkDestroyImage); // 释放自有图像。
        BL5_LOAD_PRESENT_PROC(vkGetImageMemoryRequirements); // 查询真实图像内存需求。
        BL5_LOAD_PRESENT_PROC(vkAllocateMemory); // 分配固定数量设备内存。
        BL5_LOAD_PRESENT_PROC(vkFreeMemory); // 配对释放内存。
        BL5_LOAD_PRESENT_PROC(vkBindImageMemory); // 绑定前缓冲内存。
        BL5_LOAD_PRESENT_PROC(vkCreateCommandPool); // 每槽独立命令池。
        BL5_LOAD_PRESENT_PROC(vkDestroyCommandPool); // 连同固定命令缓冲一起释放。
        BL5_LOAD_PRESENT_PROC(vkAllocateCommandBuffers); // 仅初始化阶段分配命令。
        BL5_LOAD_PRESENT_PROC(vkBeginCommandBuffer); // 初始化时录制完整复制。
        BL5_LOAD_PRESENT_PROC(vkEndCommandBuffer); // 初始化时结束录制。
        BL5_LOAD_PRESENT_PROC(vkCmdPipelineBarrier); // 显式维护普通图像布局。
        BL5_LOAD_PRESENT_PROC(vkCmdCopyImage); // 保持编码后的原始像素位，不执行色彩转换。
        BL5_LOAD_PRESENT_PROC(vkCreateSemaphore); // 复制完成后供SDK等待的二进制信号。
        BL5_LOAD_PRESENT_PROC(vkDestroySemaphore); // 配对释放信号。
        BL5_LOAD_PRESENT_PROC(vkQueueSubmit); // 在wgpu游戏队列上串接复制。
#undef BL5_LOAD_PRESENT_PROC // 不把局部加载宏泄漏给bridge。
        device_ = device; // 入口齐全后发布设备，后续任一步失败均可清理。
        count_ = count; // 半初始化清理也扫描完整有界槽位。
        for (uint32_t index = 0; index < count_; ++index) { // 按固定图像数建立完整结果。
            VkResult result = create_slot(slots_[index], original, gameQueueFamily, properties, proxyImages[index]); // 图像索引与SDK Acquire保持严格一一对应。
            if (result != VK_SUCCESS) { destroy(); return result; } // 尚未提交GPU工作，半创建可立即清理。
        } // 所有槽位创建结束。
        return VK_SUCCESS; // 只有完整资源集合才交给wgpu枚举。
    } // create结束。
    void destroy() { // 调用方必须先完成设备idle；此函数自身不阻塞GPU。
        if (device_ == VK_NULL_HANDLE) return; // 空对象或重复销毁安全返回。
        for (Slot& slot : slots_) { // 固定八槽清理同时覆盖半初始化。
            if (slot.pool != VK_NULL_HANDLE) vkDestroyCommandPool_(device_, slot.pool, nullptr); // 先释放引用图像的命令。
            if (slot.done != VK_NULL_HANDLE) vkDestroySemaphore_(device_, slot.done, nullptr); // 不再存在待执行信号与等待。
            if (slot.image != VK_NULL_HANDLE) vkDestroyImage_(device_, slot.image, nullptr); // 自有图像不交给SDK释放。
            if (slot.memory != VK_NULL_HANDLE) vkFreeMemory_(device_, slot.memory, nullptr); // 图像释放后释放独立内存。
            slot = {}; // 清除所有句柄与提交身份。
        } // 槽位清理结束。
        device_ = VK_NULL_HANDLE; // 允许下一次交换链规格重新创建。
        count_ = 0; // 禁止枚举旧图像。
        memoryBytes_ = 0; // 换规格后重新计算预算，不累积旧窗口分配量。
    } // destroy结束。
    VkResult get_images(uint32_t* count, VkImage* images) const { // 与vkGetSwapchainImagesKHR的两次枚举约定一致。
        if (count == nullptr || device_ == VK_NULL_HANDLE) return VK_ERROR_INITIALIZATION_FAILED; // 未完成创建不能暴露半集合。
        if (images == nullptr) { *count = count_; return VK_SUCCESS; } // 第一次仅返回需要容量。
        const uint32_t available = *count < count_ ? *count : count_; // 不越过宿主数组容量。
        for (uint32_t index = 0; index < available; ++index) images[index] = slots_[index].image; // 返回自有sRGB句柄，SDK原图像始终保留私有。
        *count = available; // 返回实际写入数量。
        return available == count_ ? VK_SUCCESS : VK_INCOMPLETE; // 容量不足沿用Vulkan枚举语义。
    } // get_images结束。
    VkResult prepare_copy(VkQueue queue, VkSwapchainKHR swapchain, const VkPresentInfoKHR& input, VkPresentInfoKHR& output) { // 成功后必须立刻在同一游戏队列调用SDK Present消费done。
        if (device_ == VK_NULL_HANDLE || queue == VK_NULL_HANDLE || input.swapchainCount != 1 || input.pSwapchains == nullptr || input.pSwapchains[0] != swapchain || input.pImageIndices == nullptr || input.pImageIndices[0] >= count_ || input.waitSemaphoreCount > kMaxWaits || (input.waitSemaphoreCount != 0 && input.pWaitSemaphores == nullptr)) return VK_ERROR_INITIALIZATION_FAILED; // 当前封装只允许主FSR窗口，且拒绝无界等待列表。
        Slot& slot = slots_[input.pImageIndices[0]]; // 使用SDK Acquire返回的同一个索引。
        std::array<VkPipelineStageFlags, kMaxWaits> stages{}; // 栈上固定等待阶段，没有每帧堆分配。
        stages.fill(VK_PIPELINE_STAGE_ALL_COMMANDS_BIT); // 保证宿主图像写入和布局转换都完成后才复制。
        VkSubmitInfo submit{}; // 普通二进制信号提交，与wgpu Present等待语义一致。
        submit.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO; // 显式标识完整零初始化结构。
        submit.waitSemaphoreCount = input.waitSemaphoreCount; // 消费宿主原本交给Present的信号。
        submit.pWaitSemaphores = input.pWaitSemaphores; // 借用到vkQueueSubmit同步返回即可。
        submit.pWaitDstStageMask = stages.data(); // 所有等待都覆盖复制命令。
        submit.commandBufferCount = 1; // 每帧最多提交一个固定复制命令。
        submit.pCommandBuffers = &slot.command; // 预录命令不重置、不重新录制。
        submit.signalSemaphoreCount = 1; // SDK只等待本次复制完成。
        submit.pSignalSemaphores = &slot.done; // 同游戏队列先signal再由SDK提交wait，下一次才能再次signal。
        const VkResult result = vkQueueSubmit_(queue, 1, &submit, VK_NULL_HANDLE); // 不可变simultaneous-use命令无需CPU fence；同队列和Acquire依赖约束GPU复用。
        if (result != VK_SUCCESS) return result; // 提交失败不得继续让SDK等待未产生的done。
        output = input; // 保留Present扩展链、交换链索引和每交换链结果指针。
        output.waitSemaphoreCount = 1; // 原等待已经由复制提交消费，禁止SDK重复等待。
        output.pWaitSemaphores = &slot.done; // 槽位生存期覆盖SDK对Present参数的同步读取。
        return VK_SUCCESS; // 调用方接着调用官方replacement Present，不能丢弃这个等待。
    } // prepare_copy结束。
private: // 图像、命令和同步对象只归本类所有。
    struct Slot { // 每交换链图像固定一组资源。
        VkImage image = VK_NULL_HANDLE; // wgpu实际写入的sRGB前缓冲。
        VkDeviceMemory memory = VK_NULL_HANDLE; // 前缓冲设备内存。
        VkCommandPool pool = VK_NULL_HANDLE; // 绑定游戏队列族的命令池。
        VkCommandBuffer command = VK_NULL_HANDLE; // 一次录制、多次按队列顺序执行的复制命令。
        VkSemaphore done = VK_NULL_HANDLE; // 复制与SDK Present之间的二进制信号。
    }; // Slot结束。
    static VkImageMemoryBarrier barrier(VkImage image, VkImageLayout oldLayout, VkImageLayout newLayout, VkAccessFlags source, VkAccessFlags destination) { // 单层完整颜色范围的统一布局描述。
        VkImageMemoryBarrier value{}; // 不涉及交换链PRESENT布局。
        value.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER; // 显式标识布局屏障结构。
        value.srcAccessMask = source; // 精确描述前一用途。
        value.dstAccessMask = destination; // 精确描述下一用途。
        value.oldLayout = oldLayout; // 与wgpu provider普通图像约定一致。
        value.newLayout = newLayout; // 与SDK shader-read传入约定一致。
        value.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; // 宿主复制与SDK game queue处于同一族。
        value.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; // SDK内部继续管理跨队列的后续工作。
        value.image = image; // 不混淆前缓冲和SDK后缓冲。
        value.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}; // 只覆盖首期固定的二维单层图像。
        return value; // 返回值仅用于创建阶段命令录制。
    } // barrier结束。
    VkResult create_slot(Slot& slot, const VkSwapchainCreateInfoKHR& original, uint32_t family, const VkPhysicalDeviceMemoryProperties& properties, VkImage proxy) { // 每槽创建完整后才录制复制。
        if (proxy == VK_NULL_HANDLE) return VK_ERROR_INITIALIZATION_FAILED; // SDK必须提供完整图像集合。
        VkImageCreateInfo image{}; // 前缓冲是普通图像，不是假装的真实交换链图像。
        image.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO; // 显式标识图像创建结构。
        image.flags = (original.flags & VK_SWAPCHAIN_CREATE_MUTABLE_FORMAT_BIT_KHR) != 0 ? VK_IMAGE_CREATE_MUTABLE_FORMAT_BIT : 0; // 保留wgpu声明的可变view契约，默认sRGB不需要该位。
        image.imageType = VK_IMAGE_TYPE_2D; // 只处理主窗口二维颜色。
        image.format = original.imageFormat; // 保留sRGB attachment的硬件编码和线性混合。
        image.extent = {original.imageExtent.width, original.imageExtent.height, 1}; // 保持物理输出尺寸。
        image.mipLevels = 1; // 交换链不包含mipmap。
        image.arrayLayers = 1; // 首期固定单层。
        image.samples = VK_SAMPLE_COUNT_1_BIT; // 最终窗口输出已经解析多重采样。
        image.tiling = VK_IMAGE_TILING_OPTIMAL; // 使用GPU最优布局。
        image.usage = original.imageUsage | VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_SAMPLED_BIT; // shader-read归还布局要求SAMPLED用途；不加入不合法的sRGB STORAGE用途。
        image.sharingMode = VK_SHARING_MODE_EXCLUSIVE; // 仅游戏队列访问前缓冲。
        image.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED; // 首次由wgpu写入完成合法初始化。
        VkResult result = vkCreateImage_(device_, &image, nullptr, &slot.image); // 所有自有对象使用配对的默认分配器。
        if (result != VK_SUCCESS) return result; // 由外层统一清理已经产生的句柄。
        VkMemoryRequirements requirements{}; // Vulkan给出真实尺寸与对齐。
        vkGetImageMemoryRequirements_(device_, slot.image, &requirements); // 仅在交换链创建时调用。
        if (requirements.size > kMaxMemory - memoryBytes_) return VK_ERROR_OUT_OF_DEVICE_MEMORY; // 同时约束图像数和驱动对齐后的显存峰值。
        uint32_t memoryType = UINT32_MAX; // 不允许无支持时静默使用错误内存类型。
        for (uint32_t index = 0; index < properties.memoryTypeCount; ++index) { // Vulkan固定上限的内存类型扫描。
            if ((requirements.memoryTypeBits & (1u << index)) != 0 && (properties.memoryTypes[index].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) != 0) { memoryType = index; break; } // 只采用图像允许的设备本地内存。
        } // 内存类型选择结束。
        if (memoryType == UINT32_MAX) return VK_ERROR_FEATURE_NOT_PRESENT; // 无兼容内存时原子放弃代理候选。
        VkMemoryAllocateInfo allocation{}; // 每槽独立分配，生命周期简单可审计。
        allocation.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO; // 显式标识分配结构。
        VkMemoryDedicatedAllocateInfo dedicated{}; // 普通独立图像直接声明专用分配，兼容驱动的专用内存要求。
        dedicated.sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO; // 显式标识核心专用分配扩展结构。
        dedicated.image = slot.image; // 此分配只绑定当前前缓冲。
        allocation.pNext = &dedicated; // Vulkan 1.1核心专用分配不增加扩展依赖。
        allocation.allocationSize = requirements.size; // 不以像素数猜测驱动所需内存。
        allocation.memoryTypeIndex = memoryType; // 使用已验证的设备本地类型。
        result = vkAllocateMemory_(device_, &allocation, nullptr, &slot.memory); // 创建阶段才分配显存。
        if (result != VK_SUCCESS) return result; // 外层释放当前槽位图像。
        memoryBytes_ += requirements.size; // 只累计分配成功的实际字节，失败由destroy整体撤销。
        result = vkBindImageMemory_(device_, slot.image, slot.memory, 0); // 独立分配从对齐的零偏移绑定。
        if (result != VK_SUCCESS) return result; // 绑定失败不暴露图像。
        VkCommandPoolCreateInfo pool{}; // 命令池只在创建和销毁时使用。
        pool.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO; // 显式标识命令池创建结构。
        pool.queueFamilyIndex = family; // 与传入prepare_copy的游戏队列一致。
        result = vkCreateCommandPool_(device_, &pool, nullptr, &slot.pool); // 每槽独立命令池消除跨槽重置风险。
        if (result != VK_SUCCESS) return result; // 外层统一清理。
        VkCommandBufferAllocateInfo commands{}; // 固定一个主命令缓冲。
        commands.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO; // 显式标识命令分配结构。
        commands.commandPool = slot.pool; // 绑定当前槽位命令池。
        commands.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; // 直接交给vkQueueSubmit。
        commands.commandBufferCount = 1; // 每槽复制命令数量固定。
        result = vkAllocateCommandBuffers_(device_, &commands, &slot.command); // 没有每帧命令分配。
        if (result != VK_SUCCESS) return result; // 已创建资源由外层释放。
        VkSemaphoreCreateInfo semaphore{}; // SDK Present要求普通二进制信号。
        semaphore.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO; // 显式标识二进制信号创建结构。
        result = vkCreateSemaphore_(device_, &semaphore, nullptr, &slot.done); // 同一槽位的信号靠同队列signal/wait顺序复用。
        if (result != VK_SUCCESS) return result; // 不暴露缺同步对象的槽位。
        VkCommandBufferBeginInfo begin{}; // 命令在创建时只录制一次。
        begin.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO; // 显式标识录制开始结构。
        begin.flags = VK_COMMAND_BUFFER_USAGE_SIMULTANEOUS_USE_BIT; // CPU可能领先GPU取得相同索引，不能重置pending命令。
        result = vkBeginCommandBuffer_(slot.command, &begin); // 创建阶段开始录制。
        if (result != VK_SUCCESS) return result; // 没有提交GPU工作，可立即清理。
        std::array<VkImageMemoryBarrier, 2> before = {barrier(slot.image, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_ACCESS_MEMORY_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT), barrier(proxy, VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 0, VK_ACCESS_TRANSFER_WRITE_BIT)}; // SDK初始图像为UNDEFINED；每次全幅覆盖允许丢弃旧内容，统一首帧与复用路径。
        vkCmdPipelineBarrier_(slot.command, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, nullptr, 0, nullptr, static_cast<uint32_t>(before.size()), before.data()); // 宿主写入已由Present等待同步，布局切换在同游戏队列执行。
        VkImageCopy region{}; // 不使用会执行sRGB转换的blit操作。
        region.srcSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1}; // 前缓冲完整颜色层。
        region.dstSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1}; // SDK同尺寸同通道排列的UNORM颜色层。
        region.extent = image.extent; // 全幅覆写保证UNDEFINED旧布局合法丢弃旧内容。
        vkCmdCopyImage_(slot.command, slot.image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, proxy, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region); // SRGB与对应UNORM按兼容像素大小复制原始编码值。
        std::array<VkImageMemoryBarrier, 2> after = {barrier(slot.image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_ACCESS_TRANSFER_READ_BIT, VK_ACCESS_SHADER_READ_BIT), barrier(proxy, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_SHADER_READ_BIT)}; // 前缓冲归还wgpu provider状态，SDK后缓冲满足ReplacementBufferTransferState。
        vkCmdPipelineBarrier_(slot.command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, nullptr, 0, nullptr, static_cast<uint32_t>(after.size()), after.data()); // 信号发出前发布完整复制结果与最终布局。
        return vkEndCommandBuffer_(slot.command); // 返回录制结果；只有全部槽位成功才进入Present。
    } // create_slot结束。
    VkDevice device_ = VK_NULL_HANDLE; // 借用宿主设备，永不销毁设备本身。
    VkDeviceSize memoryBytes_ = 0; // 当前全部前缓冲实际分配字节。
    uint32_t count_ = 0; // 当前完整图像集合数量。
    std::array<Slot, kMaxImages> slots_{}; // 固定预算，无跨帧增长容器。
#define BL5_PRESENT_PROC(name) PFN_##name name##_ = nullptr // 所有Vulkan调用只经宿主动态入口。
    BL5_PRESENT_PROC(vkCreateImage); // 图像创建入口。
    BL5_PRESENT_PROC(vkDestroyImage); // 图像销毁入口。
    BL5_PRESENT_PROC(vkGetImageMemoryRequirements); // 内存需求入口。
    BL5_PRESENT_PROC(vkAllocateMemory); // 分配入口。
    BL5_PRESENT_PROC(vkFreeMemory); // 释放入口。
    BL5_PRESENT_PROC(vkBindImageMemory); // 图像内存绑定入口。
    BL5_PRESENT_PROC(vkCreateCommandPool); // 命令池创建入口。
    BL5_PRESENT_PROC(vkDestroyCommandPool); // 命令池销毁入口。
    BL5_PRESENT_PROC(vkAllocateCommandBuffers); // 固定命令分配入口。
    BL5_PRESENT_PROC(vkBeginCommandBuffer); // 录制开始入口。
    BL5_PRESENT_PROC(vkEndCommandBuffer); // 录制结束入口。
    BL5_PRESENT_PROC(vkCmdPipelineBarrier); // 图像布局入口。
    BL5_PRESENT_PROC(vkCmdCopyImage); // 原始像素复制入口。
    BL5_PRESENT_PROC(vkCreateSemaphore); // 二进制信号创建入口。
    BL5_PRESENT_PROC(vkDestroySemaphore); // 二进制信号销毁入口。
    BL5_PRESENT_PROC(vkQueueSubmit); // 同游戏队列提交入口。
#undef BL5_PRESENT_PROC // 避免宏污染宿主翻译单元。
}; // Bl5FsrPresentImages结束。
