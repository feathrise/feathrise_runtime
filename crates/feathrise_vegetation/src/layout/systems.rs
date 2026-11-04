//! 布局消息收集、预算化重算和原子版本切换。

use feathrise_scene_core::residency::SceneResidencyAppExt;

use std::sync::Arc; // 活动布局通过共享块避免异步暂存深拷贝完整密度。

use bevy::prelude::{App, IntoScheduleConfigs, MessageReader, MessageWriter, ResMut, Update}; // 两个串行系统在主世界按事件驱动运行。

use crate::{MarkVegetationDirty, VegetationLayoutRejected}; // 外部只通过消息标记局部变化。

use super::async_compute::VegetationLayoutAsyncWork; // 大批布局由独立任务槽跨帧执行和轮询。
use super::compose::compose_tile; // 单块合成是无ECS副作用的纯函数。
use super::resources::{
    ActiveVegetationLayout, PendingVegetationLayout, VegetationLayoutCommit, VegetationLayoutState,
}; // 系统在活动、提交快照与暂存布局之间原子切换。

pub(super) fn install(app: &mut App) {
    app.init_resident_resource::<ActiveVegetationLayout>() // 后端始终可读取一个有效布局快照。
        .init_resident_resource::<VegetationLayoutCommit>() // GPU后端取得与活动版本一致的完整脏块集合。
        .init_resident_resource::<VegetationLayoutState>() // 注册配置、源和脏块状态。
        .init_resident_resource::<VegetationLayoutAsyncWork>() // 大批CPU合成任务不进入可克隆公共状态。
        .add_message::<MarkVegetationDirty>() // 项目和资产热重载发送局部变更。
        .add_message::<VegetationLayoutRejected>() // 诊断订阅原子拒绝原因。
        .add_resident_systems(
            Update,
            (collect_dirty_messages, recompute_dirty_tiles).chain(), // 同帧先合并请求再按预算重算。
        );
}

fn collect_dirty_messages(
    mut messages: MessageReader<MarkVegetationDirty>,
    mut state: ResMut<VegetationLayoutState>,
) {
    for message in messages.read() {
        state.queue_dirty_rect(message.min_tile, message.max_tile); // 重叠和重复请求由稳定集合合并。
    }
}

pub(crate) fn recompute_dirty_tiles(
    mut state: ResMut<VegetationLayoutState>,
    mut active: ResMut<ActiveVegetationLayout>,
    mut commit: ResMut<VegetationLayoutCommit>,
    mut async_work: ResMut<VegetationLayoutAsyncWork>,
    mut rejected: MessageWriter<VegetationLayoutRejected>,
) {
    if state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.generation != state.generation())
    {
        state.requeue_pending_batch(); // 旧批原始全集与本代新dirty合并，禁止热更漏算尚未完成区域。
        async_work.cancel(); // 正在运行的旧代worker也不得继续占用后台核心或返回结果。
    }
    if state.tile_budget_per_frame == 0 {
        async_work.cancel(); // 零预算同时暂停尚未提交的后台工作，活动布局保持不变。
        return; // 消费者恢复非零预算后会从完整dirty集合重新启动本批。
    }
    if state.pending.is_none() {
        let dirty = state.take_pending_dirty(); // 没有请求时布局稳定且不产生逐帧竞争开销。
        if dirty.is_empty() {
            return;
        }
        if let Err(error) =
            crate::competition::validate_configuration(state.groups(), state.layers())
        {
            reject(&mut state, &mut rejected, error); // 非法配置不创建任何暂存块。
            return;
        }
        let texel_size = match state.finest_texel_size() {
            Ok(value) => value, // 全部源已就绪后选择最高精度输入。
            Err(error) => {
                reject(&mut state, &mut rejected, error); // 未上传资产安全保留活动版本。
                return;
            }
        };
        let mut staging = active.clone(); // 未变块继续复用上一完整版本。
        staging.texel_size = texel_size; // 当前批次所有块共享一个固定世界分辨率。
        let committed_dirty = dirty.iter().copied().collect(); // 在逐帧消费前锁定稳定全集供成功提交。
        state.pending = Some(PendingVegetationLayout {
            generation: state.generation(), // 锁定本批次一致输入代次。
            requested_revision: active.revision.wrapping_add(1), // 成功后只增加一次活动版本。
            staging,
            dirty,
            committed_dirty, // 后续预算处理只修改remaining集合，不丢失原始提交范围。
        });
    }
    let (generation, base_revision, requested_revision, texel_size, dirty_count) = {
        let pending = state.pending.as_ref().expect("pending layout must exist"); // 新建或既有批次都在此锁定身份。
        (
            pending.generation,         // 后台结果必须来自当前配置代次。
            active.revision,            // staging基于当前完整活动版本创建。
            pending.requested_revision, // 整批只能发布预先锁定的新revision。
            pending.staging.texel_size, // 所有worker共享同一世界纹素尺寸。
            pending.dirty.len(),        // 先只读取规模，小局部更新不复制输入快照或分配块向量。
        )
    };
    if !async_work.matches(generation, base_revision, requested_revision) {
        async_work.cancel(); // 身份不匹配的迟到任务不能进入当前staging。
        if VegetationLayoutAsyncWork::should_start(dirty_count) {
            let dirty = state
                .pending
                .as_ref()
                .expect("pending layout must exist")
                .dirty
                .iter()
                .copied()
                .collect::<Vec<_>>(); // 只有整图批次才创建一次稳定worker序列。
            let (groups, layers, sources) = state.cloned_inputs(); // 大源像素通过Arc共享，不在启动帧深拷贝。
            async_work.try_start(
                generation,         // 锁定配置与源字段代次。
                base_revision,      // 锁定活动布局基线。
                requested_revision, // 锁定待发布版本。
                texel_size,         // 锁定本批世界纹素尺寸。
                dirty,              // 交出完整稳定脏块序列。
                groups,             // 交出受检竞争组快照。
                layers,             // 交出受检层配置快照。
                sources,            // 交出共享源字段快照。
            );
        }
    }
    if async_work.matches(generation, base_revision, requested_revision) {
        let Some(completed) = async_work.poll() else {
            return; // worker未全部结束时本帧不做任何布局CPU重活或部分发布。
        };
        if completed.generation != state.generation()
            || completed.base_revision != active.revision
            || completed.requested_revision != requested_revision
        {
            return; // 同帧外部身份变化时直接丢弃迟到结果并保留上一活动版本。
        }
        if completed.results.len() != completed.expected_count {
            reject(
                &mut state,
                &mut rejected,
                crate::VegetationConfigError::InvalidValue {
                    context: "vegetation async layout batch completed without every requested tile"
                        .to_owned(), // 缺块结果绝不能被当作完整原子提交。
                },
            );
            return;
        }
        for (tile, result) in completed.results {
            let resolved = match result {
                Ok(resolved) => resolved, // 全部worker成功块按Y/X稳定合入不可见staging。
                Err(error) => {
                    reject(&mut state, &mut rejected, error); // 任一块错误仍废弃整批。
                    return;
                }
            };
            let pending = state.pending.as_mut().expect("pending layout must exist"); // 结果身份已与当前批次完全核对。
            pending.dirty.remove(&tile); // 仅完整结果返回后才消费待办块。
            pending.staging.tiles.insert(tile, Arc::new(resolved)); // 新块共享给后续GPU候选构建，活动版本仍不可见。
        }
    } else {
        let budget = state.tile_budget_per_frame; // 小局部更新继续用既有逐帧预算避免任务调度成本。
        for _ in 0..budget {
            let Some(tile) = state
                .pending
                .as_ref()
                .and_then(|pending| pending.dirty.first().copied())
            else {
                break; // 当前批次全部块已经完成。
            };
            let result = {
                let (groups, layers, sources) = state.inputs(); // 只读借用在单块合成后立即结束。
                compose_tile(tile, texel_size, groups, layers, sources)
            };
            match result {
                Ok(resolved) => {
                    let pending = state.pending.as_mut().expect("pending layout must exist");
                    pending.dirty.remove(&tile); // 成功块从待办集合移除。
                    pending.staging.tiles.insert(tile, Arc::new(resolved)); // 小批也保持共享块存储与原子可见性。
                }
                Err(error) => {
                    reject(&mut state, &mut rejected, error); // 任意块失败废弃整批暂存。
                    return;
                }
            }
        }
    }
    let completed = state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.dirty.is_empty()); // 只有全部脏块完成才允许切换。
    if completed {
        let mut pending = state
            .pending
            .take()
            .expect("completed pending layout must exist");
        pending.staging.revision = pending.requested_revision; // 整批只生成一个新版本号。
        let committed_revision = pending.requested_revision; // 保存移动暂存布局后的提交身份。
        let committed_generation = pending.generation; // 后端只可把该代配置应用到本次活动布局。
        let committed_dirty = pending.committed_dirty; // 取得开始批次时锁定的全部稳定脏块。
        *active = pending.staging; // 单次资源替换完成原子切换。
        commit.publish(
            committed_revision,   // 发布与活动布局相同的revision。
            committed_generation, // 发布实际完成合成的配置与源代次。
            committed_dirty,      // 发布本批次完整脏块全集。
        ); // 与活动布局同帧发布GPU与实体后端共享的原子提交契约。
        state.last_error = None; // 成功版本清除旧错误快照。
    }
}

fn reject(
    state: &mut VegetationLayoutState,
    rejected: &mut MessageWriter<VegetationLayoutRejected>,
    error: crate::VegetationConfigError,
) {
    state.pending = None; // 丢弃所有部分完成暂存块。
    state.last_error = Some(error.clone()); // F3或测试可读取结构化拒绝原因。
    rejected.write(VegetationLayoutRejected {
        reason: error.to_string(), // 消息保持轻量且无需错误类型生命周期。
    });
}

#[cfg(test)]
mod tests; // 布局事件、失败托底与原子换版回归独立维护。
