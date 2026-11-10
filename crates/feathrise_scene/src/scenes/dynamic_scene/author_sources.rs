//! 兼容主对象沿用当前场景的未保存作者版本；其他场景实例和新地形仍严格读取自己的上下文。
use bevy::{ecs::{component::Mutable, system::SystemParam}, prelude::*};
use feathrise_scene_content::{ActiveDynamicScene, TerrainId};
use feathrise_scene_core::residency::{InstanceFrame, InstanceResourceTable};

/// 空场景容器只借用自己的空作者资源；它的清理路径不能消费当前地形的预览或保存信用。
#[derive(Resource)]
struct ContainerAuthor<T: Resource>(T);
impl<T: Resource + Default> Default for ContainerAuthor<T> { fn default() -> Self { Self(T::default()) } }
pub(super) fn install(app: &mut App) {
    app.init_resource::<ContainerAuthor<feathrise_scene_content::SparseHeightLiveOverride>>()
        .init_resource::<ContainerAuthor<feathrise_scene_content::ScenePuddleLiveOverride>>()
        .init_resource::<ContainerAuthor<feathrise_scene_content::ScenePuddleSaveHandoff>>()
        .init_resource::<ContainerAuthor<super::surface::DynamicSurfaceAuthorPreview>>()
        .init_resource::<ContainerAuthor<super::vegetation::DynamicBladeDraft>>()
        .init_resource::<ContainerAuthor<super::water::DynamicClipmapWaterDraft>>()
        .init_resource::<ContainerAuthor<super::terrain_topology::TerrainVolumePreviewRequest>>()
        .init_resource::<ContainerAuthor<super::volume_stream::TerrainVolumeStreamControl>>();
}

#[derive(SystemParam)]
struct AuthorScope<'w, 's> {
    frame: InstanceFrame<'w>,
    active: Res<'w, ActiveDynamicScene>,
    objects: Query<'w, 's, (&'static super::instances::TerrainObjectInstance, &'static super::instances::SubsceneInstance)>,
    registry: Option<Res<'w, feathrise_scene_content::SceneContentRegistry>>,
    manifests: Option<Res<'w, Assets<feathrise_scene_content::SceneManifestAsset>>>,
    target: Option<Res<'w, super::instances::TerrainObjectAuthoringTarget>>,
    preview: Option<Res<'w, feathrise_scene_content::SceneInstanceAuthorPreview>>,
}
impl AuthorScope<'_, '_> {
    fn is_container(&self) -> bool {
        self.frame.instance().is_none() && (self.preview.as_ref().is_some_and(|preview|
            preview.scene_id == self.active.scene_id && preview.terrains.is_some())
            || self.active.scene_id.as_ref().and_then(|id| self.registry.as_ref()?.handles(id))
                .and_then(|handles| self.manifests.as_ref()?.get(&handles.manifest)).is_some_and(|manifest| manifest.terrains.is_some()))
    }
    fn instance<T: Resource>(&self) -> Option<Entity> {
        let entity = self.frame.instance()?;
        let id = self.target.as_ref().and_then(|target| target.scene_id.as_ref().filter(|map| self.active.scene_id.as_ref() == Some(*map))
            .and(target.applied_id())).cloned().unwrap_or_else(TerrainId::legacy);
        let id = if std::any::TypeId::of::<T>() == std::any::TypeId::of::<feathrise_scene_content::SceneAuthorPreview>() { TerrainId::legacy() } else { id };
        if let Ok((object, node)) = self.objects.get(entity)
            && object.definition.id == id && node.path.instances.is_empty()
            && self.active.scene_id.as_ref() == Some(&node.source_scene_id) { None } else { Some(entity) }
    }
}

#[derive(SystemParam)]
pub(crate) struct TerrainAuthorRes<'w, 's, T: Resource> {
    root: Res<'w, T>, scope: AuthorScope<'w, 's>, table: Option<Res<'w, InstanceResourceTable<T>>>,
    container: Option<Res<'w, ContainerAuthor<T>>>,
}
impl<T: Resource> TerrainAuthorRes<'_, '_, T> { pub(crate) fn is_container(&self) -> bool { self.scope.is_container() } }
impl<T: Resource> std::ops::Deref for TerrainAuthorRes<'_, '_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        if self.scope.is_container() && let Some(container) = &self.container { return &container.0; }
        self.scope.instance::<T>().map_or(&self.root, |entity|
            self.table.as_ref().and_then(|table| table.values.get(&entity)).expect("地形作者上下文必须已初始化"))
    }
}

#[derive(SystemParam)]
pub(crate) struct TerrainAuthorResMut<'w, 's, T: Resource<Mutability = Mutable>> {
    root: ResMut<'w, T>, scope: AuthorScope<'w, 's>, table: Option<ResMut<'w, InstanceResourceTable<T>>>,
    container: Option<ResMut<'w, ContainerAuthor<T>>>,
}
impl<T: Resource<Mutability = Mutable>> TerrainAuthorResMut<'_, '_, T> { pub(crate) fn is_container(&self) -> bool { self.scope.is_container() } }
impl<T: Resource<Mutability = Mutable>> std::ops::Deref for TerrainAuthorResMut<'_, '_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        if self.scope.is_container() && let Some(container) = &self.container { return &container.0; }
        self.scope.instance::<T>().map_or(&self.root, |entity|
            self.table.as_ref().and_then(|table| table.values.get(&entity)).expect("地形作者上下文必须已初始化"))
    }
}
impl<T: Resource<Mutability = Mutable>> std::ops::DerefMut for TerrainAuthorResMut<'_, '_, T> {
    fn deref_mut(&mut self) -> &mut T {
        if self.scope.is_container() && let Some(container) = self.container.as_deref_mut() { return &mut container.0; }
        if let Some(entity) = self.scope.instance::<T>() {
            self.table.as_mut().and_then(|table| table.values.get_mut(&entity)).expect("地形作者上下文必须已初始化")
        } else { &mut self.root }
    }
}
