// DLSS呈现累计计数：单一Render/Present写入者，主世界只做一次无等待快照读取。
#pragma once
#include <atomic> // 所有跨线程字段使用原子，快照冲突时直接放弃本帧。
#include <cstdint> // 与Rust FFI保持固定宽度。

struct Bl5PresentationSnapshot { // 不泄露SDK类型的固定大小快照。
    uint64_t count; // 已确认的累计呈现帧数，包含生成帧。
    uint64_t epoch; // 启停、窗口规格变化或失败后的独立采样代次。
    bool ready; // 本代次是否已有可信Present结果。
}; // 快照字段结束。

class Bl5PresentationCounter { // 正常路径仅固定次数原子操作，不分配或查询GPU。
    std::atomic<uint64_t> sequence{0}, count{0}, epoch{0}; // 偶数版本代表一次完整发布。
    std::atomic<bool> ready{false}; // 尚未产生Present时不能伪造FPS。
public:
    void record(uint32_t frames) { // SDK返回自上次查询以来的实际增量，零也是有效结果。
        sequence.fetch_add(1); // 单写者进入发布区间；seq_cst保持所有字段的全序。
        count.fetch_add(frames); // 不使用设置倍率或主循环帧数推算。
        ready.store(true); // 首个结果只供消费者建立基线。
        sequence.fetch_add(1); // 完整发布计数与有效性。
    } // 记录完成。
    void invalidate() { // 启停或交换链失败后禁止跨代次相减。
        sequence.fetch_add(1); // 开始一次完整发布。
        epoch.fetch_add(1); // 总计数保持单调，消费者按新代次重建基线。
        ready.store(false); // 等待下一个可信Present。
        sequence.fetch_add(1); // 发布新代次。
    } // 失效完成。
    Bl5PresentationSnapshot snapshot() const { // 只尝试一次，绝不循环等待Present线程。
        const auto before = sequence.load(); // 记录读前版本。
        Bl5PresentationSnapshot result{count.load(), epoch.load(), ready.load()}; // 各字段独立原子读取。
        const auto after = sequence.load(); // 检查读取过程中是否有发布。
        result.ready = result.ready && before == after && (before & 1) == 0; // 混合快照仅报告暂不可用。
        return result; // 下一主世界帧自然重试。
    } // 快照完成。
}; // 单写者计数器结束。
