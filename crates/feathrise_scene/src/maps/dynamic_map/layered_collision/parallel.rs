//! 单个碰撞候选的有界后台worker；全部真实结束后才归还批次，不阻塞主世界。
use bevy::tasks::TaskPool; // 复用现有CPU池，不新建运行时线程池。
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
}; // 共享只读工作函数并把worker异常归为候选失败。

pub(super) const MAX_WORKERS: usize = 4; // 本域并行计算上限，给其他后台功能保留池余量。
const MIN_ITEMS_PER_WORKER: usize = 8; // 少量块直接计算，避免细碎任务调度成本。

pub(super) fn worker_count(threads: usize, items: usize, requested: usize) -> usize {
    // 只决定批次内部并行数，不增加在途候选。
    requested
        .clamp(1, MAX_WORKERS) // 调试请求也不能绕过本域上限。
        .min(threads.saturating_sub(1).max(1)) // 至少保留一个池线程的容量；单线程退回协调器直接计算。
        .min(items.div_ceil(MIN_ITEMS_PER_WORKER).max(1)) // 没有足够工作量时不拆批。
} // 上限同时适用于布局与物理形状阶段。

fn run<T, R, F>(items: Vec<(usize, T)>, work: &F) -> Result<Vec<(usize, R)>, String>
// worker只读快照，返回独占完成值。
where
    F: Fn(T) -> Result<R, String>,
{
    // 不接触World、Assets或Commands。
    catch_unwind(AssertUnwindSafe(|| {
        // 普通panic不能提前释放其他仍在执行的worker槽。
        items
            .into_iter()
            .map(|(index, item)| work(item).map(|value| (index, value)))
            .collect() // 本分组失败即可停，整批仍等待其他分组。
    }))
    .unwrap_or_else(|_| Err("碰撞后台worker异常，保留上一有效版本".into())) // OOM/进程终止不在可恢复异常承诺内。
} // 不改变几何失败的原始错误文本。

pub(super) async fn map<T, R, F>(
    // 只能由后台协调器调用；主世界仍使用check_ready。
    pool: &TaskPool,     // 已有AsyncComputeTaskPool或测试专属池。
    items: Vec<T>,       // 仅移动变化块的工作描述。
    worker_limit: usize, // 请求上限，实际数仍受池容量和粒度限制。
    work: F,             // 大型只读输入在闭包里使用Arc。
) -> Result<Vec<R>, String>
// 全部完成后一次返回，不逐块发布。
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Result<R, String> + Send + Sync + 'static,
{
    // 所有子任务拥有有效快照。
    let count = worker_count(pool.thread_num(), items.len(), worker_limit); // 不在每块上独立spawn。
    if count == 1 {
        // 低核和小批次避免子任务调度，调用者本身已在后台。
        return run(items.into_iter().enumerate().collect(), &work)
            .map(|items| items.into_iter().map(|(_, value)| value).collect()); // 串行与并行共用同一块函数。
    } // 这里不调用scope/block_on，不占用worker同步等其他worker。
    let work = Arc::new(work); // 闭包捕获的源快照只共享，不按worker复制整图。
    let mut groups: Vec<Vec<_>> = (0..count).map(|_| Vec::new()).collect(); // 最多四组，受512块上限约束。
    for (index, item) in items.into_iter().enumerate() {
        groups[index % count].push((index, item));
    } // 稳定交错分组，返回时恢复原序。
    let tasks: Vec<_> = groups
        .into_iter()
        .map(|items| {
            // 所有worker先派发，再异步收集。
            let work = Arc::clone(&work); // 不复制输入数组。
            pool.spawn(async move { run(items, work.as_ref()) }) // 每个worker连续计算自己的一组块。
        })
        .collect(); // 不detach，批次必须持有所有句柄直到真实结束。
    drop(work); // 收集结束后不再有隐藏共享快照持有者。
    let mut output = Vec::new(); // 只有完整成功才向发布层交付。
    let mut failure = None; // 首错保留，但不能用?提前取消尚在运行的CPU任务。
    for task in tasks {
        // 主线程不运行这个循环；await让出后台协调器。
        match task.await {
            // 等待真实结束，不假设丢弃Task可以抢占CPU。
            Ok(mut values) => output.append(&mut values), // 尚未发布的局部完成值。
            Err(error) => {
                failure.get_or_insert(error);
            } // 继续回收所有其他worker。
        } // 最慢worker仍属于当前批次。
    } // 至此所有worker和捕获的Arc都已释放。
    if let Some(error) = failure {
        return Err(error);
    } // 失败时丢弃半成品，旧地面不受影响。
    output.sort_unstable_by_key(|(index, _)| *index); // 完成先后不影响块顺序与确定性。
    Ok(output.into_iter().map(|(_, value)| value).collect()) // 仅最终完整结果离开协调器。
} // 同时运行的计算数有限，跨图旧批次仍占原single-flight槽。

#[cfg(test)] // 调度合同与真实多核执行分别验证。
#[path = "parallel_tests.rs"] // 测试不扩充生产调度主体。
mod tests; // 无图形依赖。
