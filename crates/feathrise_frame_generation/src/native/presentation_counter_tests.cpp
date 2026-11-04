// 直接验证生产呈现计数器；不加载NVIDIA SDK、不创建窗口或GPU。
#include "presentation_counter.h" // 测试正式使用的计数与非阻塞快照。
#include <cassert> // 失败立即返回非零退出码。
#include <thread> // 模拟Present写入和主世界读取并行。

int main() { // 固定规模CPU回归。
    Bl5PresentationCounter counter; // 未呈现时没有可用样本。
    assert(!counter.snapshot().ready); // 初始化不能冒充0FPS事实。
    counter.record(2); // 一张基础帧加一张实际生成帧。
    counter.record(0); // SDK尚未更新时零增量不能补成一帧。
    counter.record(1); // 关闭或回退后的真实普通Present。
    auto first = counter.snapshot(); // 稳定快照反映完整区间。
    assert(first.ready && first.count == 3 && first.epoch == 0); // 可变输出必须按实际累计。
    counter.invalidate(); // 模拟失败或尺寸切换，不记入失败帧。
    auto failed = counter.snapshot(); // 旧数字必须立即失效。
    assert(!failed.ready && failed.count == 3 && failed.epoch == 1); // 单调计数保留，旧采样代次作废。
    counter.record(2); // 恢复后的第一个真实Present。
    auto recovered = counter.snapshot(); // 消费者必须从新代次重新预热。
    assert(recovered.ready && recovered.count == 5 && recovered.epoch == 1); // 恢复不重复累计旧帧。
    Bl5PresentationCounter concurrent; // 独立并发计数器。
    std::atomic<bool> done{false}; // 仅测试线程完成通知。
    std::thread writer([&] { // 生产契约是单写者。
        for (uint32_t i = 0; i < 10000; ++i) { concurrent.invalidate(); concurrent.record(2); } // 第n代完成后计数严格等于2n。
        done.store(true); // 发布固定规模工作完成。
    }); // 主测试线程同时读取，不修改计数器。
    do { // 仅测试压力循环；生产snapshot自身没有重试循环。
        auto value = concurrent.snapshot(); // 不等待写者，混合快照返回ready=false。
        assert(!value.ready || value.count == value.epoch * 2); // 任何有效快照必须属于完整同代次。
    } while (!done.load()); // 写者总共10000轮，测试工作量有界。
    writer.join(); // 测试退出前回收线程。
    assert(concurrent.snapshot().count == 20000); // 不丢失或重复累计。
    return 0; // 全部回归通过。
} // 测试入口结束。
