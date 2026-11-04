// 本文件通过SDK公开presentQueue.submitFunc修复1.1.4在真实图像Acquire失败时误signal二进制Present信号的问题。
// 【限定依据】FrameInterpolationSwapchainVK.cpp:974–1063的copyAndPresent失败分支仍添加frameRenderedSemaphores[0]，却没有Present消费它。
// 仅适用于签名锁定的SDK 1.1.4、composeOnPresentQueue=false；正常命令提交、其他队列及其他结构完全透传。
#pragma once // 同一原生翻译单元只定义一次有限提交适配器。
#include <array> // 最多保留两个timeline完成信号，逐帧不分配内存。
#include <vulkan/vulkan.h> // 只接受公开Vulkan提交描述，不借用SDK私有对象布局。

namespace bl5_fsr3_present_submit { // 与bridge中的其他原生辅助逻辑隔离。

inline bool is_failed_acquire_completion(const VkSubmitInfo& input, VkFence fence) { // 严格匹配已审计的失败Acquire提交形状。
    if (fence != VK_NULL_HANDLE || input.sType != VK_STRUCTURE_TYPE_SUBMIT_INFO || input.commandBufferCount != 0 || input.waitSemaphoreCount != 1 || input.pWaitSemaphores == nullptr || input.pWaitDstStageMask == nullptr || input.pWaitDstStageMask[0] != VK_PIPELINE_STAGE_ALL_COMMANDS_BIT || input.signalSemaphoreCount < 2 || input.signalSemaphoreCount > 3 || input.pSignalSemaphores == nullptr || input.pNext == nullptr) return false; // 其他提交原样交给原始Vulkan入口。
    const auto* timeline = static_cast<const VkTimelineSemaphoreSubmitInfo*>(input.pNext); // SDK Helpers.cpp固定将timeline作为唯一pNext节点。
    if (timeline->sType != VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO || timeline->pNext != nullptr || timeline->waitSemaphoreValueCount != 1 || timeline->pWaitSemaphoreValues == nullptr || timeline->signalSemaphoreValueCount != input.signalSemaphoreCount || timeline->pSignalSemaphoreValues == nullptr) return false; // 不改写未知扩展链、残缺数组或升级后的协议。
    const uint64_t completedFrame = timeline->pWaitSemaphoreValues[0]; // 失败分支等待唯一composition timeline的当前呈现编号。
    if (completedFrame == 0 || timeline->pSignalSemaphoreValues[0] != 0) return false; // 首个signal必须是SDK add(binary)约定的零值；合法timeline signal必大于初始零。
    for (uint32_t index = 1; index < input.signalSemaphoreCount; ++index) { // 最多两项固定扫描。
        if (timeline->pSignalSemaphoreValues[index] != completedFrame) return false; // 只允许同帧replacement/present timeline完成标记。
    } // 严格timeline值匹配结束。
    return true; // 没有真实图像可呈现，只需推进timeline，不得signal无人消费的二进制信号。
} // 失败Acquire识别结束。

inline VkResult submit(PFN_vkQueueSubmit nativeSubmit, VkQueue presentQueue, uint32_t submitCount, const VkSubmitInfo* submits, VkFence fence, bool* filtered = nullptr) { // 只能接入SDK的独立presentQueue回调。
    if (filtered != nullptr) *filtered = false; // 由宿主选择是否记录稀有修复事件。
    if (nativeSubmit == nullptr || presentQueue == VK_NULL_HANDLE) return VK_ERROR_INITIALIZATION_FAILED; // 未附着时不调用空句柄。
    if (submitCount != 1 || submits == nullptr || !is_failed_acquire_completion(submits[0], fence)) return nativeSubmit(presentQueue, submitCount, submits, fence); // 非精确形状保持原始参数与批次语义。
    const auto* originalTimeline = static_cast<const VkTimelineSemaphoreSubmitInfo*>(submits[0].pNext); // 已由上面的严格匹配确认完整链与数组。
    std::array<VkSemaphore, 2> signals{}; // 去掉首个binary后最多保留两个timeline句柄。
    std::array<uint64_t, 2> values{}; // timeline值必须随句柄同步压缩。
    const uint32_t retainedCount = submits[0].signalSemaphoreCount - 1; // 不删除任何timeline完成条件。
    for (uint32_t index = 0; index < retainedCount; ++index) { // 固定两项上限，无堆分配。
        signals[index] = submits[0].pSignalSemaphores[index + 1]; // 只跳过未取得真实图像对应的binary信号。
        values[index] = originalTimeline->pSignalSemaphoreValues[index + 1]; // 保持每个timeline与其原始值配对。
    } // 输出数组构造结束。
    VkTimelineSemaphoreSubmitInfo timeline = *originalTimeline; // 原wait数组和wait值保持不变。
    timeline.signalSemaphoreValueCount = retainedCount; // timeline计数与VkSubmitInfo同步更新。
    timeline.pSignalSemaphoreValues = values.data(); // 指针只需活到同步vkQueueSubmit返回。
    VkSubmitInfo corrected = submits[0]; // 保留全部等待阶段、命令与其他固定字段。
    corrected.pNext = &timeline; // 使用匹配压缩后的timeline描述。
    corrected.signalSemaphoreCount = retainedCount; // 只少一次无消费者的binary signal。
    corrected.pSignalSemaphores = signals.data(); // 所有必要进度信号仍提交到同一Present队列。
    if (filtered != nullptr) *filtered = true; // 仅精确命中缺陷分支时发布修复事件。
    return nativeSubmit(presentQueue, 1, &corrected, fence); // 无CPU等待，返回真实提交结果。
} // 有界Present提交适配结束。

} // bl5_fsr3_present_submit命名空间结束。
