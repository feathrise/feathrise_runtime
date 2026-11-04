//! 叶片密度候选的后台纯CPU构建、身份核对与无阻塞轮询。

use bevy::prelude::Resource; // Task槽独立于GPU协调器和活动缓存。
use bevy::tasks::futures::check_ready; // Update只探测完成状态而不等待CPU任务。
use bevy::tasks::{AsyncComputeTaskPool, Task}; // 大地图候选准备跨帧离开主线程执行。

use crate::{
    ActiveVegetationLayout, VegetationLayerId, VegetationLayoutCommit, VegetationLayoutState,
}; // 后台任务拥有布局、提交和受检输入的完整快照。

use super::resources::VegetationBladeDensityCache; // 上一活动图集通过Arc被廉价共享到任务。
use super::systems::build::{
    BuiltBladeDensityBatch, PendingBladeDensityBatchBuild, build_pending_batch_cpu,
}; // Task只执行不访问ECS或Assets的纯CPU阶段。

const ASYNC_DENSITY_TILE_THRESHOLD: usize = 64; // 小夹具和小地图保持同步，整图与大地图局部换版进入后台。

/// 一个CPU候选任务不可混用的完整身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BladeDensityBuildIdentity {
    pub(super) revision: u64,                        // 锁定活动布局版本。
    pub(super) configuration_generation: u64,        // 锁定生成该布局的配置与源代次。
    pub(super) bound_layers: Vec<VegetationLayerId>, // 锁定跨层原子采用集合及稳定顺序。
}

impl BladeDensityBuildIdentity {
    /// 由当前主世界快照构造可比较身份。
    pub(super) fn new(
        revision: u64,                      // 当前活动布局版本。
        configuration_generation: u64,      // 当前布局正式提交代次。
        bound_layers: &[VegetationLayerId], // 已去重稳定层集合。
    ) -> Self {
        Self {
            revision,                            // 保存活动布局身份。
            configuration_generation,            // 保存配置与源身份。
            bound_layers: bound_layers.to_vec(), // 层数很小且任务必须拥有独立快照。
        }
    }
}

/// 后台CPU任务完成后交回主线程的未登记候选或明确失败。
pub(super) struct BladeDensityBuildCompletion {
    pub(super) identity: BladeDensityBuildIdentity, // 主线程登记Assets前必须重新核对。
    pub(super) ticket: u64,                         // 构建产物还必须匹配启动时已消费票据。
    pub(super) result: Result<BuiltBladeDensityBatch, (VegetationLayerId, String)>, // 过期成功值可直接Drop。
}

/// 主世界持有的最多一个叶片密度CPU任务槽。
#[derive(Resource, Default)]
pub(super) struct VegetationBladeDensityCpuBuild {
    pending: Option<PendingBladeDensityCpuBuild>, // 同一时间只准备一个跨层原子候选。
}

struct PendingBladeDensityCpuBuild {
    identity: BladeDensityBuildIdentity, // 用于热更、换版和绑定变化时取消。
    ticket: u64,                         // 取消后不复用且完成时再次核对。
    cancelled: bool,                     // 运行中的无await任务先标记过期并保持严格single-flight。
    task: Task<Result<BuiltBladeDensityBatch, (VegetationLayerId, String)>>, // 纯CPU产物不接触主世界Assets。
}

impl VegetationBladeDensityCpuBuild {
    /// 当前任务是否仍对应主世界期望身份。
    pub(super) fn matches(&self, identity: &BladeDensityBuildIdentity) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| !pending.cancelled && pending.identity == *identity) // 过期标记及完整身份必须全部匹配。
    }

    /// 当前任务是否正在覆盖指定活动版本和全部可见层。
    pub(super) fn covers_visible_layers(
        &self,
        revision: u64, // 状态快照当前活动版本。
        visible_layers: impl Iterator<Item = VegetationLayerId>, // 可见层可只是任务原子集合的子集。
    ) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            !pending.cancelled
                && pending.identity.revision == revision
                && visible_layers
                    .into_iter()
                    .all(|layer| pending.identity.bound_layers.binary_search(&layer).is_ok()) // 稳定层向量支持无分配包含检查。
        })
    }

    /// 判断当前地图规模是否值得把候选CPU构建移出主线程。
    pub(super) fn should_start_async(active_tile_count: usize) -> bool {
        active_tile_count >= ASYNC_DENSITY_TILE_THRESHOLD
            && AsyncComputeTaskPool::try_get().is_some() // 逻辑单测无全局池时保持同步行为。
    }

    /// 启动一个拥有全部输入快照的后台纯CPU任务。
    #[allow(clippy::too_many_arguments)] // 参数逐项表达任务不可混用的身份与输入快照。
    pub(super) fn start(
        &mut self,
        identity: BladeDensityBuildIdentity, // 当前完整构建身份。
        ticket: u64,                         // 启动时已经消费，取消后不得复用。
        active: ActiveVegetationLayout,      // 解析块由内部Arc共享，不复制完整密度。
        commit: VegetationLayoutCommit,      // 脏块列表固定属于同一revision。
        layout: VegetationLayoutState,       // 源像素由内部Arc共享，配置按代锁定。
        bound_layers: Vec<VegetationLayerId>, // 跨层集合与identity保持同序。
        cache: VegetationBladeDensityCache,  // 上一图集由Arc共享供局部补丁。
    ) -> bool {
        if self.pending.is_some() {
            return false; // 旧任务真正结束并丢弃前禁止叠加第二个百MiB级构建。
        }
        let pool = AsyncComputeTaskPool::get(); // 调用方已通过should_start_async确认全局池存在。
        let task = pool.spawn(async move {
            build_pending_batch_cpu(PendingBladeDensityBatchBuild {
                ticket,                      // 纯CPU产物沿用已消费票据。
                active: &active,             // 任务闭包拥有活动布局快照。
                commit: &commit,             // 任务闭包拥有原子提交快照。
                layout: &layout,             // 任务闭包拥有受检输入快照。
                bound_layers: &bound_layers, // 任务闭包拥有稳定层集合。
                cache: &cache,               // 任务闭包拥有上一活动缓存快照。
            })
        }); // 无ECS借用或Assets写入越过线程边界。
        self.pending = Some(PendingBladeDensityCpuBuild {
            identity,         // 保存登记前必须再次核对的身份。
            ticket,           // 保存不可复用票据供完成核对。
            cancelled: false, // 新任务初始仍属于当前身份。
            task,             // 持有句柄使Bevy持续驱动任务。
        });
        true // 调用方本帧只启动后台任务并立即返回。
    }

    /// 是否仍有当前或已标记过期但尚未真正结束的CPU任务。
    pub(super) fn is_busy(&self) -> bool {
        self.pending.is_some() // single-flight期间任何第二次大构建都必须等待。
    }

    /// 非阻塞取得已经完成的CPU候选。
    pub(super) fn poll(&mut self) -> Option<BladeDensityBuildCompletion> {
        if self.pending.as_ref()?.cancelled {
            self.reap_cancelled(); // 过期任务只允许丢弃，不得作为正常完成返回。
            return None;
        }
        let pending = self.pending.as_mut()?; // 无任务时调用方可以同步构建或保持稳定。
        let result = check_ready(&mut pending.task)?; // 未完成时立即返回，不阻塞当前帧。
        let pending = self
            .pending
            .take()
            .expect("ready density CPU task must exist"); // 就绪后释放任务槽供下一版本使用。
        Some(BladeDensityBuildCompletion {
            identity: pending.identity, // 交回启动时完整身份。
            ticket: pending.ticket,     // 交回启动时已消费票据。
            result,                     // 交回未登记候选或失败。
        })
    }

    /// 取消尚未公开的过期CPU候选任务。
    pub(super) fn cancel(&mut self) {
        if let Some(pending) = self.pending.as_mut() {
            pending.cancelled = true; // 已开始的无await闭包不能被Task Drop抢占，先永久标记结果过期。
        }
        self.reap_cancelled(); // 每帧非阻塞回收已真正结束的旧任务。
    }

    fn reap_cancelled(&mut self) {
        let ready = self.pending.as_mut().is_some_and(|pending| {
            pending.cancelled && check_ready(&mut pending.task).is_some() // 只消费已经完成的过期结果。
        });
        if ready {
            self.pending = None; // 真正结束后才释放single-flight槽并允许下一身份启动。
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::tasks::TaskPoolBuilder; // 独立本地池避免修改进程级AsyncCompute单例。

    use super::*; // 直接锁定私有single-flight任务槽语义。

    #[test]
    fn cancelled_running_task_keeps_single_flight_slot_until_it_really_finishes() {
        let pool = TaskPoolBuilder::new().num_threads(1).build(); // 一个本地worker足以持有永不就绪测试任务。
        let identity = BladeDensityBuildIdentity::new(7, 3, &[VegetationLayerId(11)]); // 构造完整旧任务身份。
        let task = pool.spawn(async {
            std::future::pending::<Result<BuiltBladeDensityBatch, (VegetationLayerId, String)>>()
                .await
        }); // 永不自然完成，模拟必须继续占住single-flight槽的在途重任务。
        let mut slot = VegetationBladeDensityCpuBuild {
            pending: Some(PendingBladeDensityCpuBuild {
                identity: identity.clone(), // 保存旧身份。
                ticket: 5,                  // 保存已消费票据。
                cancelled: false,           // 调用cancel前仍为当前任务。
                task,                       // 持有不会自行完成的Task句柄。
            }),
        };
        slot.cancel(); // 身份变化只能标记过期并非阻塞等待。
        assert!(slot.is_busy()); // 旧任务真正结束前必须继续占用唯一大构建槽。
        assert!(!slot.matches(&identity)); // 已取消任务即使身份值相同也不能返回或登记。
    }
}
