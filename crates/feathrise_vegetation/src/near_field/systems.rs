//! 近场观察变化检测、空间索引采用、确定性合并和消息发布系统。

use std::collections::BTreeMap; // worker局部Vec最终在主线程转换为稳定活动集合。

use bevy::prelude::{
    // ECS访问仅存在于本系统，纯候选worker不持有World借用。
    GlobalTransform, // 观察者读取已经传播完成的世界位置。
    Local,           // 复用观察者位置缓冲避免稳定移动路径重复申请Vec。
    MessageWriter,   // Entered和Exited消息严格在主线程串行写入。
    Query,           // 只查询带近场观察者标记的世界变换。
    Res,             // 活动布局与配置状态只读访问。
    ResMut,          // 近场运行资源在完整结果形成后一次更新。
    Vec2,            // 近场距离仅使用世界X/Z二维位置。
    Vec3Swizzles,    // 从GlobalTransform平移提取XZ。
    With,            // 查询过滤近场观察者标记。
}; // 完成近场ECS系统依赖导入。

use crate::{
    // 系统只负责资源身份、消息契约和最终差分。
    ActiveVegetationLayout,     // 当前完整活动布局revision与密度采样。
    VegetationLayoutState,      // 当前层配置、源代次和独立配置代次。
    VegetationNearFieldEntered, // 新增或变更代理的进入消息。
    VegetationNearFieldExited,  // 越界、删除或变更代理的退出消息。
}; // 完成核心资源和消息导入。

use super::collection::{build_layer_work, collect_near_field_candidates}; // 纯层计算可按工作量串行或有界并行。
use super::{VegetationNearFieldObserver, VegetationNearFieldRuntime}; // 系统独占维护近场观察者与运行资源。

pub(super) fn update_near_field_proxies(
    // 唯一ECS系统确保消息和运行集合按稳定顺序原子更新。
    observers: Query<&GlobalTransform, With<VegetationNearFieldObserver>>, // 多观察者只读取世界位置。
    layout: Res<ActiveVegetationLayout>, // 当前活动布局在整个候选计算期间保持只读。
    state: Res<VegetationLayoutState>,   // 当前层配置与源变化身份保持只读。
    mut runtime: ResMut<VegetationNearFieldRuntime>, // 活动滞回集合和显式索引跨帧保存。
    mut entered: MessageWriter<VegetationNearFieldEntered>, // 主线程串行发布进入消息。
    mut exited: MessageWriter<VegetationNearFieldExited>, // 主线程串行发布退出消息。
    mut observer_positions: Local<Vec<Vec2>>, // 两份Vec通过swap跨帧复用容量。
) {
    // 系统返回前始终形成一个完整新活动集合或保持上一集合不变。
    observer_positions.clear(); // 清空上次交换回来的旧观察者内容但保留容量。
    observer_positions.extend(
        // 一次填充当前全部观察者世界XZ。
        observers // 遍历所有明确标记的近场观察者。
            .iter() // Query只读迭代不会修改Transform。
            .map(|transform| transform.translation().xz()), // 世界Y高度不参与平面近场距离。
    ); // 完成观察者位置收集。
    observer_positions.sort_by_key(|position| (position.x.to_bits(), position.y.to_bits())); // 多观察者生成顺序不影响变化检测或结果。
    let state_generation = state.generation(); // 配置或密度源任一变化都必须重新判断最终启用集合。
    if runtime.last_layout_revision == layout.revision // 活动布局没有切换。
        && runtime.last_state_generation == state_generation // 配置和密度源没有变化。
        && runtime.last_observers == *observer_positions
    // 所有观察者位置逐位相同。
    {
        // 三份身份稳定时无需估算、查询或派发任何任务。
        return; // 保持当前活动集合和消息队列不变。
    } // 完成稳定帧快速路径。
    runtime.explicit_indexes.rebuild_if_needed(
        // 仅层配置代次变化时重建静态显式落点索引。
        state.configuration_generation(), // 密度源上传不会推进该独立代次。
        state.layers(),                   // 使用同一代已经受检的完整层配置。
    ); // 完成显式空间索引与当前层配置对齐。
    let layer_work = build_layer_work(
        // 为每个HybridNearField层创建稳定只读工作描述。
        state.layers(),            // 当前完整层配置切片。
        &runtime.explicit_indexes, // 当前配置代显式落点索引。
        &observer_positions,       // 当前稳定排序观察者位置。
    ); // 完成工作量估算和稳定层排序。
    let entries = collect_near_field_candidates(
        // 小工作串行，大工作按层在本帧有界并行。
        &layer_work,         // 当前稳定层ID升序工作列表。
        &observer_positions, // 当前只读观察者位置。
        &layout,             // 当前活动布局共享只读采样。
        &runtime.active,     // 上一完整集合决定每个候选使用进入或退出半径。
    ); // 全部worker完成后取得稳定排序去重局部结果。
    let discovered = entries // 主线程独占构造最终有序活动集合。
        .into_iter() // 稳定联合主键升序消费局部结果。
        .collect::<BTreeMap<_, _>>(); // 不使用Mutex或跨线程共享BTreeMap写入。
    publish_difference(
        // 消息发布严格发生在完整新集合形成之后。
        &runtime,     // 读取上一完整活动集合。
        &discovered,  // 读取本次完整目标集合。
        &mut entered, // 串行写入进入消息。
        &mut exited,  // 串行写入退出消息。
    ); // 完成稳定代理差分消息发布。
    runtime.active = discovered; // 消息完成后一次替换当前滞回集合。
    runtime.last_layout_revision = layout.revision; // 锁定本次差分对应的活动布局。
    runtime.last_state_generation = state_generation; // 锁定本次差分对应的配置和密度源代次。
    std::mem::swap(&mut runtime.last_observers, &mut *observer_positions); // 交换已分配缓冲供下帧继续复用容量。
} // 完成近场代理更新系统。

fn publish_difference(
    // 纯串行消息阶段保持原有先变更后删除的稳定顺序。
    runtime: &VegetationNearFieldRuntime, // 上一完整活动集合。
    discovered: &BTreeMap<
        // 本次完整活动目标集合。
        (crate::VegetationLayerId, u64), // 稳定联合主键类型。
        super::ActiveNearFieldProxy,     // 当前完整代理描述类型。
    >, // 完成本次目标集合参数类型。
    entered: &mut MessageWriter<VegetationNearFieldEntered>, // 新增或变更进入消息写入器。
    exited: &mut MessageWriter<VegetationNearFieldExited>, // 删除或变更退出消息写入器。
) {
    // 所有BTreeMap迭代按层ID和实例ID稳定升序。
    for ((layer, instance_id), proxy) in discovered {
        // 先处理新增和同ID描述变化。
        let key = (*layer, *instance_id); // 复制稳定联合主键供旧集合查找。
        if runtime.active.get(&key) != Some(proxy) {
            // 完全相同代理无需重复消息。
            if runtime.active.contains_key(&key) {
                // 同ID原型或变换变化需先回收旧代理。
                exited.write(VegetationNearFieldExited {
                    // 发布旧代理退出消息。
                    layer: *layer,             // 保留当前稳定层身份。
                    instance_id: *instance_id, // 保留当前稳定实例身份。
                }); // 完成同ID旧代理退出消息。
            } // 完成同ID变更预回收。
            entered.write(VegetationNearFieldEntered {
                // 新增或变更代理发布完整进入描述。
                layer: *layer,              // 消费者可为不同层安装不同玩法适配器。
                prototype: proxy.prototype, // 原型不含树木或岩石类别枚举。
                instance_id: *instance_id,  // 与GPU候选共享稳定身份。
                transform: proxy.transform, // 近场代理位置必须与远场一致。
            }); // 完成当前代理进入消息。
        } // 完成当前目标代理差分判断。
    } // 完成全部新增或变化代理消息。
    for (layer, instance_id) in runtime.active.keys() {
        // 再处理目标集合中已经不存在的旧代理。
        if !discovered.contains_key(&(*layer, *instance_id)) {
            // 越过外圈、离图或层删除统一回收。
            exited.write(VegetationNearFieldExited {
                // 发布稳定退出消息。
                layer: *layer,             // 保留旧代理所属层身份。
                instance_id: *instance_id, // 保留旧代理稳定实例身份。
            }); // 完成当前缺失代理退出消息。
        } // 完成当前旧代理保留判断。
    } // 完成全部旧代理退出差分。
} // 完成近场消息差分发布。
