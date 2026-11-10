//! 运行时来源视图与正式目录分离；场景容器不再加载属于兼容主地形的画布、植被和水域。
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_content::{ActiveDynamicScene, SceneContentRegistry, SceneId, SceneInstanceAuthorPreview, SceneManifestAsset, SceneTerrainDescriptor};
use feathrise_scene_core::residency::{InstanceRes, SceneResidencyAppExt};

#[derive(Resource, Default)]
struct ContainerSource {
    owner: Option<SceneId>,
    registry: Option<SceneContentRegistry>,
    manifest: Option<Handle<SceneManifestAsset>>,
    stamp: Option<(u64, u64)>,
}

#[derive(SystemParam)]
pub struct SceneRuntimeCatalog<'w> {
    registry: InstanceRes<'w, SceneContentRegistry>,
    scope: feathrise_scene_core::residency::InstanceFrame<'w>,
    active: Res<'w, ActiveDynamicScene>,
    container: Option<Res<'w, ContainerSource>>,
}
impl std::ops::Deref for SceneRuntimeCatalog<'_> {
    type Target = SceneContentRegistry;
    fn deref(&self) -> &Self::Target {
        if self.scope.instance().is_none() && let Some(container) = &self.container
            && container.owner == self.active.scene_id && let Some(registry) = &container.registry {
            registry
        } else { &self.registry }
    }
}
pub(super) fn install(app: &mut App) {
    app.init_resident_resource::<ContainerSource>()
        .add_resident_systems(PreUpdate, prepare);
}

fn prepare(active: Res<ActiveDynamicScene>, registry: Res<SceneContentRegistry>,
    preview: Res<SceneInstanceAuthorPreview>, mut state: ResMut<ContainerSource>,
    mut manifests: ResMut<Assets<SceneManifestAsset>>, mut events: MessageReader<AssetEvent<SceneManifestAsset>>) {
    let handle = active.scene_id.as_ref().and_then(|id| registry.handles(id)).map(|handles| handles.manifest.id());
    let changed = events.read().any(|event| match event {
        AssetEvent::Modified { id } | AssetEvent::Added { id } | AssetEvent::Removed { id } => Some(*id) == handle,
        _ => false,
    });
    let author = preview.scene_id == active.scene_id && preview.terrains.is_some();
    let stamp = (registry.revision, if author { preview.revision } else { 0 });
    if state.owner == active.scene_id && state.stamp == Some(stamp) && !changed { return; }
    state.owner.clone_from(&active.scene_id); state.stamp = Some(stamp);
    state.registry = None;
    let Some(id) = &active.scene_id else { return; };
    let Some(handles) = registry.handles(id) else { return; };
    let Some(owner) = manifests.get(&handles.manifest) else { state.stamp = None; return; };
    if owner.terrains.is_none() && !author { return; }
    let mut container = owner.clone();
    let independent_water = owner.terrain.is_empty(); // 原兼容主地形的水仍归私有对象，真正空根的水属于场景自身。
    container.terrain = SceneTerrainDescriptor::empty();
    if author { container.terrains.clone_from(&preview.terrains); }
    container.puddles = None; // 兼容主地形自己的潜力图随对象源投影，容器不再次读取。
    let projected = if let Some(handle) = &state.manifest {
        manifests.insert(handle.id(), container).expect("容器清单句柄必须存在"); handle.clone()
    } else { let handle = manifests.add(container); state.manifest = Some(handle.clone()); handle };
    let mut handles = handles.clone();
    handles.manifest = projected;
    handles.surface = None; handles.vegetation = None;
    if !independent_water { handles.water = None; }
    state.registry = Some(SceneContentRegistry { catalog: registry.catalog.clone(), revision: registry.revision,
        scenes: std::collections::BTreeMap::from([(id.clone(), handles)]), last_error: None });
}
