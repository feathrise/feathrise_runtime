//! 大批布局在有界异步计算池中的多核执行与无阻塞轮询。

use std::collections::BTreeMap; // worker乱序完成后按块Y/X恢复确定性结果顺序。
use std::sync::Arc; // 输入快照、任务队列和取消标记由少量长期worker共享。
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering}; // worker以无锁索引领取块并在块边界协作取消。

use bevy::prelude::Resource; // 异步任务槽独立于可克隆的公共布局状态。
use bevy::tasks::futures::check_ready; // 主线程只做一次非阻塞就绪检查。
use bevy::tasks::{AsyncComputeTaskPool, Task}; // CPU密集布局跨帧运行，不占用ECS帧关键路径。

use crate::{
    VegetationCompetitionGroup, VegetationConfigError, VegetationLayerConfig, VegetationLayerId,
    VegetationSourceField,
}; // worker拥有完整不可变配置快照。

use super::compose::compose_tile; // 每块仍调用原有确定性CPU权威实现。
use super::resources::{VegetationDirtyTile, VegetationResolvedTile}; // 任务输入输出保持既有公共布局契约。

const ASYNC_LAYOUT_TILE_THRESHOLD: usize = 64; // 小范围热更继续走逐帧预算，整图批次才支付任务调度成本。

type TileResult = (
    VegetationDirtyTile, // 结果必须携带稳定块身份，不能依赖worker完成顺序。
    Result<VegetationResolvedTile, VegetationConfigError>, // 任一失败仍由主线程原子拒绝整批。
);

/// 一批后台worker全部结束后交回主线程的完整结果。
pub(super) struct VegetationLayoutAsyncCompletion {
    pub generation: u64,         // 防止旧配置代次的迟到结果污染新批次。
    pub base_revision: u64,      // 防止基于旧活动布局构造的staging覆盖新版本。
    pub requested_revision: u64, // 结果只能写入启动时锁定的待发布版本。
    pub expected_count: usize,   // 协作取消或意外缺块时不得误判为完整成功。
    pub results:
        BTreeMap<VegetationDirtyTile, Result<VegetationResolvedTile, VegetationConfigError>>, // 块按Y/X稳定合并。
}

/// ECS持有的单批异步任务槽；公共布局状态无需包含不可克隆Task句柄。
#[derive(Resource, Default)]
pub(crate) struct VegetationLayoutAsyncWork {
    batch: Option<VegetationLayoutAsyncBatch>, // 同一时刻只允许一批布局写入下一活动版本。
}

struct VegetationLayoutAsyncBatch {
    generation: u64,                     // 锁定启动时配置与源字段代次。
    base_revision: u64,                  // 锁定创建暂存布局时的活动版本。
    requested_revision: u64,             // 锁定成功后的唯一新revision。
    expected_count: usize,               // 完成时核对所有请求块均已返回。
    cancelled: Arc<AtomicBool>,          // 代次变化或暂停时让正在运行的worker在块边界退出。
    workers: Vec<Task<Vec<TileResult>>>, // worker数量受异步池线程数约束而非按块无限创建。
    results: BTreeMap<VegetationDirtyTile, Result<VegetationResolvedTile, VegetationConfigError>>, // 已完成worker结果暂存在不可见批次。
}

struct VegetationLayoutInputSnapshot {
    texel_size: f32,                         // 整批固定使用最高精度源字段的世界纹素尺寸。
    groups: Vec<VegetationCompetitionGroup>, // 竞争组数量很小，可在启动帧廉价复制。
    layers: Vec<VegetationLayerConfig>,      // 层配置和显式落点快照禁止与热更代次混用。
    sources: BTreeMap<VegetationLayerId, VegetationSourceField>, // 大源像素由字段内部Arc切片共享。
}

impl VegetationLayoutAsyncWork {
    /// 判断当前脏块规模与运行环境是否适合启动后台批次。
    pub(super) fn should_start(tile_count: usize) -> bool {
        tile_count >= ASYNC_LAYOUT_TILE_THRESHOLD && AsyncComputeTaskPool::try_get().is_some() // 未初始化池的逻辑单测继续走同步预算。
    }

    /// 当前后台批次是否与主世界的待处理身份完全一致。
    pub(super) fn matches(
        &self,
        generation: u64,
        base_revision: u64,
        requested_revision: u64,
    ) -> bool {
        self.batch.as_ref().is_some_and(|batch| {
            batch.generation == generation // 配置或源字段不能跨代。
                && batch.base_revision == base_revision // staging必须来自同一活动基线。
                && batch.requested_revision == requested_revision // 换版票据必须一致。
        })
    }

    /// 尝试把整图级脏块批次交给有界AsyncCompute worker。
    #[allow(clippy::too_many_arguments)] // 参数显式描述不可混用的批次身份和输入快照。
    pub(super) fn try_start(
        &mut self,
        generation: u64,
        base_revision: u64,
        requested_revision: u64,
        texel_size: f32,
        dirty: Vec<VegetationDirtyTile>,
        groups: Vec<VegetationCompetitionGroup>,
        layers: Vec<VegetationLayerConfig>,
        sources: BTreeMap<VegetationLayerId, VegetationSourceField>,
    ) -> bool {
        if self.batch.is_some() || !Self::should_start(dirty.len()) {
            return false; // 已有任务或小局部更新继续使用原有同步预算路径。
        }
        let Some(pool) = AsyncComputeTaskPool::try_get() else {
            return false; // 纯逻辑单测未初始化全局池时保持原有确定性同步行为。
        };
        let worker_count = async_worker_count(pool.thread_num(), dirty.len()); // 至少保留一个异步线程给管线编译等并行冷启动任务。
        let tiles: Arc<[VegetationDirtyTile]> = Arc::from(dirty); // 所有worker共享稳定Y/X输入序列。
        let cursor = Arc::new(AtomicUsize::new(0)); // 每个块只由一个worker领取。
        let cancelled = Arc::new(AtomicBool::new(false)); // 无await纯CPU任务只能在块边界协作停止。
        let snapshot = Arc::new(VegetationLayoutInputSnapshot {
            texel_size, // 保存整批固定纹素尺寸。
            groups,     // 保存受检竞争组快照。
            layers,     // 保存受检层配置快照。
            sources,    // 保存共享像素源快照。
        });
        let mut workers = Vec::with_capacity(worker_count); // 只创建与池容量相称的长期worker。
        for _ in 0..worker_count {
            let worker_tiles = Arc::clone(&tiles); // worker共享不可变块序列。
            let worker_cursor = Arc::clone(&cursor); // worker共享领取索引。
            let worker_cancelled = Arc::clone(&cancelled); // worker共享代次失效标记。
            let worker_snapshot = Arc::clone(&snapshot); // worker共享不可变输入快照。
            workers.push(pool.spawn(async move {
                compose_worker(
                    worker_tiles,     // 传入稳定块序列。
                    worker_cursor,    // 传入无锁领取索引。
                    worker_cancelled, // 传入协作取消标记。
                    worker_snapshot,  // 传入同代布局输入。
                )
            })); // Task由AsyncCompute池自动跨帧驱动。
        }
        self.batch = Some(VegetationLayoutAsyncBatch {
            generation,                  // 保存启动代次供轮询核对。
            base_revision,               // 保存活动基线供轮询核对。
            requested_revision,          // 保存待发布revision供轮询核对。
            expected_count: tiles.len(), // 完成必须返回全部块。
            cancelled,                   // 主线程可在热更时请求停止。
            workers,                     // 持有句柄使任务不会因Drop自动取消。
            results: BTreeMap::new(),    // 尚无worker完成。
        });
        true // 调用方本帧不得再同步计算同一批块。
    }

    /// 非阻塞收集已经完成的worker；全部完成前不返回部分布局。
    pub(super) fn poll(&mut self) -> Option<VegetationLayoutAsyncCompletion> {
        let batch = self.batch.as_mut()?; // 没有后台批次时调用方可走同步路径。
        let mut worker_index = 0; // swap_remove后在当前位置继续检查换入的句柄。
        while worker_index < batch.workers.len() {
            let Some(output) = check_ready(&mut batch.workers[worker_index]) else {
                worker_index += 1; // 未完成worker不会阻塞当前Update。
                continue;
            };
            let _completed_task = batch.workers.swap_remove(worker_index); // 已完成句柄不再保留或重复轮询。
            for (tile, result) in output {
                batch.results.insert(tile, result); // BTreeMap消除worker完成顺序差异。
            }
        }
        if !batch.workers.is_empty() {
            return None; // 任一worker仍运行时活动布局保持上一完整版本。
        }
        let completed = self
            .batch
            .take()
            .expect("completed async layout batch must exist"); // 所有句柄完成后取得完整所有权。
        Some(VegetationLayoutAsyncCompletion {
            generation: completed.generation,                 // 交回启动代次。
            base_revision: completed.base_revision,           // 交回活动基线。
            requested_revision: completed.requested_revision, // 交回待发布revision。
            expected_count: completed.expected_count,         // 交回完整性计数。
            results: completed.results,                       // 一次交回全部稳定排序结果。
        })
    }

    /// 废弃当前后台批次并请求worker在完成手中块后立即停止。
    pub(super) fn cancel(&mut self) {
        if let Some(batch) = self.batch.take() {
            batch.cancelled.store(true, Ordering::Release); // 先通知正在执行且无法被Task Drop抢占的纯CPU闭包。
        } // 随后Drop全部Task句柄，尚未开始的任务由Bevy直接取消。
    }
}

fn async_worker_count(pool_threads: usize, tile_count: usize) -> usize {
    let available_workers = pool_threads.max(1).div_ceil(2).clamp(1, 4); // 只占约半池且最多四worker，为管线和资源冷启动保留容量。
    available_workers.min(tile_count.max(1)) // worker绝不超过实际块数。
}

fn compose_worker(
    tiles: Arc<[VegetationDirtyTile]>, // 同一批稳定Y/X块序列。
    cursor: Arc<AtomicUsize>,          // 全worker共享的下一个块索引。
    cancelled: Arc<AtomicBool>,        // 只在块边界检查以免改变纯合成结果。
    snapshot: Arc<VegetationLayoutInputSnapshot>, // 同代不可变输入集。
) -> Vec<TileResult> {
    let mut results = Vec::new(); // 每个worker只暂存自己领取的完整块。
    loop {
        if cancelled.load(Ordering::Acquire) {
            break; // 热更或暂停后不再领取新块，当前已完成块也不会发布。
        }
        let index = cursor.fetch_add(1, Ordering::Relaxed); // 每个索引只会发给一个worker。
        let Some(&tile) = tiles.get(index) else {
            break; // 所有请求块均已领取。
        };
        let result = compose_tile(
            tile,                // 合成当前稳定块。
            snapshot.texel_size, // 使用整批固定世界纹素尺寸。
            &snapshot.groups,    // 使用启动代竞争组。
            &snapshot.layers,    // 使用启动代层配置。
            &snapshot.sources,   // 使用启动代共享源字段。
        );
        results.push((tile, result)); // 成功和错误都交回主线程统一原子决策。
    }
    results // 仅Task完成后主线程可见，绝不形成部分活动布局。
}

#[cfg(test)]
mod tests {
    use super::*; // 锁定有界worker数量和大批门槛的纯策略。

    #[test]
    fn worker_count_reserves_async_capacity_and_never_exceeds_tiles() {
        assert_eq!(async_worker_count(8, 1), 1); // 单块不创建空闲worker。
        assert_eq!(async_worker_count(8, 1024), 4); // 大池最多使用四个布局worker。
        assert_eq!(async_worker_count(6, 1024), 3); // 常见正式异步池只取约一半线程。
        assert_eq!(async_worker_count(1, 1024), 1); // 单线程设备仍能完成而不产生零worker。
    }
}
