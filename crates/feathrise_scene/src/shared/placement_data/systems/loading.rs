//! 去重WorldAsset依赖的事件归属与固定预算就绪检查。
//!
//! 【功能设计】多个placement可复用同一GLB，因此加载门只跟踪唯一
//! `AssetId<WorldAsset>`。资产事件用HashSet完成O(1)归属；稳定帧只从
//! 去重队列弹出帧开始时的固定数量，不再对八千条计划执行`any/all`。

use bevy::asset::{AssetId, DependencyLoadState, LoadState, RecursiveDependencyLoadState}; // 直接根、一层依赖与递归依赖任一失败都必须关闭候选。
use bevy::prelude::*; // AssetServer提供非阻塞状态查询。
use bevy::world_serialization::WorldAsset; // 加载跟踪只处理GLTF Scene0的WorldAsset。

use super::super::resources::PlacementSceneLoadTracker; // 唯一跟踪器持有去重集、Ready信用与检查队列。

pub(super) const PLACEMENT_SCENE_LOAD_CHECK_BUDGET_PER_FRAME: usize = 128; // 每帧最多读取一百二十八个唯一Scene依赖状态。

/// AssetServer三层状态收口后的有限结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlacementSceneDependencyState {
    Waiting, // 未失败但尚未完整Ready，继续保留队列席位。
    Ready,   // 直接与所有递归依赖已完整就绪。
    Failed,  // 任一层加载失败，保留旧活动批次并等待匹配资产事件重试。
}

/// 本帧队列轮询的结果和真实进度证据。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PlacementSceneLoadAdvance {
    pub(super) checked: usize, // 用于测试证明固定查询上限，不作为heartbeat。
    pub(super) newly_ready: usize, // 只有首次获得Ready信用才是真实进度。
    pub(super) all_ready: bool, // Ready集合与归属集数量相等时O(1)完成。
    pub(super) failed: bool,   // 本帧发现任一明确失败资产。
}

/// 把一个属于当前修订的Scene ID排入一次有界复查。
///
/// 返回值只表示队列或Ready信用真实变化，重复事件不续接heartbeat。
pub(super) fn queue_scene_check(
    tracker: &mut PlacementSceneLoadTracker, // 当前候选唯一跟踪器。
    id: AssetId<WorldAsset>,                 // 已经O(1)归属成功的资产ID。
    invalidate_ready: bool,                  // Added/Modified/Removed使旧依赖Ready信用立即失效。
) -> bool {
    let readiness_changed = invalidate_ready && tracker.ready_scene_ids.remove(&id); // 热更间隙不允许旧Ready继续放行。
    let queued = tracker.queued_scene_ids.insert(id); // 同一ID在队列中最多出现一次。
    if queued {
        tracker.scene_checks.push_back(id); // 事件仅做O(1)追加，真实AssetServer查询留到有界轮询系统。
    }
    readiness_changed || queued // 稳定重复事件两项都为false。
}

/// 查询生产AssetServer中一个Scene的完整依赖状态。
pub(super) fn scene_dependency_state(
    asset_server: &AssetServer, // 查询只读不阻塞，不等待IO。
    id: AssetId<WorldAsset>,    // 队列当前唯一Scene ID。
) -> PlacementSceneDependencyState {
    let Some((direct, dependencies, recursive)) = asset_server.get_load_states(id) else {
        return PlacementSceneDependencyState::Waiting; // 尚未建立状态表时继续非阻塞等待。
    };
    if matches!(direct, LoadState::Failed(_))
        || matches!(dependencies, DependencyLoadState::Failed(_))
        || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
    {
        return PlacementSceneDependencyState::Failed; // 任一层失败都拒绝整份候选。
    }
    if asset_server.is_loaded_with_dependencies(id) {
        PlacementSceneDependencyState::Ready // 只信任Bevy对递归依赖的完整证明。
    } else {
        PlacementSceneDependencyState::Waiting // 返回主循环，不同步等待。
    }
}

/// 只处理本帧开始时队列中的固定前缀，Waiting ID同帧不会被反复弹出。
pub(super) fn poll_scene_loads(
    tracker: &mut PlacementSceneLoadTracker, // 去重队列与Ready集合由本函数原子更新。
    classify: &mut impl FnMut(AssetId<WorldAsset>) -> PlacementSceneDependencyState, // 生产查AssetServer，测试注入可计数闭包。
) -> PlacementSceneLoadAdvance {
    let checks_this_frame = tracker
        .scene_checks
        .len()
        .min(PLACEMENT_SCENE_LOAD_CHECK_BUDGET_PER_FRAME); // 冻结帧开始长度，防止少量Waiting ID在同帧循环。
    let mut advance = PlacementSceneLoadAdvance::default(); // 不将checked本身视为实际加载进度。
    for _ in 0..checks_this_frame {
        let id = tracker
            .scene_checks
            .pop_front()
            .expect("帧开始已冻结可弹出数量"); // 队列顺序在稳定运行中轮转。
        tracker.queued_scene_ids.remove(&id); // 先释放去重席位，Waiting分支再排入队尾。
        advance.checked += 1; // 每个ID只计一次AssetServer状态查询。
        match classify(id) {
            PlacementSceneDependencyState::Waiting => {
                queue_scene_check(tracker, id, false); // 同帧不再处理，下帧继续循环检查。
            }
            PlacementSceneDependencyState::Ready => {
                if tracker.ready_scene_ids.insert(id) {
                    advance.newly_ready += 1; // 重复Loaded事件或轮询不重复增加heartbeat。
                }
            }
            PlacementSceneDependencyState::Failed => {
                tracker.ready_scene_ids.remove(&id); // 失败不保留旧Ready信用。
                advance.failed = true; // 失败ID不自动重排，只由后续匹配资产事件唤醒。
                break; // 本帧已确定整份候选失败，不再消耗额外查询预算。
            }
        }
    }
    advance.all_ready = tracker.ready_scene_ids.len() == tracker.scene_ids.len(); // 空文档自然Ready，非空文档不遍历全表。
    advance // 调用者只在newly_ready或状态改变时推进公开heartbeat。
}
