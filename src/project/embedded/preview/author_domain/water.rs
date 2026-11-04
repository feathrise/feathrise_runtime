use bevy::{
    asset::RenderAssetUsages,
    ecs::system::SystemState,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_map_content::{MapId, MapWaterAsset, TerrainId};
use feathrise_scene::{
    DynamicClipmapWaterDraft, DynamicClipmapWaterDraftSnapshot, TerrainWaterDrafts,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
pub(super) struct Prepared {
    map: MapId,
    terrain: Option<TerrainId>,
    root: bool,
    revision: u64,
    definition: Option<MapWaterAsset>,
    sources: BTreeMap<String, Arc<[u8]>>,
    images: VecDeque<(String, Image)>,
    handles: BTreeMap<String, Handle<Image>>,
    previous: Option<Arc<DynamicClipmapWaterDraftSnapshot>>,
    previous_credit: Option<Arc<DynamicClipmapWaterDraftSnapshot>>,
}
pub(super) fn prepare(
    map: MapId,
    terrain: Option<TerrainId>,
    root: bool,
    revision: u64,
    definition: Option<MapWaterAsset>,
    sources: BTreeMap<String, Arc<[u8]>>,
) -> Result<Prepared, String> {
    if root && terrain.is_some() {
        return Err("根水预览不能携带地形身份".into());
    }
    let mut images = VecDeque::new();
    if let Some(definition) = &definition {
        definition.validate(&map)?;
        if sources.len() != definition.layers.len() {
            return Err("水预览遮罩与水层数量不一致".into());
        }
        for layer in &definition.layers {
            let source = sources.get(&layer.id).ok_or("水预览缺少完整R8遮罩")?;
            let count = (layer.resolution[0] as usize)
                .checked_mul(layer.resolution[1] as usize)
                .ok_or("水预览尺寸溢出")?;
            if source.len() != count {
                return Err("水预览R8字节与声明尺寸不一致".into());
            }
            let mut image = Image::new(
                Extent3d {
                    width: layer.resolution[0],
                    height: layer.resolution[1],
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                source.to_vec(),
                TextureFormat::R8Unorm,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            image.sampler = ImageSampler::linear();
            images.push_back((layer.id.clone(), image));
        }
    } else if !sources.is_empty() {
        return Err("撤回水预览不能携带游离遮罩".into());
    }
    Ok(Prepared {
        map,
        terrain,
        root,
        revision,
        definition,
        sources,
        images,
        handles: BTreeMap::new(),
        previous: None,
        previous_credit: None,
    })
}
#[derive(Default)]
pub(super) struct State {
    publisher: Option<SystemState<TerrainWaterDrafts<'static>>>,
    published: BTreeMap<(MapId, Option<TerrainId>, bool), Arc<DynamicClipmapWaterDraftSnapshot>>,
    retired: Option<Task<()>>,
    #[cfg(feature = "validation")]
    last: Option<(MapId, Option<TerrainId>, bool, u64)>,
}
impl State {
    pub(super) fn advance(
        &mut self,
        world: &mut World,
        value: &mut Prepared,
    ) -> Result<bool, String> {
        if let Some((id, image)) = value.images.pop_front() {
            value
                .handles
                .insert(id, world.resource_mut::<Assets<Image>>().add(image));
            return Ok(false);
        }
        let publisher = self
            .publisher
            .get_or_insert_with(|| SystemState::new(world));
        if let Some(id) = &value.terrain {
            if !publisher
                .get_mut(world)
                .map_err(|error| error.to_string())?
                .ready(&value.map, id)
            {
                return Ok(false);
            }
            value.previous = publisher
                .get_mut(world)
                .map_err(|error| error.to_string())?
                .shared(&value.map, id);
        }
        if value.terrain.is_none() {
            value.previous = world
                .get_resource::<DynamicClipmapWaterDraft>()
                .and_then(|draft| {
                    if value.root {
                        draft.shared_root_for_map(&value.map)
                    } else {
                        draft.shared_for_map(&value.map)
                    }
                });
        }
        let publishing = value.definition.is_some();
        if let Some(definition) = value.definition.take() {
            let snapshot = DynamicClipmapWaterDraftSnapshot {
                source_revision: value.revision,
                definition,
                sources: std::mem::take(&mut value.sources),
                masks: std::mem::take(&mut value.handles),
            };
            if let Some(id) = &value.terrain {
                publisher
                    .get_mut(world)
                    .map_err(|error| error.to_string())?
                    .publish(&value.map, id, snapshot)
                    .map_err(|_| "水预览目标身份变化")?;
            } else {
                let mut output = world.resource_mut::<DynamicClipmapWaterDraft>();
                if value.root {
                    output.publish_root(snapshot);
                } else {
                    output.publish(snapshot);
                }
            }
        } else if let Some(id) = &value.terrain {
            if let Some(owned) = self.published.get(&(value.map.clone(), value.terrain.clone(), value.root)) {
                publisher.get_mut(world).map_err(|error| error.to_string())?.clear_owned(&value.map, id, owned);
            }
        } else {
            let owned = self.published.get(&(value.map.clone(), None, value.root));
            if value.previous.as_ref().zip(owned).is_some_and(|(current, owned)| Arc::ptr_eq(current, owned)) {
                let mut output = world.resource_mut::<DynamicClipmapWaterDraft>();
                if value.root { output.clear_root(); } else { output.clear(); }
            }
        }
        publisher.apply(world);
        let published = if !publishing { None } else if let Some(id) = &value.terrain {
            publisher
                .get_mut(world)
                .map_err(|error| error.to_string())?
                .shared(&value.map, id)
        } else {
            world
                .get_resource::<DynamicClipmapWaterDraft>()
                .and_then(|draft| {
                    if value.root {
                        draft.shared_root_for_map(&value.map)
                    } else {
                        draft.shared_for_map(&value.map)
                    }
                })
        };
        let key = (value.map.clone(), value.terrain.clone(), value.root);
        value.previous_credit = if let Some(published) = published {
            self.published.insert(key, published)
        } else {
            self.published.remove(&key)
        };
        #[cfg(feature = "validation")]
        {
            self.last = Some((
                value.map.clone(),
                value.terrain.clone(),
                value.root,
                value.revision,
            ));
        }
        Ok(true)
    }
    pub(super) fn clear_published(&mut self, world: &mut World) -> Result<bool, String> {
        if let Some(task) = self.retired.as_mut() {
            if check_ready(task).is_none() {
                return Ok(false);
            }
            self.retired = None;
        }
        if self.published.is_empty() {
            return Ok(true);
        }
        let publisher = self
            .publisher
            .get_or_insert_with(|| SystemState::new(world));
        let mut old = Vec::new();
        for _ in 0..4 {
            let Some(((map, terrain, root), owned)) = self.published.pop_first() else {
                break;
            };
            let frozen = if let Some(id) = &terrain {
                publisher
                    .get_mut(world)
                    .map_err(|error| error.to_string())?
                    .shared(&map, id)
            } else {
                world
                    .get_resource::<DynamicClipmapWaterDraft>()
                    .and_then(|draft| {
                        if root {
                            draft.shared_root_for_map(&map)
                        } else {
                            draft.shared_for_map(&map)
                        }
                    })
            };
            if let Some(id) = &terrain {
                publisher.get_mut(world).map_err(|error| error.to_string())?.clear_owned(&map, id, &owned);
            } else if frozen
                .as_ref()
                .is_some_and(|snapshot| Arc::ptr_eq(snapshot, &owned))
            {
                if let Some(mut draft) = world.get_resource_mut::<DynamicClipmapWaterDraft>()
                {
                    if root {
                        draft.clear_root();
                    } else {
                        draft.clear();
                    }
                }
            }
            old.push((frozen, owned));
        }
        publisher.apply(world);
        self.retired = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(old);
        }));
        Ok(false)
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        self.last.as_ref().map_or(serde_json::Value::Null,|(map,terrain,root,revision)|{
            let draft=world.get_resource::<DynamicClipmapWaterDraft>();
            let published=draft.and_then(|draft|if *root{draft.root_for_map(map)}else{draft.for_map(map)});
            serde_json::json!({"map":map,"terrain":terrain,"root":root,"source_revision":revision,
                "root_resource_source_revision":published.map(|snapshot|snapshot.source_revision)})
        })
    }
}
