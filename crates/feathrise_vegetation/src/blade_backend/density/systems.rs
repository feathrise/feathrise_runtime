//! 主世界叶片密度候选构建、逐层GPU校验与原子窗口换版。

use std::collections::BTreeSet; // 同层多个LOD窗口先去重并按稳定ID排序。

use bevy::ecs::system::SystemParam; // 把跨世界状态资源组合成具名系统参数。
use bevy::prelude::{Assets, Commands, Entity, Image, MessageWriter, Query, Res, ResMut}; // 主世界只维护图片资产、窗口组件和结构化拒绝消息。

use crate::blade::{GpuBladeDensityMap, GpuBladePatch}; // 成功批次沿用已验收叶片窗口密度绑定ABI。
use crate::{
    ActiveVegetationLayout, VegetationLayerId, VegetationLayoutCommit, VegetationLayoutRejected,
    VegetationLayoutState,
}; // 候选只读取已经原子发布的活动布局和同版本提交块。

use super::async_build::{BladeDensityBuildIdentity, VegetationBladeDensityCpuBuild}; // 大场景CPU候选按完整身份后台构建。
use super::components::{VegetationBladeLayer, VegetationBladeLayoutRevision}; // 查询公开层绑定并私有记录真正采用身份。
use super::resources::{
    PendingBladeDensityBatch, PendingBladeDensityState, VegetationBladeDensityCache,
    VegetationBladeValidationLayers, VegetationGpuMipCoordinator, VegetationGpuMipInbox,
    VegetationGpuMipJob, VegetationGpuMipRequest, VegetationGpuMipRuntimeAvailable,
}; // 状态机的活动缓存、候选批次、请求和回读收件箱。

pub(super) mod build; // CPU参考选择与候选批次构建隔离为加载/换版冷路径。
mod cpu_fallback; // 无RenderApp逻辑测试的完整CPU图片路径独立维护。
mod results; // GPU回读消费、逐字节比较与超时判断保持为纯状态转换。

use build::{PendingBladeDensityBatchBuild, build_pending_batch_cpu, register_built_batch}; // 主状态机把纯CPU构建与主世界Assets登记分开。
use cpu_fallback::sync_cpu_density_fallback; // 真实渲染运行时不会调用该回退。
#[cfg(test)]
use results::compare_gpu_mip_bytes; // 子模块回归直接锁定逐字节拒绝口径。
use results::{consume_gpu_results, mark_timeout_if_needed}; // 主系统组合无ECS副作用的结果状态转换。

/// 一个显式绑定叶片层的窗口查询。
type BladeDensityWindowQuery<'a> = (
    Entity,                                    // 成功后通过Commands原子插入密度句柄和采用身份。
    &'a VegetationBladeLayer,                  // 当前消费者要求的内容无关层。
    &'a mut GpuBladePatch,                     // 成功后同步替换密度世界映射。
    Option<&'a VegetationBladeLayoutRevision>, // 缺失或身份变化时需要采用当前缓存。
);

/// 主世界密度桥接系统的全部共享状态。
#[derive(SystemParam)]
pub(super) struct BladeDensityMainResources<'w> {
    active: Res<'w, ActiveVegetationLayout>, // 只读取一次完整活动版本。
    commit: Res<'w, VegetationLayoutCommit>, // 相邻版本同范围时读取稳定脏块清单。
    layout: Res<'w, VegetationLayoutState>,  // 正式GPU竞争输入读取当前受检配置与源字段。
    validation: Res<'w, VegetationBladeValidationLayers>, // 不可见迁移层与可见窗口共同进入原子候选门。
    images: ResMut<'w, Assets<Image>>, // 候选图片与当前有效图片都由同一资产表持有。
    cache: ResMut<'w, VegetationBladeDensityCache>, // 失败前绝不覆盖当前有效层缓存。
    cpu_build: ResMut<'w, VegetationBladeDensityCpuBuild>, // 大候选后台任务完成前不写Assets。
    coordinator: ResMut<'w, VegetationGpuMipCoordinator>, // 同时只允许一个原子候选批次。
    request: ResMut<'w, VegetationGpuMipRequest>, // RenderApp每帧只提取当前串行层。
    inbox: ResMut<'w, VegetationGpuMipInbox>, // 接收异步回调完成的版本化结果。
    runtime: Res<'w, VegetationGpuMipRuntimeAvailable>, // 无RenderApp测试走CPU完整图片回退。
    rejected: MessageWriter<'w, VegetationLayoutRejected>, // 首次失败发布结构化原因。
}

/// 同步显式绑定窗口；GPU运行时只允许完整候选批次通过后换版。
pub(super) fn sync_resolved_blade_density(
    mut commands: Commands,                          // 成功换版时批量写窗口组件。
    mut resources: BladeDensityMainResources,        // 具名资源避免易错的长位置参数列表。
    mut windows: Query<BladeDensityWindowQuery<'_>>, // 通常是同层近中远多个LOD窗口。
) {
    resources.coordinator.update_index = resources.coordinator.update_index.wrapping_add(1); // 每次Update推进有界等待时钟。
    let bound_layers = collect_bound_layers(&mut windows, &resources.validation); // 固定可见绑定与不可见校验的完整层集合。
    let active_revision = resources.active.revision; // 后续状态转换都对照同一版本值。
    resources
        .cache
        .rejected
        .retain(|(revision, _)| *revision >= active_revision); // 清理不会再次活动的旧拒绝代次。

    if active_revision == 0 || bound_layers.is_empty() {
        resources.cpu_build.cancel(); // 空布局或无消费者时废弃未登记后台候选。
        cancel_pending_batch(
            &mut resources.coordinator,
            &mut resources.request,
            &mut resources.images,
        ); // 空核心布局或无绑定窗口都不保留候选资产。
        resources.inbox.results.clear(); // 旧回调即使稍后到达也没有可采用消费者。
        return; // 不覆盖消费者原本直接绑定的密度资产。
    }

    if !resources.runtime.0 {
        resources.cpu_build.cancel(); // CPU逻辑回退不需要正式GPU候选后台任务。
        cancel_pending_batch(
            &mut resources.coordinator,
            &mut resources.request,
            &mut resources.images,
        ); // 无RenderApp逻辑测试不保留永远不会完成的GPU候选。
        if resources.cpu_build.is_busy() {
            return; // RenderApp状态切换时先等旧百MiB级任务真正退出，避免同步回退与它重叠占用CPU和内存。
        }
        sync_cpu_density_fallback(
            &mut commands,
            &resources.active,
            &resources.commit,
            &mut resources.images,
            &mut resources.cache,
            &mut resources.rejected,
            &mut windows,
        ); // CPU回退仍要求完整Mip生成成功后才修改窗口。
        return; // 真实渲染应用禁止绕过后续GPU回读门。
    }

    sync_gpu_density_candidates(&mut commands, &mut resources, &bound_layers, &mut windows); // RenderApp存在时执行候选分离、串行回读和整批原子采用。
}

/// 收集当前全部绑定层并转为稳定有序数组。
fn collect_bound_layers(
    windows: &mut Query<BladeDensityWindowQuery<'_>>, // 查询含可写Patch，因此使用顺序可变迭代。
    validation: &VegetationBladeValidationLayers,     // 正式场景可先校验而不替换任何旧窗口。
) -> Vec<VegetationLayerId> {
    let mut layers = BTreeSet::new(); // 同层多个LOD窗口只产生一个共享候选。
    for (_, binding, _, _) in windows.iter_mut() {
        layers.insert(binding.0); // BTreeSet同时去重并固定从小到大顺序。
    }
    layers.extend(validation.iter()); // 只校验层使用相同GPU输入、Mip和版本门但不参与窗口应用。
    layers.into_iter().collect() // 只在窗口集合或布局处理帧分配很小的层数组。
}

/// 驱动有RenderApp时的候选批次生命周期。
fn sync_gpu_density_candidates(
    commands: &mut Commands, // 完整批次成功后才用于窗口组件提交。
    resources: &mut BladeDensityMainResources, // 持有活动布局、候选资产和跨世界队列。
    bound_layers: &[VegetationLayerId], // 本帧必须一起采用的稳定层集合。
    windows: &mut Query<BladeDensityWindowQuery<'_>>, // 成功或已有当前缓存时应用全部窗口。
) {
    let revision = resources.active.revision; // GPU结果身份始终使用当前活动版本。
    let configuration_generation = resources.commit.configuration_generation(); // GPU候选必须使用生成活动布局的同代输入。
    if resources.commit.revision() != revision
        || configuration_generation != resources.layout.generation()
    {
        resources.cpu_build.cancel(); // 新配置尚未形成活动布局时丢弃旧代未登记CPU候选。
        cancel_pending_batch(
            &mut resources.coordinator,
            &mut resources.request,
            &mut resources.images,
        ); // 已登记但未公开的GPU候选也不能跨配置代继续采用。
        return; // 可见窗口继续保留直接密度或上一已验证缓存。
    }
    let identity = BladeDensityBuildIdentity::new(
        revision,                 // 锁定当前活动布局版本。
        configuration_generation, // 锁定当前正式输入代次。
        bound_layers,             // 锁定跨层原子集合。
    );
    let identity_changed = resources.coordinator.pending.as_ref().is_some_and(|batch| {
        batch.revision != revision
            || batch.configuration_generation != configuration_generation
            || batch.layer_ids().as_slice() != bound_layers // 换版、热更或绑定集合改变会使旧候选整体过时。
    });
    if identity_changed {
        cancel_pending_batch(
            &mut resources.coordinator,
            &mut resources.request,
            &mut resources.images,
        ); // 候选从未公开，取消只移除候选资产而不碰旧缓存。
    }
    if !resources.cpu_build.matches(&identity) {
        resources.cpu_build.cancel(); // 未登记CPU候选使用相同完整身份规则丢弃迟到结果。
    }

    consume_gpu_results(
        &mut resources.coordinator,
        &mut resources.request,
        &mut resources.inbox,
    ); // 迟到票据和非当前层结果在主世界无副作用丢弃。
    mark_timeout_if_needed(&mut resources.coordinator); // 超时与显式GPU错误走同一整批失败路径。

    if let Some(reason) = resources
        .coordinator
        .pending
        .as_ref()
        .and_then(|batch| batch.failure.clone())
    {
        reject_pending_batch(
            reason,
            &mut resources.coordinator,
            &mut resources.request,
            &mut resources.images,
            &mut resources.cache,
            &mut resources.rejected,
        ); // 任一层失败后所有候选图片一次释放且旧窗口保持不变。
        return; // 同一版本通过拒绝集合禁止下一帧立刻重试。
    }

    if resources
        .coordinator
        .pending
        .as_ref()
        .is_some_and(PendingBladeDensityBatch::is_complete)
    {
        promote_pending_batch(
            commands,
            &mut resources.coordinator,
            &mut resources.request,
            &mut resources.images,
            &mut resources.cache,
            windows,
        ); // 全层通过后在同一主世界系统原子替换缓存与窗口。
        return; // 新候选已成为当前有效图，不再发布额外作业。
    }

    if resources.coordinator.pending.is_none() {
        let all_current = bound_layers.iter().all(|layer| {
            resources
                .cache
                .layers
                .get(layer)
                .is_some_and(|cached| cached.revision == revision) // 晚创建LOD窗口可以直接复用已验收当前图。
        });
        if all_current {
            resources.cpu_build.cancel(); // 防御外部已提供当前缓存时遗留无用后台任务。
            apply_current_cache(commands, revision, &resources.cache, windows); // 只修复缺少组件或切换绑定层的新窗口，不创建GPU作业。
            return;
        }
        if bound_layers
            .iter()
            .any(|layer| resources.cache.rejected.contains(&(revision, *layer)))
        {
            resources.cpu_build.cancel(); // 已拒绝版本禁止遗留任务下一帧重新登记候选。
            return; // 已失败版本持续使用上一有效窗口，禁止每帧重复分配候选。
        }
        if resources.cpu_build.matches(&identity) {
            let Some(completed) = resources.cpu_build.poll() else {
                return; // 后台纯CPU候选未完成时主线程不等待，也不登记部分Image。
            };
            if completed.identity != identity {
                return; // 热更同帧产生的过期成功或失败结果都无副作用丢弃。
            }
            match completed.result {
                Ok(built) => {
                    if completed.ticket != resources.coordinator.next_ticket
                        || !built.matches(
                            completed.ticket,
                            identity.revision,
                            identity.configuration_generation,
                        )
                    {
                        return; // 票据或产物身份异常时未登记Image直接丢弃，下一帧使用新票据重建。
                    }
                    resources.coordinator.pending = Some(register_built_batch(
                        built,                              // 全部CPU步骤已成功且身份仍为当前。
                        resources.coordinator.update_index, // GPU超时从本次登记帧开始。
                        &mut resources.images,              // 主线程一次登记全部候选图片。
                    ));
                }
                Err((layer, reason)) => {
                    if resources.cache.rejected.insert((revision, layer)) {
                        resources
                            .rejected
                            .write(VegetationLayoutRejected { reason }); // 当前身份CPU失败才记录一次拒绝。
                    }
                    return; // 失败产物从未向Assets表加入任何部分候选。
                }
            }
        } else {
            if resources.cpu_build.is_busy() {
                return; // 已标记过期的大任务真正结束前保持single-flight并继续使用旧窗口。
            }
            let ticket = next_gpu_ticket(); // 多个场景各自拥有协调器，GPU跨世界票据仍必须全局唯一。
            resources.coordinator.next_ticket = ticket; // 启动CPU构建时立即消费票据以拒绝迟到结果。
            if VegetationBladeDensityCpuBuild::should_start_async(resources.active.tiles.len()) {
                let started = resources.cpu_build.start(
                    identity.clone(),                  // 保存登记前必须再次核对的完整身份。
                    ticket,                            // 保存本次唯一票据。
                    resources.active.as_ref().clone(), // 块密度通过Arc共享，不复制完整布局。
                    resources.commit.as_ref().clone(), // 固定完整脏块提交清单。
                    resources.layout.as_ref().clone(), // 源像素通过Arc共享并锁定配置代。
                    bound_layers.to_vec(),             // 层数很小且后台任务必须拥有。
                    resources.cache.as_ref().clone(),  // 上一图集通过Arc共享供局部补丁。
                );
                debug_assert!(started, "idle density CPU slot must accept one task"); // is_busy门保证正式路径不会覆盖旧任务。
                return; // 本帧只启动任务，不在主线程执行任何整图CPU构建。
            }
            let result = build_pending_batch_cpu(PendingBladeDensityBatchBuild {
                ticket,                    // 小场景同步路径沿用相同票据语义。
                active: &resources.active, // 小场景直接借用当前活动布局。
                commit: &resources.commit, // 小场景直接借用同版本提交。
                layout: &resources.layout, // 小场景直接借用受检输入。
                bound_layers,              // 小场景仍要求完整跨层集合。
                cache: &resources.cache,   // 小场景仍可使用上一局部图集。
            });
            match result {
                Ok(built) => {
                    resources.coordinator.pending = Some(register_built_batch(
                        built,                              // 同步小候选也先完成所有CPU步骤。
                        resources.coordinator.update_index, // GPU超时从资产登记帧开始。
                        &mut resources.images,              // 成功后才批量登记图片。
                    ));
                }
                Err((layer, reason)) => {
                    if resources.cache.rejected.insert((revision, layer)) {
                        resources
                            .rejected
                            .write(VegetationLayoutRejected { reason }); // 小场景CPU失败保持同一拒绝契约。
                    }
                    return; // 同步失败也没有部分Image进入Assets。
                }
            }
        }
    }

    publish_next_gpu_job(&mut resources.coordinator, &mut resources.request); // 一个层完成并通过后才发布稳定顺序中的下一层。
}

/// 资源快照恢复旧协调器时也不回退票据，避免另一个场景的迟到回读命中同层/同修订。
fn next_gpu_ticket() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 把稳定顺序中的下一等待层发布为唯一RenderApp请求。
fn publish_next_gpu_job(
    coordinator: &mut VegetationGpuMipCoordinator, // 读取当前批次并推进单层状态。
    request: &mut VegetationGpuMipRequest,         // 写入下一帧提取的轻量作业。
) {
    if request.job.is_some() {
        return; // 当前层尚未回读时禁止第二层并发占用额外staging显存。
    }
    let Some(batch) = coordinator.pending.as_mut() else {
        return; // 稳定布局没有候选作业。
    };
    let Some(layer) = batch.next_waiting_layer() else {
        return; // 全部层已请求或通过，等待结果/最终提交。
    };
    let pending = batch
        .layers
        .get_mut(&layer)
        .expect("next waiting vegetation layer must exist in pending batch"); // 键来自同一BTreeMap。
    let width = match pending.cached.atlas.base_width() {
        Ok(width) => width, // CPU构建已经验证基础级存在。
        Err(reason) => {
            batch.failure = Some(reason); // 防御损坏状态并沿用旧活动缓存。
            return;
        }
    };
    let height = match pending.cached.atlas.base_height() {
        Ok(height) => height, // 非正方形高度独立传给RenderApp。
        Err(reason) => {
            batch.failure = Some(reason); // 防御损坏状态并沿用旧活动缓存。
            return;
        }
    };
    let mip_level_count = match pending.cached.atlas.mip_level_count() {
        Ok(levels) => levels, // 级数必须与完整回读计划一致。
        Err(reason) => {
            batch.failure = Some(reason); // 防御损坏状态并沿用旧活动缓存。
            return;
        }
    };
    pending.state = PendingBladeDensityState::Requested; // 先推进阶段再发布可提取请求。
    request.job = Some(VegetationGpuMipJob {
        ticket: batch.ticket,                // 当前批次唯一票据。
        revision: batch.revision,            // 活动版本在最终采用前仍会再次核对。
        layer,                               // 一个请求只包含当前稳定层。
        image: pending.cached.image.clone(), // 强句柄保证RenderAsset与回读完成前候选存活。
        width,                               // 验证GPU基础级宽。
        height,                              // 验证GPU基础级高。
        mip_level_count,                     // 验证完整链级数。
        base: pending.base.clone(),          // 正式运行携带GPU竞争输入与局部旧图复制计划。
    });
}

/// 将全部已验证候选一次替换为活动缓存并更新所有窗口。
fn promote_pending_batch(
    commands: &mut Commands,                       // 同一系统批量提交窗口组件。
    coordinator: &mut VegetationGpuMipCoordinator, // 取得并移除完整候选批次。
    request: &mut VegetationGpuMipRequest,         // 最终成功后清空提取请求。
    images: &mut Assets<Image>,                    // 新图保留，旧内部图在窗口换版同帧释放。
    cache: &mut VegetationBladeDensityCache,       // 原子替换每层当前有效记录。
    windows: &mut Query<BladeDensityWindowQuery<'_>>, // 采用同版本全部绑定窗口。
) {
    let Some(batch) = coordinator.pending.take() else {
        return; // 防御重复提交。
    };
    if !batch.is_complete() {
        coordinator.pending = Some(batch); // 非完整批次绝不被意外采用。
        return;
    }
    let revision = batch.revision; // 全部候选共享同一活动版本。
    request.job = None; // 不再提取已经成为活动图的作业。
    for (layer, pending) in batch.layers {
        let new_image_id = pending.cached.image.id(); // 插入前保存新候选资产身份。
        if let Some(previous) = cache.layers.insert(layer, pending.cached)
            && previous.image.id() != new_image_id
        {
            images.remove(previous.image.id()); // 新候选已完整驻留并通过后才释放上一内部图片。
        }
        cache.rejected.remove(&(revision, layer)); // 成功版本不保留过去同键拒绝记录。
    }
    apply_current_cache(commands, revision, cache, windows); // 图片与世界映射在同一主世界Update被全部窗口采用。
}

/// 取消未公开候选并保留所有当前缓存和窗口。
fn cancel_pending_batch(
    coordinator: &mut VegetationGpuMipCoordinator, // 移除候选批次所有权。
    request: &mut VegetationGpuMipRequest,         // 停止继续提取旧票据。
    images: &mut Assets<Image>,                    // 释放从未被窗口采用的候选图片。
) {
    request.job = None; // RenderApp下一次提取看到无当前作业。
    let Some(batch) = coordinator.pending.take() else {
        return; // 没有候选时不触碰资产表。
    };
    for pending in batch.layers.into_values() {
        images.remove(pending.cached.image.id()); // 候选句柄与当前缓存分离，删除不会影响旧活动图。
    }
}

/// 拒绝整个候选批次并为其中全部层记录当前版本失败。
fn reject_pending_batch(
    reason: String,                                // 首个GPU错误、差异或超时原因。
    coordinator: &mut VegetationGpuMipCoordinator, // 提供批次层集合和候选资产。
    request: &mut VegetationGpuMipRequest,         // 拒绝后停止提取当前作业。
    images: &mut Assets<Image>,                    // 删除全部未公开候选。
    cache: &mut VegetationBladeDensityCache,       // 只写拒绝集合，当前层缓存保持原值。
    rejected: &mut MessageWriter<VegetationLayoutRejected>, // 整批只发布一条结构化诊断。
) {
    let Some(batch) = coordinator.pending.take() else {
        return; // 防御重复拒绝。
    };
    request.job = None; // 停止RenderApp继续观察失败作业。
    for (layer, pending) in batch.layers {
        cache.rejected.insert((batch.revision, layer)); // 任一绑定层查询都能阻止同版本重试。
        images.remove(pending.cached.image.id()); // 失败候选从未绑定窗口，可安全直接释放。
    }
    rejected.write(VegetationLayoutRejected { reason }); // 上层可以统一记录或展示迁移门失败原因。
}

/// 把当前已验收缓存应用到缺失或身份变化的窗口。
fn apply_current_cache(
    commands: &mut Commands,             // 插入强图片句柄和私有采用身份。
    revision: u64,                       // 所有目标缓存必须与该活动版本相同。
    cache: &VegetationBladeDensityCache, // 只读已经通过GPU或CPU逻辑回退的当前缓存。
    windows: &mut Query<BladeDensityWindowQuery<'_>>, // 同层多个LOD共享一个句柄。
) {
    for (entity, binding, mut patch, applied) in windows.iter_mut() {
        if applied.is_some_and(|applied| {
            applied.revision == revision && applied.layer == binding.0 // 同版本切换绑定层也会重新采用。
        }) {
            continue; // 稳定窗口不产生组件写入或变更检测噪声。
        }
        let Some(cached) = cache.layers.get(&binding.0) else {
            continue; // 某层尚未成功时窗口继续保留原直接绑定或上一版本图。
        };
        if cached.revision != revision {
            continue; // GPU候选处理中绝不采用上一缓存冒充当前版本。
        }
        patch.set_density_field(cached.world_min, cached.world_max); // 与图片在同一系统采用同源世界映射。
        commands.entity(entity).insert((
            GpuBladeDensityMap::new(cached.image.clone()), // 强句柄让正式RenderAsset持续可达。
            VegetationBladeLayoutRevision {
                revision,         // 记录真正采用的活动版本。
                layer: binding.0, // 记录真正采用的层身份。
            },
        )); // 候选GPU图片已经驻留并逐字节通过，不再触发错误纹理闪现。
    }
}

#[cfg(test)]
mod tests; // 主世界票据、原子提交和失败托底回归拆到子文件。
