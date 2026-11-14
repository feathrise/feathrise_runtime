//! 内置网格分块LOD的单飞后台组合、来源复验和完整换柄；不创建额外场景或碰撞实体。
use super::{components::*, ownership::owner, resources::*, types::*};
use bevy::{asset::VisitAssetDependencies, prelude::*, tasks::{AsyncComputeTaskPool, futures::check_ready}};
use feathrise_scene_content::MeshDisplayLodSettings;

pub(super) fn release(runtime: &mut MeshLodRuntime, target: &mut VisualTarget, entity: Entity) {
    if let Some(selection) = target.chunk_selection.take() {
        runtime.retired.push_back((selection.mesh, selection.bytes));
    }
    target.chunk_desired.clear();
    target.chunk_retry_credit = None;
    runtime.chunk_coarse.remove(&entity);
}

pub(super) fn is_current(target: &VisualTarget, mesh: &Handle<Mesh>) -> bool {
    target.chunk_selection.as_ref().is_some_and(|selection| selection.mesh == *mesh)
}

pub(super) fn build(source: Mesh) -> Result<Built, String> {
    // 只有内置作者网格持有这项私有布局声明；普通GLB仍完整保留所有UV与自定义属性。
    if source.contains_attribute(crate::ATTRIBUTE_AUTHORING_WIRE) {
        feathrise_terrain::build_editable_mesh_chunk_lods(source)
            .map(|chunks| Built::Chunks(std::sync::Arc::new(chunks)))
    } else {
        feathrise_terrain::build_static_mesh_lods(source).map(Built::Whole)
    }
}

/// 每个目标至多2个块、每块5档；选择只看缓存元数据，组合复制留在唯一worker。
pub(super) fn select(world: &mut World, runtime: &mut MeshLodRuntime, entity: Entity,
    target: &mut VisualTarget, views: &[View], policy: Option<MeshDisplayLodSettings>) {
    let Some(policy) = policy.filter(|_| plain_builtin_material(world, entity)) else {
        restore(world, runtime, entity, target);
        return;
    };
    let Some(chunks) = runtime.cache.get(&target.key).and_then(|cache| cache.chunks.clone()) else { return; };
    let Some(pose) = world.get::<GlobalTransform>(entity) else {
        restore(world, runtime, entity, target); return;
    };
    let layers = world.get::<bevy::camera::visibility::RenderLayers>(entity).cloned().unwrap_or_default();
    let previous = target.chunk_selection.as_ref().map_or(&[][..], |value| value.levels.as_slice());
    let selection = chunks.select(views, pose, &layers, policy.max_error_pixels, policy.hysteresis_ratio, previous);
    if selection.iter().all(|&level| level == 0) {
        restore(world, runtime, entity, target); return;
    }
    target.chunk_desired = selection.clone();
    if target.chunk_selection.as_ref().is_some_and(|value| value.levels == selection)
        || runtime.composition.is_some() || runtime.release.is_some()
        || target.chunk_retry_credit == Some(runtime.credit_revision) { return; }
    let ticket = CompositionTicket { entity, key: target.key, selection };
    let choices = ticket.selection.clone();
    runtime.cache.get_mut(&target.key).unwrap().pins += 1; // worker持有的共享数组也占缓存信用，不能提前退役归还。
    runtime.composition = Some((ticket, AsyncComputeTaskPool::get().spawn(async move {
        chunks.compose(&choices)
    })));
}

fn restore(world: &mut World, runtime: &mut MeshLodRuntime, entity: Entity, target: &mut VisualTarget) {
    if target.chunk_selection.is_some() {
        if let Some(cache) = runtime.cache.get_mut(&target.key) { cache.pins = cache.pins.saturating_sub(1); }
        world.entity_mut(entity).insert(Mesh3d(target.original.clone())).remove::<StaticMeshChunkLodStatus>();
        target.applied = 0;
        runtime.credit_revision = runtime.credit_revision.wrapping_add(1);
    }
    release(runtime, target, entity);
}

/// 内置材质只有颜色与粗糙度。外部覆盖为带纹理材质时保原，不能让省略作者UV的派生改变外观。
fn plain_builtin_material(world: &World, entity: Entity) -> bool {
    let Some(material) = world.get::<MeshMaterial3d<StandardMaterial>>(entity)
        .and_then(|handle| world.get_resource::<Assets<StandardMaterial>>()?.get(&handle.0)) else { return false; };
    // 使用材质正式依赖契约覆盖可选高光等纹理扩展，避免维护一份容易漏项的贴图字段名单。
    let mut textured = false;
    material.visit_dependencies(&mut |_| textured = true);
    !textured
}

/// 只在全部选择完成后接收候选；相机、对象、策略或作者工具换代均重新核对，不采用迟到组合。
pub(super) fn poll(world: &mut World, runtime: &mut MeshLodRuntime, views: &[View],
    global: Option<MeshDisplayLodSettings>, authoring: bool, preview: bool) {
    if runtime.release.is_some() { return; }
    let Some((_, task)) = runtime.composition.as_mut() else { return; };
    let Some(result) = check_ready(task) else { return; };
    let (ticket, _) = runtime.composition.take().unwrap();
    if let Some(cache) = runtime.cache.get_mut(&ticket.key) { cache.pins = cache.pins.saturating_sub(1); }
    let valid = runtime.current(ticket.key) && (!authoring || preview)
        && runtime.targets.get(&ticket.entity).is_some_and(|target|
            target.key == ticket.key && target.chunk_desired == ticket.selection)
        && owner(world, ticket.entity).is_some_and(|owner| owner.active && owner.policy.resolve_display(global).is_some())
        && plain_builtin_material(world, ticket.entity);
    let valid = valid && runtime.targets.get(&ticket.entity).is_some_and(|target| {
        // 层级发现按预算轮转，实体可能已经换父级而旧target尚未重登记；同一Mesh也不能跨所有者接收。
        let Some(policy) = owner(world, ticket.entity).filter(|owner| owner.root == target.owner)
            .and_then(|owner| owner.policy.resolve_display(global)) else { return false; };
        let Some(pose) = world.get::<GlobalTransform>(ticket.entity) else { return false; };
        let Some(chunks) = runtime.cache.get(&ticket.key).and_then(|cache| cache.chunks.as_ref()) else { return false; };
        let layers = world.get::<bevy::camera::visibility::RenderLayers>(ticket.entity).cloned().unwrap_or_default();
        let previous = target.chunk_selection.as_ref().map_or(&[][..], |value| value.levels.as_slice());
        chunks.select(views, pose, &layers, policy.max_error_pixels, policy.hysteresis_ratio, previous) == ticket.selection
            && world.get::<Mesh3d>(ticket.entity).is_some_and(|current|
                current.0 == target.original || is_current(target, &current.0))
    });
    if !valid {
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move { drop(result); }));
        return;
    }
    let mesh = match result {
        Ok(mesh) => mesh,
        Err(error) => {
            world.resource_mut::<MeshLodRuntimeStatus>().detail = error;
            return;
        }
    };
    let bytes = super::capture::mesh_bytes(&mesh);
    if bytes > UPLOAD_BUDGET || runtime.cache_bytes.saturating_add(bytes) > CACHE_BYTES {
        runtime.targets.get_mut(&ticket.entity).unwrap().chunk_retry_credit = Some(runtime.credit_revision);
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move { drop(mesh); }));
        world.resource_mut::<MeshLodRuntimeStatus>().detail = "分块LOD等待缓存信用，保留上一完整显示".into();
        return;
    }
    let triangles = mesh.indices().map_or(0, |indices| indices.len() / 3);
    let handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let target = runtime.targets.get_mut(&ticket.entity).unwrap();
    if target.chunk_selection.is_none() { runtime.cache.get_mut(&ticket.key).unwrap().pins += 1; }
    if let Some(old) = target.chunk_selection.replace(ChunkSelection {
        levels: ticket.selection.clone(), mesh: handle.clone(), bytes,
    }) { runtime.retired.push_back((old.mesh, old.bytes)); }
    target.applied = 1;
    runtime.cache_bytes += bytes;
    runtime.chunk_coarse.insert(ticket.entity);
    world.entity_mut(ticket.entity).insert((Mesh3d(handle), StaticMeshChunkLodStatus { levels: ticket.selection, triangles }));
}
