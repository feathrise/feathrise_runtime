//! 退役placement实体树的固定预算非递归回收。

use bevy::prelude::*; // exclusive系统逐个静默、断开父子关系并销毁叶实体。

use super::super::super::resources::ScenePlacementRuntimeState; // 唯一运行时状态持有退役树队列。
use super::PLACEMENT_TREE_CLEANUP_BUDGET_PER_FRAME; // 每帧树操作硬预算由退役功能域统一声明。
use super::super::super::components::{ScenePlacementInstance, PlacementPendingInstance, PlacementPendingRevision};

/// 每帧以固定操作数静默并回收退役树，绝不对仍有Children的实体调用linked-spawn递归despawn。
pub(crate) fn cleanup_retired_placement_trees(world: &mut World) {
    let mut retired = {
        let mut state = feathrise_scene_core::residency::instance_resource_mut::<ScenePlacementRuntimeState>(world);
        std::mem::take(&mut state.retired_trees) // 暂时移出队列，避免exclusive World访问与资源借用重叠。
    };
    let mut budget = PLACEMENT_TREE_CLEANUP_BUDGET_PER_FRAME;
    while budget > 0 {
        let Some(mut cleanup) = retired.pop_front() else {
            break;
        };
        while budget > 0 && cleanup.quiesce_cursor < cleanup.roots.len() {
            let root = cleanup.roots[cleanup.quiesce_cursor];
            cleanup.quiesce_cursor += 1;
            budget -= 1; // 每个直接placement根的静默化计为一个固定预算操作。
            if world.get::<ScenePlacementInstance>(root).is_some_and(ScenePlacementInstance::is_active) {
                // 已由新批次接管或失败后仍属于旧活动批次的根，不注销WorldAssetRoot和玩法状态。
                if let Some((carry, parent)) = cleanup.restore_carry
                    && world.get::<ChildOf>(root).map(ChildOf::parent) == Some(carry)
                    && world.get_entity(parent).is_ok() {
                    world.entity_mut(root).insert(ChildOf(parent))
                        .remove::<(PlacementPendingInstance, PlacementPendingRevision)>();
                }
                continue;
            }
            if let Ok(mut entity) = world.get_entity_mut(root) {
                entity.insert(Visibility::Hidden);
                entity.remove::<WorldAssetRoot>(); // 先注销World实例源，回收期间GLB热更不能继续长出新后代。
            }
        }
        if cleanup.quiesce_cursor < cleanup.roots.len() {
            retired.push_front(cleanup);
            break; // 本帧预算耗尽，保留精确游标到下一帧。
        }
        if let Some((carry, _)) = cleanup.restore_carry.take() {
            // 每个失败候选只新增一个carry；全部根归还后清空壳，重复重试不会累积容器。
            if world.get::<Children>(carry).is_none_or(|children| children.is_empty()) {
                let _ = world.despawn(carry);
            }
        }
        if cleanup.stack.is_empty() && world.get_entity(cleanup.root).is_ok() {
            cleanup.stack.push(cleanup.root); // DFS栈只随树深增长，不一次物化整棵实体表。
        }
        while budget > 0 {
            let Some(&entity) = cleanup.stack.last() else {
                break;
            };
            if world.get_entity(entity).is_err() {
                cleanup.stack.pop();
                budget -= 1; // 外部提前销毁同样只消费一个幂等处理额度。
                continue;
            }
            if world.get::<WorldAssetRoot>(entity).is_some() {
                let mut entity_mut = world.entity_mut(entity);
                entity_mut.insert(Visibility::Hidden);
                entity_mut.remove::<WorldAssetRoot>(); // 嵌套World根也先停止热重载，再进入子树断边。
                budget -= 1;
                continue;
            }
            let child = world
                .get::<Children>(entity)
                .and_then(|children| children.last().copied()); // 从尾部取子节点使关系hook保持O(1)。
            if let Some(child) = child {
                if let Ok(mut child_mut) = world.get_entity_mut(child) {
                    child_mut.insert(Visibility::Hidden);
                    child_mut.remove::<ChildOf>(); // 立即断开linked-spawn关系，父节点最终despawn不会递归整棵树。
                    cleanup.stack.push(child);
                }
                budget -= 1;
                continue;
            }
            let _ = world.despawn(entity); // 仅在确认无Children后销毁单个叶节点。
            cleanup.stack.pop();
            budget -= 1;
        }
        let finished = cleanup.stack.is_empty() && world.get_entity(cleanup.root).is_err();
        if !finished {
            retired.push_front(cleanup);
            break;
        }
    }
    world
        .resource_mut::<ScenePlacementRuntimeState>()
        .retired_trees = retired; // exclusive处理完成后一次归还剩余所有权。
}
