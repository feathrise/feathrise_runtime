//! 编辑器根作者资源只覆盖当前对象；其他地形严格使用自己的实例资源，防止同SceneId串画布。
use bevy::prelude::*;
use feathrise_scene_content::{SceneId, TerrainId};

#[cfg(test)]
mod inheritance_tests;
#[cfg(test)]
mod height_source_tests;
#[derive(bevy::ecs::system::SystemParam)]
pub struct TerrainObjectStitchStatus<'w,'s> {
    runtime:Res<'w,super::resources::SubsceneRuntime>,
    status:Query<'w,'s,&'static super::super::clipmap::stitches::StitchStatus>,
}
impl TerrainObjectStitchStatus<'_, '_> {
    pub fn for_object(&self,map:&SceneId,id:&TerrainId)->Option<(u8,u8)> {
        let entity=self.runtime.active_entity(&feathrise_scene_content::SceneInstancePath::root(map.clone()).terrain(id.clone()))?;
        self.status.get(entity).ok().map(|s|(s.assigned,s.active))
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerrainAuthoringApply;
/// 未保存三维作者根独立于正式批次生命；采用新批次时必须先转交它的稳定源路径。
#[derive(Component)]
pub struct TerrainVolumeAuthorPreviewRoot(pub feathrise_scene_content::SceneInstancePath);

pub(super) fn transfer_volume_previews(
    world: &mut World,
    index: &std::collections::BTreeMap<feathrise_scene_content::SceneInstancePath, Entity>,
) {
    let mut query = world.query::<(Entity, &TerrainVolumeAuthorPreviewRoot, Option<&ChildOf>)>();
    let pending: Vec<_> = query
        .iter(world)
        .filter_map(|(entity, source, parent)| {
            let next = index.get(&source.0)?;
            parent
                .is_none_or(|parent| parent.parent() != *next)
                .then_some((entity, *next))
        })
        .collect(); // 作者场景最多32区，加上单飞旧/新候选；不扫描正式场景的全部实体。
    for (entity, parent) in pending {
        world.entity_mut(entity).insert(ChildOf(parent));
    }
}
#[derive(Resource, Default)]
pub struct TerrainObjectAuthoringTarget {
    pub scene_id: Option<SceneId>,
    pub terrain: Option<TerrainId>,
    applied: Option<(SceneId, TerrainId, Entity)>,
}
impl TerrainObjectAuthoringTarget {
    #[cfg(test)]
    pub(in crate::scenes::dynamic_scene) fn test_applied(map:SceneId,terrain:TerrainId,entity:Entity)->Self {
        Self{scene_id:Some(map.clone()),terrain:Some(terrain.clone()),applied:Some((map,terrain,entity))}
    }
    pub fn instance(&self) -> Option<Entity> {
        self.applied
            .as_ref()
            .and_then(|(_, _, entity)| (*entity != Entity::PLACEHOLDER).then_some(*entity))
    }
    pub(crate) fn applied_id(&self) -> Option<&TerrainId> {
        self.applied.as_ref().map(|(_, id, _)| id)
    }
    pub fn applied_to(&self,map:&SceneId,id:&TerrainId)->bool {
        self.applied.as_ref().is_some_and(|(owner,terrain,_)|owner==map && terrain==id)
    }
}

/// 活动实例整体换代后，未保存洞穴预览转交新根，不能随旧批次退役而消失。
pub(super) fn rebind_volume_roots(
    mut commands: Commands,
    runtime: Res<super::resources::SubsceneRuntime>,
    target: Res<TerrainObjectAuthoringTarget>,
    root: Res<super::super::terrain_topology::TerrainVolumePreviewRequest>,
    table: Res<
        feathrise_scene_core::residency::InstanceResourceTable<
            super::super::terrain_topology::TerrainVolumePreviewRequest,
        >,
    >,
    parents: Query<Option<&ChildOf>>,
    mut cursor: Local<Option<feathrise_scene_content::SceneInstancePath>>,
) {
    use std::ops::Bound::{Excluded, Unbounded};
    let Some(batch) = &runtime.active else {
        *cursor = None;
        return;
    };
    for _ in 0..128 {
        let next = if let Some(after) = cursor.as_ref() {
            batch.index.range((Excluded(after), Unbounded)).next()
        } else {
            batch.index.iter().next()
        };
        let Some((path, parent)) = next else {
            *cursor = None;
            break;
        };
        if let Some(id) = &path.terrain_id {
            let selected = path.instances.is_empty()
                && target
                    .applied
                    .as_ref()
                    .is_some_and(|(map, selected, _)| map == &path.root_scene_id && selected == id);
            let request = if selected {
                Some(&*root)
            } else {
                table.values.get(parent)
            };
            if let Some(preview) = request.and_then(|request| request.target.as_ref())
                && preview.scene_id == path.root_scene_id
                && preview.record.terrain_patch.as_ref().is_some_and(|patch| {
                    patch
                        .terrain_id
                        .as_ref()
                        .map_or(*id == TerrainId::legacy(), |owner| owner == id)
                })
                && parents
                    .get(preview.entity)
                    .is_ok_and(|old| old.is_none_or(|old| old.parent() != *parent))
            {
                commands.entity(preview.entity).insert(ChildOf(*parent));
            }
        }
        *cursor = Some(path.clone());
    }
}

fn swap_preview<T: Resource<Mutability = bevy::ecs::component::Mutable> + Default>(
    world: &mut World,
    old: Option<Entity>,
    new: Entity,
) {
    if !world.contains_resource::<T>()
        || !world.contains_resource::<feathrise_scene_core::residency::InstanceResourceTable<T>>()
    {
        return;
    }
    world.resource_scope(|world, mut root: Mut<T>| {
        let mut table = world.resource_mut::<feathrise_scene_core::residency::InstanceResourceTable<T>>();
        if let Some(old) = old
            && let Some(value) = table.values.get_mut(&old)
        {
            std::mem::swap(&mut *root, value);
        } else {
            *root = T::default();
        }
        if let Some(value) = table.values.get_mut(&new) {
            std::mem::swap(&mut *root, value);
        }
    });
}
/// 输入帧只改变所选身份；在下一作者写入前交换少量资源，未选地形继续持有原预览句柄。
pub(super) fn sync_preview_owner(world: &mut World) {
    let Some(target) = world.get_resource::<TerrainObjectAuthoringTarget>() else {
        return;
    };
    let Some(map) = target.scene_id.clone() else {
        return;
    };
    let requested = target.terrain.clone();
    let previous = target.applied.clone();
    let runtime = world.resource::<super::resources::SubsceneRuntime>();
    let Some(id) = requested else {
        if let Some((_, _, old)) = previous {
            let primary = runtime
                .active_entity(
                    &feathrise_scene_content::SceneInstancePath::root(map).terrain(TerrainId::legacy()),
                )
                .unwrap_or(Entity::PLACEHOLDER);
            if old != primary {
                exchange_previews(world, Some(old), primary);
            }
            world.resource_mut::<TerrainObjectAuthoringTarget>().applied = None;
        }
        return;
    };
    let path = feathrise_scene_content::SceneInstancePath::root(map.clone()).terrain(id.clone());
    let entity = runtime.active_entity(&path).unwrap_or(Entity::PLACEHOLDER);
    if previous
        .as_ref()
        .is_some_and(|(owner, target, _)| owner == &map && target == &id)
    {
        world.resource_mut::<TerrainObjectAuthoringTarget>().applied = Some((map, id, entity));
        return;
    }
    let old = previous.as_ref().map(|(_, _, entity)| *entity).or_else(|| {
        runtime.active_entity(
            &feathrise_scene_content::SceneInstancePath::root(map.clone()).terrain(TerrainId::legacy()),
        )
    });
    if old != Some(entity) {
        exchange_previews(world, old, entity);
    }
    world.resource_mut::<TerrainObjectAuthoringTarget>().applied = Some((map, id, entity));
}
fn exchange_previews(world: &mut World, old: Option<Entity>, entity: Entity) {
    swap_height_preview(world, old, entity);
    swap_preview::<feathrise_scene_content::ScenePuddleLiveOverride>(world, old, entity);
    swap_preview::<feathrise_scene_content::ScenePuddleSaveHandoff>(world, old, entity);
    swap_preview::<super::super::surface::DynamicSurfaceAuthorPreview>(world, old, entity);
    swap_preview::<super::super::vegetation::DynamicBladeDraft>(world, old, entity);
    swap_water_preview(world, old, entity);
    swap_preview::<super::super::terrain_topology::TerrainVolumePreviewRequest>(world, old, entity);
    swap_preview::<super::super::volume_stream::TerrainVolumeStreamControl>(world, old, entity);
}

fn swap_water_preview(world: &mut World, old: Option<Entity>, new: Entity) {
    type Draft = super::super::water::DynamicClipmapWaterDraft;
    type Drafts = feathrise_scene_core::residency::InstanceResourceTable<Draft>;
    if !world.contains_resource::<Draft>() || !world.contains_resource::<Drafts>() { return; }
    world.resource_scope(|world, mut root: Mut<Draft>| {
        let mut table = world.resource_mut::<Drafts>();
        if let Some(value) = old.and_then(|old| table.values.get_mut(&old)) {
            root.swap_terrain_channel(value);
        } else {
            root.clear(); // 没有旧地形可停放时仅清当前地形，不能清或移动场景独立水网。
        }
        if let Some(value) = table.values.get_mut(&new) { root.swap_terrain_channel(value); }
    });
}

/// 有旧上下文时原地交换；没有归属可承接时，大高度树只能整体移交场景现有后台回收槽。
fn swap_height_preview(world: &mut World, old: Option<Entity>, new: Entity) {
    type Height = feathrise_scene_content::SparseHeightLiveOverride;
    type Heights = feathrise_scene_core::residency::InstanceResourceTable<Height>;
    if !world.contains_resource::<Height>() || !world.contains_resource::<Heights>() {
        return;
    }
    let old_exists = old.is_some_and(|old| world.resource::<Heights>().values.contains_key(&old));
    if !old_exists && world.resource::<Height>().tile_count() > 0 {
        let height = std::mem::take(&mut *world.resource_mut::<Height>());
        world.resource_mut::<crate::shared::scene_residency::ResidentScenes>().retire_sparse_height(height);
    }
    swap_preview::<Height>(world, old, new);
}

fn inherit<T: Resource<Mutability = bevy::ecs::component::Mutable> + Clone>(
    world: &mut World,
    entity: Entity,
    previous: Option<Entity>,
    root: bool,
) {
    let value = if root {
        world.get_resource::<T>().cloned()
    } else {
        previous.and_then(|previous| {
            world
                .get_resource::<feathrise_scene_core::residency::InstanceResourceTable<T>>()?
                .values
                .get(&previous)
                .cloned()
        })
    };
    if let Some(value) = value
        && let Some(mut table) =
            world.get_resource_mut::<feathrise_scene_core::residency::InstanceResourceTable<T>>()
    {
        table.values.insert(entity, value);
    }
}
/// 来源变化重建实例时按稳定路径承接旧作者输入；另一地形的重建不能丢掉未保存的预览。
pub(super) fn inherit_previews(
    world: &mut World,
    entity: Entity,
    node: &feathrise_scene_content::ExpandedSceneInstance,
    previous: Option<Entity>,
) {
    let Some(object) = &node.terrain else {
        return;
    };
    // 驱动器在resource_scope内持有SubsceneRuntime；旧实例由调用方按稳定路径提供，不能回读World。
    let target = world.resource::<TerrainObjectAuthoringTarget>();
    let root = node.path.instances.is_empty()
        && (target
            .applied
            .as_ref()
            .is_some_and(|(map, id, _)| map == &node.source_scene_id && id == &object.id)
            || target.applied.is_none() && object.id == TerrainId::legacy());
    inherit::<feathrise_scene_content::SparseHeightLiveOverride>(world, entity, previous, root);
    inherit::<feathrise_scene_content::ScenePuddleLiveOverride>(world, entity, previous, root);
    inherit::<feathrise_scene_content::ScenePuddleSaveHandoff>(world, entity, previous, root);
    inherit::<super::super::surface::DynamicSurfaceAuthorPreview>(world, entity, previous, root);
    inherit::<super::super::vegetation::DynamicBladeDraft>(world, entity, previous, root);
    inherit_water_preview(world, entity, previous, root);
    inherit::<super::super::terrain_topology::TerrainVolumePreviewRequest>(
        world, entity, previous, root,
    );
    inherit::<super::super::volume_stream::TerrainVolumeStreamControl>(
        world, entity, previous, root,
    );
}

fn inherit_water_preview(world: &mut World, entity: Entity, previous: Option<Entity>, root: bool) {
    type Draft = super::super::water::DynamicClipmapWaterDraft;
    type Drafts = feathrise_scene_core::residency::InstanceResourceTable<Draft>;
    let value = if root { world.get_resource::<Draft>().map(Draft::terrain_channel_clone) } else {
        previous.and_then(|previous| world.get_resource::<Drafts>()?.values.get(&previous).map(Draft::terrain_channel_clone))
    };
    if let Some(value) = value && let Some(mut table) = world.get_resource_mut::<Drafts>() {
        table.values.insert(entity, value); // 新地形实例只继承自己的作者通道，根水不能被复制为私有覆盖。
    }
}

#[derive(Resource, Default)]
pub(super) struct EmptyTerrainGround(pub super::super::clipmap::DynamicClipmapGroundSurface);

/// 编辑器读取所选对象的运行状态；保持资源表私有，不公开调度器或候选批次。
#[derive(bevy::ecs::system::SystemParam)]
pub struct TerrainObjectResource<'w, T: Resource> {
    root: Res<'w, T>,
    active: Option<Res<'w, feathrise_scene_content::ActiveDynamicScene>>,
    target: Option<Res<'w, TerrainObjectAuthoringTarget>>,
    runtime: Option<Res<'w, super::resources::SubsceneRuntime>>,
    table: Option<Res<'w, feathrise_scene_core::residency::InstanceResourceTable<T>>>,
}
impl<T: Resource> TerrainObjectResource<'_, T> {
    /// 场景根没有独立地形实例；显式读取真实主场景资源，不能随属性焦点借用private状态。
    pub fn for_root(&self, map: &SceneId) -> Option<&T> {
        self.active.as_ref().filter(|active| active.scene_id.as_ref() == Some(map)).map(|_| &*self.root)
    }

    /// 当前画布的状态须同时属于期望对象、已交接作者根和活动实例；交接帧不能借兄弟对象的同号修订。
    pub fn for_object(&self, map: &SceneId, id: &TerrainId) -> Option<&T> {
        let target = self.target.as_ref()?;
        if target.scene_id.as_ref() != Some(map) || target.terrain.as_ref() != Some(id)
            || !target.applied_to(map, id) {
            return None;
        }
        let (entity, status) = self.for_saved_object(map, id)?;
        (target.instance() == Some(entity)).then_some(status)
    }

    /// 保存可等待非当前脏画布；只按完整已采用路径取实例，不以当前选择或根资源替代它的完成证据。
    pub fn for_saved_object(&self, map: &SceneId, id: &TerrainId) -> Option<(Entity, &T)> {
        let entity = self.runtime.as_ref()?.active_entity(
            &feathrise_scene_content::SceneInstancePath::root(map.clone()).terrain(id.clone()),
        )?;
        Some((entity, self.table.as_ref()?.values.get(&entity)?))
    }

    /// 保存只能读取候选的明确失败来解除等待；候选Ready不能替代活动实例的原子采用证据。
    pub fn for_preparing_saved_object(&self, map: &SceneId, id: &TerrainId) -> Option<(Entity, &T)> {
        let path = feathrise_scene_content::SceneInstancePath::root(map.clone()).terrain(id.clone());
        let entity = *self.runtime.as_ref()?.candidate.as_ref()?.index.get(&path)?;
        Some((entity, self.table.as_ref()?.values.get(&entity)?))
    }
}
impl<T: Resource> std::ops::Deref for TerrainObjectResource<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.target
            .as_ref()
            .and_then(|target| target.instance())
            .and_then(|entity| self.table.as_ref()?.values.get(&entity))
            .unwrap_or(&self.root)
    }
}

/// 既有作者工具读取所选对象的源坐标曲面，不能用场景容器或另一块Clipmap的分页信用。
#[derive(bevy::ecs::system::SystemParam)]
pub struct TerrainObjectClipmapGround<'w> {
    root: Res<'w, super::super::clipmap::DynamicClipmapGroundSurface>,
    root_source: Option<Res<'w, super::super::resources::DynamicSceneRuntimeState>>,
    empty: Option<Res<'w, EmptyTerrainGround>>,
    target: Option<Res<'w, TerrainObjectAuthoringTarget>>,
    runtime: Option<Res<'w, super::resources::SubsceneRuntime>>,
    table: Option<
        Res<
            'w,
            feathrise_scene_core::residency::InstanceResourceTable<
                super::super::clipmap::DynamicClipmapGroundSurface,
            >,
        >,
    >,
    sources: Option<Res<'w, feathrise_scene_core::residency::InstanceResourceTable<super::super::resources::DynamicSceneRuntimeState>>>,
}
impl TerrainObjectClipmapGround<'_> {
    /// 作者入口同时核对活动对象、根预览归属与实际高度路径；同图兄弟的相同修订不能互相授信。
    pub fn authoring_snapshot(
        &self,
        map: &SceneId,
        terrain: Option<&TerrainId>,
        height_path: &str,
        live_revision: u64,
    ) -> Option<&super::super::clipmap::ClipmapGroundSnapshot> {
        let (snapshot, root) = if let Some(id) = terrain {
            let target = self.target.as_ref()?;
            if target.scene_id.as_ref() != Some(map) || target.terrain.as_ref() != Some(id)
                || !target.applied_to(map, id) {
                return None;
            }
            let entity = self.runtime.as_ref()?.active_entity(
                &feathrise_scene_content::SceneInstancePath::root(map.clone()).terrain(id.clone()),
            )?;
            if target.instance() != Some(entity) {
                return None; // 完整批次换代尚未交接作者根时，不借新实例的页表使用旧对象live。
            }
            let source = self.sources.as_ref()?.values.get(&entity)?;
            if source.active_scene_id.as_ref() != Some(map)
                || source.sparse_height_path.as_deref() != Some(height_path) {
                return None;
            }
            (self.table.as_ref()?.values.get(&entity)?.snapshot.as_ref()?, source.root?)
        } else {
            if self.target.as_ref().is_some_and(|target| target.terrain.is_some()) {
                return None;
            }
            let source = self.root_source.as_ref()?;
            if source.active_scene_id.as_ref() != Some(map)
                || source.sparse_height_path.as_deref() != Some(height_path) {
                return None;
            }
            (self.root.snapshot.as_ref()?, source.root?)
        };
        (snapshot.scene_id == *map && snapshot.revision[0] == root.to_bits() && snapshot.ready
            && snapshot.authoring_height_revision() == Some(live_revision)).then_some(snapshot)
    }
}
impl std::ops::Deref for TerrainObjectClipmapGround<'_> {
    type Target = super::super::clipmap::DynamicClipmapGroundSurface;
    fn deref(&self) -> &Self::Target {
        let Some(target) = &self.target else {
            return &self.root;
        };
        let Some(id) = &target.terrain else {
            return &self.root;
        };
        target
            .scene_id
            .as_ref()
            .and_then(|map| {
                self.runtime.as_ref()?.active_entity(
                    &feathrise_scene_content::SceneInstancePath::root(map.clone()).terrain(id.clone()),
                )
            })
            .and_then(|entity| self.table.as_ref()?.values.get(&entity))
            .unwrap_or_else(|| self.empty.as_ref().map_or(&self.root, |empty| &empty.0))
    }
}
