//! 跨地图传送落点使用来源姿态索引，单飞后台构造，主线程只做稳定ID查找。
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_map_content::{
    MapId, MapManifestAsset, MapTerrainBinding, MapTerrainObject, TerrainBindingIndex,
    TerrainObjectTransform,
};
use std::{collections::BTreeMap, sync::Arc};
struct Entry {
    objects: Arc<[MapTerrainObject]>,
    index: Arc<TerrainBindingIndex>,
}
struct Work {
    map: MapId,
    objects: Arc<[MapTerrainObject]>,
    task: Task<Arc<TerrainBindingIndex>>,
}
#[derive(Resource, Default)]
pub(crate) struct PortalBindingResolver {
    entries: BTreeMap<MapId, Entry>,
    work: Option<Work>,
}
impl PortalBindingResolver {
    pub(super) fn poll(&mut self) {
        if let Some(index) = self
            .work
            .as_mut()
            .and_then(|work| check_ready(&mut work.task))
        {
            let work = self.work.take().unwrap();
            let retired = self.entries.insert(
                work.map,
                Entry {
                    objects: work.objects,
                    index,
                },
            );
            if let Some(retired) = retired {
                AsyncComputeTaskPool::get()
                    .spawn(async move { drop(retired) })
                    .detach();
            }
        }
    }
    pub(super) fn resolve(
        &mut self,
        manifest: &MapManifestAsset,
        binding: &MapTerrainBinding,
    ) -> Result<TerrainObjectTransform, String> {
        let Some(objects) = manifest.terrains.as_ref() else {
            if binding.terrain_id == feathrise_map_content::TerrainId::legacy()
                && !manifest.terrain.is_empty()
            {
                return binding.resolve(Default::default());
            }
            return Err("传送落点的关联地形已不存在".into());
        };
        if let Some(entry) = self
            .entries
            .get(&manifest.id)
            .filter(|entry| Arc::ptr_eq(&entry.objects, objects))
        {
            return entry.index.resolve(binding);
        }
        if self.work.is_none() {
            let input = objects.clone();
            self.work = Some(Work {
                map: manifest.id.clone(),
                objects: objects.clone(),
                task: AsyncComputeTaskPool::get()
                    .spawn(async move { Arc::new(TerrainBindingIndex::from_objects(&input)) }),
            });
        }
        Err("传送落点关联地形索引仍在加载".into())
    }
}
