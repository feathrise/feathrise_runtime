//! 将原植被编译结果送入正式草稿发布器；大像素只在管道线程准备，每帧最多登记一张Image。

use bevy::{asset::RenderAssetUsages, ecs::system::SystemState, image::ImageSampler, prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat}};
use feathrise_scene_content::{SceneId, SceneVegetationKind, TerrainId,
    embedded_preview::{VEGETATION_PREVIEW_FORMAT, VegetationPreviewCanvas, decode_vegetation_preview}};
use feathrise_scene::{DynamicBladeDraft, DynamicBladeDraftSnapshot, TerrainVegetationDrafts};
use std::{collections::{BTreeMap, VecDeque}, sync::Arc};
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};

pub(super) struct Prepared {
    scene_id: SceneId,
    canvases: VecDeque<Canvas>,
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    fn snapshot(map: &SceneId) -> DynamicBladeDraftSnapshot {
        DynamicBladeDraftSnapshot { source_revision:7,
            definition:feathrise_scene_content::SceneVegetationAsset { schema_version:feathrise_scene_content::SCENE_SCHEMA_VERSION,scene_id:map.clone(),..default() },
            sources:BTreeMap::new(),densities:BTreeMap::new(),nonzero_layers:default() }
    }
    #[test]
    fn handback_releases_only_the_exact_published_source_even_when_revisions_match() {
        let mut app = App::new(); app.add_plugins(MinimalPlugins).init_resource::<DynamicBladeDraft>();
        let map = SceneId::parse("MAP-preview-ownership").unwrap();
        app.world_mut().resource_mut::<DynamicBladeDraft>().publish(snapshot(&map));
        let owned = app.world().resource::<DynamicBladeDraft>().frozen_for_scene(&map).unwrap();
        let mut state = State::default(); state.published.insert((map.clone(),None),owned.clone());
        app.world_mut().resource_mut::<DynamicBladeDraft>().publish(snapshot(&map));
        let external = app.world().resource::<DynamicBladeDraft>().frozen_for_scene(&map).unwrap();
        assert!(!Arc::ptr_eq(&owned,&external));
        assert!(!state.clear_published(app.world_mut()).unwrap());
        assert!(Arc::ptr_eq(&external,&app.world().resource::<DynamicBladeDraft>().frozen_for_scene(&map).unwrap()));
        let mut state = State::default(); state.published.insert((map.clone(),None),external);
        assert!(!state.clear_published(app.world_mut()).unwrap());
        assert!(app.world().resource::<DynamicBladeDraft>().for_scene(&map).is_none());
    }
}
struct Canvas {
    source: VegetationPreviewCanvas,
    images: VecDeque<(String, Image)>,
    handles: BTreeMap<String, Handle<Image>>,
    #[cfg(feature = "validation")]
    evidence: serde_json::Value,
}

pub(super) fn prepare(format: &str, bytes: &[u8], map: &SceneId) -> Result<Prepared, String> {
    if format != VEGETATION_PREVIEW_FORMAT { return Err("植被预览格式不兼容".into()); }
    let batch = decode_vegetation_preview(bytes)?;
    if &batch.scene_id != map { return Err("植被预览正文与控制场景不一致".into()); }
    let mut canvases = VecDeque::new();
    for source in batch.canvases {
        #[cfg(feature = "validation")]
        let evidence = {
            use sha2::{Digest, Sha256};
            let densities = source.densities.iter().map(|(id, values)| serde_json::json!({"id":id,
                "bytes":values.len(), "nonzero":values.iter().filter(|value| **value != 0).count(),
                "sha256":Sha256::digest(values).iter().map(|byte|format!("{byte:02x}")).collect::<String>()})).collect::<Vec<_>>();
            serde_json::json!({"terrain_id":source.terrain_id.as_ref().map(TerrainId::as_str),
                "source_revision":source.source_revision, "layout_revision":source.layout_revision, "densities":densities})
        };
        let mut images = VecDeque::new();
        for layer in &source.definition.layers {
            let mut image = Image::new(Extent3d { width: layer.resolution[0], height: layer.resolution[1],
                depth_or_array_layers: 1 }, TextureDimension::D2, source.densities[&layer.id].to_vec(),
                TextureFormat::R8Unorm, RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD);
            image.sampler = ImageSampler::nearest();
            images.push_back((layer.id.clone(), image));
        }
        canvases.push_back(Canvas { source, images, handles: BTreeMap::new(),
            #[cfg(feature = "validation")] evidence });
    }
    Ok(Prepared { scene_id: batch.scene_id, canvases })
}

#[derive(Default)]
pub(super) struct State {
    pending: Option<Prepared>,
    complete: Vec<(Option<TerrainId>, DynamicBladeDraftSnapshot)>,
    publisher: Option<SystemState<TerrainVegetationDrafts<'static>>>,
    published: BTreeMap<(SceneId, Option<TerrainId>), Arc<DynamicBladeDraftSnapshot>>,
    retired: Option<Task<()>>,
    #[cfg(feature = "validation")]
    evidence: Vec<serde_json::Value>,
}
impl State {
    pub(super) fn receive(&mut self, value: Prepared) {
        #[cfg(feature = "validation")]
        { self.evidence.clear(); }
        self.pending = Some(value);
    }
    pub(super) fn cancel(&mut self) {
        // 退役大图交后台；旧任务不再拥有发布权，但也不在帧线程析构完整像素。
        let pending = self.pending.take();
        let complete = std::mem::take(&mut self.complete);
        if pending.is_some() || !complete.is_empty() {
            bevy::tasks::AsyncComputeTaskPool::get().spawn(async move { drop((pending, complete)); }).detach();
        }
    }
    pub(super) fn clear_published(&mut self, world: &mut World) -> Result<bool, String> {
        if let Some(task) = self.retired.as_mut() {
            if check_ready(task).is_none() { return Ok(false); }
            self.retired = None;
        }
        if self.published.is_empty() { return Ok(true); }
        let publisher = self.publisher.get_or_insert_with(|| SystemState::new(world));
        let mut retired = Vec::new();
        for _ in 0..4 {
            let Some(((map, id), owned)) = self.published.pop_first() else { break; };
            let current = if let Some(id) = &id {
                publisher.get_mut(world).map_err(|error|format!("植被预览清理上下文不可用：{error}"))?.shared(&map, id)
            } else { world.get_resource::<DynamicBladeDraft>().and_then(|draft|draft.frozen_for_scene(&map)) };
            // 只有仍为本会话发布的确切Arc才清除；相同作者修订不能冒充外部系统的新来源。
            if current.as_ref().is_some_and(|value|Arc::ptr_eq(value, &owned)) {
                if let Some(id) = id {
                    publisher.get_mut(world).map_err(|error|format!("植被预览清理上下文不可用：{error}"))?.clear(&map, std::iter::once(id));
                } else if let Some(mut draft) = world.get_resource_mut::<DynamicBladeDraft>() { draft.clear(); }
            }
            retired.push((owned, current));
        }
        publisher.apply(world);
        self.retired = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired); }));
        Ok(false)
    }
    pub(super) fn advance(&mut self, world: &mut World) -> Result<bool, String> {
        if let Some(task) = self.retired.as_mut() {
            if check_ready(task).is_none() { return Ok(false); }
            self.retired = None;
        }
        let Some(pending) = self.pending.as_mut() else { return Ok(true); };
        if let Some(canvas) = pending.canvases.front_mut() {
            if let Some((id, image)) = canvas.images.pop_front() {
                canvas.handles.insert(id, world.resource_mut::<Assets<Image>>().add(image));
                return Ok(false);
            }
            let canvas = pending.canvases.pop_front().unwrap();
            #[cfg(feature = "validation")]
            self.evidence.push(canvas.evidence);
            self.complete.push((canvas.source.terrain_id, DynamicBladeDraftSnapshot {
                source_revision: canvas.source.source_revision,
                definition: canvas.source.definition,
                sources: canvas.source.densities,
                densities: canvas.handles,
                nonzero_layers: canvas.source.nonzero_layers,
            }));
            return Ok(false);
        }
        let publisher = self.publisher.get_or_insert_with(|| SystemState::new(world));
        let ready = publisher.get_mut(world).map_err(|error|format!("植被预览目标上下文不可用：{error}"))?.ready(&pending.scene_id,
            self.complete.iter().filter_map(|(id, _)| id.clone()));
        if !ready { return Ok(false); }
        // 全批资源存在才发布；根兼容来源与独立terrain来源在同一帧完成。
        let mut retired = Vec::new();
        let ids = self.complete.iter().map(|(id,_)|id.clone()).collect::<Vec<_>>();
        for id in &ids {
            let previous = if let Some(id) = id {
                publisher.get_mut(world).map_err(|error|format!("植被预览旧来源上下文不可用：{error}"))?.shared(&pending.scene_id,id)
            } else { world.get_resource::<DynamicBladeDraft>().and_then(|draft|draft.frozen_for_scene(&pending.scene_id)) };
            if let Some(previous) = previous { retired.push(previous); }
        }
        let mut terrain = Vec::new();
        for (id, snapshot) in std::mem::take(&mut self.complete) {
            if let Some(id) = id { terrain.push((id, snapshot)); }
            else {
                let mut output = world.resource_mut::<DynamicBladeDraft>();
                let density_only = output.for_scene(&pending.scene_id).is_some_and(|old|
                    old.definition.layers == snapshot.definition.layers
                    && snapshot.definition.layers.iter().all(|layer| layer.kind != SceneVegetationKind::Mesh));
                if density_only { output.publish_density(snapshot); } else { output.publish(snapshot); }
            }
        }
        if !terrain.is_empty() {
            publisher.get_mut(world).map_err(|error|format!("植被预览发布上下文不可用：{error}"))?.publish(&pending.scene_id, terrain)
                .map_err(|_| "植被预览发布期间目标地形身份发生变化")?;
        }
        publisher.apply(world);
        for id in ids {
            let adopted = if let Some(id) = &id {
                publisher.get_mut(world).map_err(|error|format!("植被预览采用上下文不可用：{error}"))?.shared(&pending.scene_id,id)
            } else { world.get_resource::<DynamicBladeDraft>().and_then(|draft|draft.frozen_for_scene(&pending.scene_id)) };
            let adopted = adopted.ok_or("植被发布没有产生当前来源")?;
            if let Some(old) = self.published.insert((pending.scene_id.clone(),id),adopted) { retired.push(old); }
        }
        self.pending = None;
        if retired.is_empty() { Ok(true) } else {
            self.retired = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired); }));
            Ok(false)
        }
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        use feathrise_scene::{SubsceneInstance, TerrainObjectInstance, TerrainObjectAuthoringTarget};
        use feathrise_scene_core::residency::InstanceResourceTable;
        let root = world.get_resource::<DynamicBladeDraft>();
        let map = self.published.keys().next().map(|(map,_)|map);
        let root_revision = map.and_then(|map|root.as_ref()?.revision_for_scene(map));
        let canvases = self.evidence.iter().cloned().map(|mut evidence| {
            let terrain = evidence["terrain_id"].as_str();
            let Some(map) = map else { return evidence; };
            let is_root = terrain.is_none() || world.get_resource::<TerrainObjectAuthoringTarget>().is_some_and(|target|
                terrain.and_then(|terrain|TerrainId::parse(terrain).ok()).is_some_and(|id|target.applied_to(map, &id)));
            let draft = if is_root { root } else {
                world.get_resource::<InstanceResourceTable<DynamicBladeDraft>>().and_then(|table|
                    table.values.iter().take(128).find_map(|(entity, draft)| {
                        let object = world.get::<TerrainObjectInstance>(*entity)?;
                        let instance = world.get::<SubsceneInstance>(*entity)?;
                        (Some(object.definition.id.as_str()) == terrain && &instance.source_scene_id == map
                            && instance.path.instances.is_empty()).then_some(draft)
                    }))
            };
            evidence["draft_revision"] = serde_json::json!(draft.and_then(|draft|draft.revision_for_scene(map)));
            evidence["adopted_source_revision"] = serde_json::json!(draft.and_then(|draft|draft.for_scene(map)).map(|snapshot|snapshot.source_revision));
            evidence
        }).collect::<Vec<_>>();
        serde_json::json!({"canvases":canvases, "root_draft_revision":root_revision})
    }
}
