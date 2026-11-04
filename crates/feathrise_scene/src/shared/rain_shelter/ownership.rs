//! 显式代理只服从正式地图/placement采用门，隐藏候选与退休旧树不能遮雨。

use super::{resources::RainShelterRegistry, types::ShelterSource};
use crate::MapPlacementInstance; // 正式只读所有权和批次活动门。
use bevy::prelude::*; // 有界层级查询。
use feathrise_map_content::MapId; // placement固定地图身份。
use feathrise_scene_core::state::ScenePhase; // 静态地图状态。 // 来源及反向索引。

pub(super) type OwnerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static ChildOf>,
        Option<&'static MapPlacementInstance>,
        Option<&'static DespawnOnExit<ScenePhase>>,
    ),
>; // 单根查询不扫描世界。

pub(super) fn resolve_owner(entity: Entity, query: &OwnerQuery) -> Option<(Entity, Vec<Entity>)> {
    // 新源/层级变更才最多走64级。
    let mut ancestors = Vec::with_capacity(8); // 普通节点层级小，最高64个实体ID。
    let mut current = entity; // 从代理本身查找显式根。
    for _ in 0..64 {
        // 限制损坏或极深层级的最坏工作量。
        let (parent, placement, scoped) = query.get(current).ok()?; // 实体消失时不猜测所有权。
        ancestors.push(current); // 反向索引记录真实路径。
        if placement.is_some() || scoped.is_some() {
            return Some((current, ancestors));
        } // 首个正式拥有者定义生命周期。
        let Some(parent) = parent else {
            return Some((entity, ancestors));
        }; // 玩法直接插入的独立代理以自身为根。
        current = parent.parent(); // 只沿单条祖先链。
    } // 深度上限之外不继续主线程遍历。
    None // 过深层级拒绝，不默认为活动地图。
} // 完成根解析。

pub(super) fn remove_source(
    registry: &mut RainShelterRegistry,
    entity: Entity,
) -> Option<ShelterSource> {
    // 删除时同步维护两个索引。
    let source = registry.sources.remove(&entity)?; // 不存在时常量返回。
    if let Some(sources) = registry.owners.get_mut(&source.owner) {
        sources.remove(&entity);
        if sources.is_empty() {
            registry.owners.remove(&source.owner);
        }
    } // 删除空根，旧批次不占预算。
    for ancestor in &source.ancestors {
        // 最多64个键，无世界扫描。
        if let Some(sources) = registry.descendants.get_mut(ancestor) {
            sources.remove(&entity);
            if sources.is_empty() {
                registry.descendants.remove(ancestor);
            }
        } // 删除空反向索引。
    } // 完成有界路径清理。
    Some(source) // 身份可用于同实体重挂重建。
} // 完成定向移除。

pub(super) fn insert_source(
    registry: &mut RainShelterRegistry,
    entity: Entity,
    source: ShelterSource,
) {
    // 只登记完整解析结果。
    registry
        .owners
        .entry(source.owner)
        .or_default()
        .insert(entity); // 每根批次可一次检查活动门。
    for ancestor in &source.ancestors {
        registry
            .descendants
            .entry(*ancestor)
            .or_default()
            .insert(entity);
    } // 最多64个小索引项。
    registry.sources.insert(entity, source); // 原子保存完整来源。
} // 完成小索引插入。

pub(super) fn owner_active(
    owner: Entity,
    query: &OwnerQuery,
    context: &(ScenePhase, Option<MapId>),
) -> bool {
    // 稳定帧每个缓存根只做一次get。
    let Ok((_, placement, scoped)) = query.get(owner) else {
        return false;
    }; // 根卸载立即失效。
    if let Some(placement) = placement {
        // 正式采用门来自共享批次，不能只看Visibility。
        let map = match context.0 {
            ScenePhase::Active => context.1.as_ref().map(|id| id.as_str()),
            _ => None,
        }; // 静态和动态都用真实MapId比较。
        return placement.is_active() && map == Some(placement.map_id().as_str()); // Pending/Retired无论是否有实体都不遮雨。
    } // 完成placement活动门。
    if let Some(scoped) = scoped {
        return scoped.0 == context.0;
    } // 直接游戏场景根同样使用显式所有权。
    true // 没有正式根的直接ECS代理由源创建context继续校验。
} // 完成单根活动判定。
