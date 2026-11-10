//! 小型来源快照有界捕获，完整引用遍历与树内存分配只在单飞CPU任务中执行。

use super::{expand_scene_instances, SceneInstanceSource};
use super::resources::*;
use crate::{SceneContentRegistry, SceneManifestAsset};
use bevy::{asset::{AssetEvent, AssetLoadFailedEvent, LoadState}, prelude::*, tasks::{AsyncComputeTaskPool, futures_lite::future}};
use std::{ops::Bound::{Excluded, Unbounded}, sync::Arc};

const SOURCE_CAPTURE_BUDGET: usize = 2; // 首载、目录重载和批量热更都共用同一帧预算。

pub(super) fn drive_instance_graph(
    request: Res<SceneInstanceRootRequest>,
    preview: Res<SceneInstanceAuthorPreview>,
    registry: Option<Res<SceneContentRegistry>>,
    manifests: Res<Assets<SceneManifestAsset>>,
    server: Option<Res<AssetServer>>,
    mut events: MessageReader<AssetEvent<SceneManifestAsset>>,
    mut failures: MessageReader<AssetLoadFailedEvent<SceneManifestAsset>>,
    mut work: ResMut<SceneInstanceGraphWork>,
    mut graph: ResMut<SceneInstanceGraph>,
) {
    // 完成后必须归还原来源缓存，即使结果迟到也不能通过丢句柄释放并发槽。
    let mut completed = None;
    if let Some(task) = work.task.as_mut() {
        if let Some(done) = future::block_on(future::poll_once(task)) {
            work.task = None;
            work.sources = done.sources;
            work.pending = done.pending;
            completed = Some((done.identity, done.result, done.waiting, done.root_references));
        }
    }
    let Some(registry) = registry.as_deref() else { events.clear(); return; };
    if let Some(task) = work.releasing.as_mut() {
        if future::block_on(future::poll_once(task)).is_some() { work.releasing = None; }
    }
    if work.catalog_revision != Some(registry.revision) {
        work.catalog_revision = Some(registry.revision);
        work.cursor = None;
        work.scanning = true;
        work.pruning = true;
        work.prune_cursor = None;
        work.revision = work.revision.wrapping_add(1);
    }
    for event in events.read().take(128) {
        let id = match *event {
            AssetEvent::Added { id } | AssetEvent::Modified { id }
            | AssetEvent::Removed { id } | AssetEvent::Unused { id }
            | AssetEvent::LoadedWithDependencies { id } => id,
        };
        if let Some(scene_id) = work.handles.get(&id).or_else(|| manifests.get(id).map(|m| &m.id))
            && registry.handles(scene_id).is_some_and(|handles| handles.manifest.id() == id) {
            // 地形/容器投影沿用SceneId但有私有资产句柄；只接受目录权威源，避免重建→投影→重建反馈。
            let scene_id = scene_id.clone();
            work.queued.insert(scene_id);
        }
    }
    for event in failures.read().take(128) {
        if let Some(id) = work.handles.get(&event.id)
            && registry.handles(id).is_some_and(|handles| handles.manifest.id() == event.id) {
            let id = id.clone();
            work.queued.insert(id);
        }
    }
    if !failures.is_empty() { failures.clear(); work.scanning = true; work.cursor = None; }
    if !events.is_empty() {
        // 批量清单通知溢出时重扫目录；不在一帧遍历全部资产，也不忽略来源更新。
        events.clear();
        work.scanning = true;
        work.cursor = None;
        work.revision = work.revision.wrapping_add(1);
    }
    let mut captured = 0;
    while work.pruning && captured < SOURCE_CAPTURE_BUDGET {
        let next = if let Some(cursor) = &work.prune_cursor {
            work.known_sources.range((Excluded(cursor), Unbounded)).next().cloned()
        } else { work.known_sources.first().cloned() };
        let Some(id) = next else { work.pruning = false; break; };
        work.prune_cursor = Some(id.clone());
        captured += 1;
        if !registry.scenes.contains_key(&id) {
            work.known_sources.remove(&id);
            work.updates.insert(id, SourceUpdate::Unavailable);
            work.revision = work.revision.wrapping_add(1);
        }
    }
    while captured < SOURCE_CAPTURE_BUDGET {
        let scene_id = if let Some(id) = work.queued.pop_first() { Some(id) }
        else if work.scanning {
            let next = if let Some(cursor) = &work.cursor {
                registry.scenes.range((Excluded(cursor), Unbounded)).next().map(|(id, _)| id.clone())
            } else { registry.scenes.keys().next().cloned() };
            work.cursor = next.clone();
            work.scanning = next.is_some();
            next
        } else { None };
        let Some(scene_id) = scene_id else { break; };
        captured += 1;
        let Some(handles) = registry.handles(&scene_id) else {
            work.updates.insert(scene_id, SourceUpdate::Unavailable);
            work.revision = work.revision.wrapping_add(1);
            continue;
        };
        work.handles.insert(handles.manifest.id(), scene_id.clone());
        work.known_sources.insert(scene_id.clone());
        if let Some(manifest) = manifests.get(&handles.manifest) {
            if manifest.is_data_driven() && let Some(bounds) = manifest.world_bounds() {
                let snapshot = Arc::new(SceneInstanceSource { terrains: manifest.terrains.clone(), scene_id: manifest.id.clone(), revision: work.revision.wrapping_add(1), world_bounds: bounds, instances: manifest.subscenes.clone() });
                work.updates.insert(scene_id, SourceUpdate::Ready(snapshot));
                work.revision = work.revision.wrapping_add(1);
            } else {
                work.updates.insert(scene_id, SourceUpdate::Unavailable);
                work.revision = work.revision.wrapping_add(1);
            }
        } else {
            let failed = server.as_ref().is_some_and(|server| matches!(server.get_load_state(handles.manifest.id()), Some(LoadState::Failed(_))));
            work.updates.insert(scene_id, if failed { SourceUpdate::Unavailable } else { SourceUpdate::Pending });
            work.revision = work.revision.wrapping_add(1);
        } // 尚未加载的清单由Added/Loaded事件唤醒，稳定帧不重复申请IO。
    }
    let identity = GraphIdentity { root: request.scene_id.clone(), revision: work.revision, author_revision: preview.revision };
    if work.desired.as_ref().is_some_and(|old| old.root != identity.root) {
        graph.phase = SceneInstanceGraphPhase::Waiting; graph.last_error = None;
    }
    work.desired = Some(identity.clone());
    if let Some((completed_identity, result, waiting, root_references)) = completed {
        // 先处理本帧新通知，再核对完整身份，防止旧树在同帧热更时短暂采用。
        let current_references = identity.root.as_ref().and_then(|root|
            if preview.scene_id.as_ref() == Some(root) { Some(&preview.instances) }
            else { work.sources.get(root).map(|source| &source.instances) });
        let pose_only = completed_identity.root == identity.root && completed_identity.revision == identity.revision
            && current_references.is_some_and(|current| same_topology(&root_references, current));
        // 来源与引用拓扑完整匹配时允许上一帧姿态；否则每帧移动鼠标会永久取消真实内容预览。
        if (completed_identity == identity || pose_only) && !work.scanning && !work.pruning && work.queued.is_empty() {
            if waiting { graph.phase = SceneInstanceGraphPhase::Waiting; graph.last_error = None; }
            else { match result {
                Ok(instances) => {
                    if graph.root_scene_id != completed_identity.root || work.published_sources_revision != completed_identity.revision
                        || !same_topology(&work.published_references, &root_references) {
                        graph.structural_revision = graph.structural_revision.wrapping_add(1);
                    }
                    work.published_sources_revision = completed_identity.revision;
                    work.published_references = root_references;
                    let retired = std::mem::replace(&mut graph.instances, instances);
                    work.releasing = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired); }));
                    graph.root_scene_id = completed_identity.root;
                    graph.revision = graph.revision.wrapping_add(1);
                    graph.phase = SceneInstanceGraphPhase::Ready;
                    graph.last_error = None;
                }
                Err(error) => {
                    graph.phase = SceneInstanceGraphPhase::Failed;
                    graph.last_error = Some(error);
                }
            } }
        } else {
            work.started = None;
            work.releasing = Some(AsyncComputeTaskPool::get().spawn(async move { drop(result); }));
        }
    }
    if work.task.is_some() || work.releasing.is_some() || work.scanning || work.pruning || !work.queued.is_empty()
        || work.started.as_ref() == Some(&identity) { return; }
    let mut sources = std::mem::take(&mut work.sources); // 只移动整个缓存，不在任务启动帧深拷贝全部引用图。
    let updates = std::mem::take(&mut work.updates);
    let mut pending = std::mem::take(&mut work.pending);
    let author = preview.scene_id.clone().map(|id| (id, preview.instances.clone(), preview.terrains.clone()));
    work.started = Some(identity.clone());
    if graph.root_scene_id != identity.root || graph.instances.is_empty() {
        graph.phase = SceneInstanceGraphPhase::Building;
    } // 同根完整树保持可消费；后台更新不能关闭已经发布的姿态预览。
    work.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        for (id, update) in updates {
            match update {
                SourceUpdate::Ready(source) => { pending.remove(&id); sources.insert(id, source); }
                SourceUpdate::Pending => { sources.remove(&id); pending.insert(id); }
                SourceUpdate::Unavailable => { sources.remove(&id); pending.remove(&id); }
            }
        }
        // 作者预览只覆盖后台局部树输入，不改写保存来源缓存。
        let original = author.as_ref().and_then(|(id, _, _)| sources.get(id).cloned());
        if let Some((id, instances, terrains)) = author.filter(|(id, _, _)| identity.root.as_ref() == Some(id)) {
            if let Some(source) = sources.get(&id) {
                let replacement = Arc::new(SceneInstanceSource { terrains, scene_id: id.clone(), revision: source.revision, world_bounds: source.world_bounds, instances });
                sources.insert(id, replacement);
            }
        }
        let waiting = identity.root.as_ref().is_some_and(|root| reachable_pending(root, &sources, &pending));
        let result = if waiting { Ok(Arc::from([])) }
            else if let Some(root) = &identity.root { expand_scene_instances(root, &sources).map(Arc::from) } else { Ok(Arc::from([])) };
        let root_references = identity.root.as_ref().and_then(|id| sources.get(id))
            .map_or_else(|| Arc::from([]), |source| source.instances.clone());
        if let Some(original) = original { sources.insert(original.scene_id.clone(), original); }
        GraphCompletion { identity, sources, result, pending, waiting, root_references }
    }));
}

fn same_topology(before: &[super::SceneInstancePlacement], after: &[super::SceneInstancePlacement]) -> bool {
    before.len() == after.len() && before.iter().zip(after).all(|(old, new)|
        old.id == new.id && old.source_scene_id == new.source_scene_id)
} // 仅父图最多256条直接引用，严格比较身份；变换与显示标签不参与拓扑世代。

fn reachable_pending(root: &crate::SceneId,
    sources: &std::collections::BTreeMap<crate::SceneId, Arc<SceneInstanceSource>>,
    pending: &std::collections::BTreeSet<crate::SceneId>) -> bool {
    let mut visited = std::collections::BTreeSet::new();
    let mut todo = vec![root];
    while let Some(id) = todo.pop() {
        if !visited.insert(id) { continue; }
        if pending.contains(id) { return true; }
        if let Some(source) = sources.get(id) { todo.extend(source.instances.iter().map(|instance| &instance.source_scene_id)); }
    }
    false
}
