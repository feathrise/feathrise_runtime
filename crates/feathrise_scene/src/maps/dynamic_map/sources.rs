//! 运行时来源视图与正式目录分离；地图容器不再加载属于兼容主地形的画布、植被和水域。
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_map_content::{ActiveDynamicMap, MapContentRegistry, MapId, MapInstanceAuthorPreview, MapManifestAsset, MapTerrainDescriptor};
use feathrise_scene_core::residency::{InstanceRes, SceneResidencyAppExt};

#[derive(Resource, Default)]
struct ContainerSource {
    owner: Option<MapId>,
    registry: Option<MapContentRegistry>,
    manifest: Option<Handle<MapManifestAsset>>,
    stamp: Option<(u64, u64)>,
}

#[derive(SystemParam)]
pub struct MapRuntimeCatalog<'w> {
    registry: InstanceRes<'w, MapContentRegistry>,
    scope: feathrise_scene_core::residency::InstanceFrame<'w>,
    active: Res<'w, ActiveDynamicMap>,
    container: Option<Res<'w, ContainerSource>>,
}
impl std::ops::Deref for MapRuntimeCatalog<'_> {
    type Target = MapContentRegistry;
    fn deref(&self) -> &Self::Target {
        if self.scope.instance().is_none() && let Some(container) = &self.container
            && container.owner == self.active.map_id && let Some(registry) = &container.registry {
            registry
        } else { &self.registry }
    }
}
pub(super) fn install(app: &mut App) {
    app.init_resident_resource::<ContainerSource>()
        .add_resident_systems(PreUpdate, prepare);
}

fn prepare(active: Res<ActiveDynamicMap>, registry: Res<MapContentRegistry>,
    preview: Res<MapInstanceAuthorPreview>, mut state: ResMut<ContainerSource>,
    mut manifests: ResMut<Assets<MapManifestAsset>>, mut events: MessageReader<AssetEvent<MapManifestAsset>>) {
    let handle = active.map_id.as_ref().and_then(|id| registry.handles(id)).map(|handles| handles.manifest.id());
    let changed = events.read().any(|event| match event {
        AssetEvent::Modified { id } | AssetEvent::Added { id } | AssetEvent::Removed { id } => Some(*id) == handle,
        _ => false,
    });
    let author = preview.map_id == active.map_id && preview.terrains.is_some();
    let stamp = (registry.revision, if author { preview.revision } else { 0 });
    if state.owner == active.map_id && state.stamp == Some(stamp) && !changed { return; }
    state.owner.clone_from(&active.map_id); state.stamp = Some(stamp);
    state.registry = None;
    let Some(id) = &active.map_id else { return; };
    let Some(handles) = registry.handles(id) else { return; };
    let Some(owner) = manifests.get(&handles.manifest) else { state.stamp = None; return; };
    if owner.terrains.is_none() && !author { return; }
    let mut container = owner.clone();
    let independent_water = owner.terrain.is_empty(); // 原兼容主地形的水仍归私有对象，真正空根的水属于地图自身。
    container.terrain = MapTerrainDescriptor::empty();
    if author { container.terrains.clone_from(&preview.terrains); }
    container.puddles = None; // 兼容主地形自己的潜力图随对象源投影，容器不再次读取。
    let projected = if let Some(handle) = &state.manifest {
        manifests.insert(handle.id(), container).expect("容器清单句柄必须存在"); handle.clone()
    } else { let handle = manifests.add(container); state.manifest = Some(handle.clone()); handle };
    let mut handles = handles.clone();
    handles.manifest = projected;
    handles.surface = None; handles.vegetation = None;
    if !independent_water { handles.water = None; }
    state.registry = Some(MapContentRegistry { catalog: registry.catalog.clone(), revision: registry.revision,
        maps: std::collections::BTreeMap::from([(id.clone(), handles)]), last_error: None });
}
