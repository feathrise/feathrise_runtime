//! 地图实例与地形实例共享调度，但作者句柄严格隔离；不会为地形创建伪MapId或重复角色。

use bevy::prelude::*;
use feathrise_map_content::{ExpandedMapInstance, MapContentRegistry, MapManifestAsset, TerrainId};
use feathrise_scene_core::residency::InstanceResourceTable;
use std::collections::BTreeMap;

/// 运行实例持有明确的地形定义；后端和编辑器查询共享稳定身份。
#[derive(Component, Clone)]
pub struct TerrainObjectInstance {
    pub definition: std::sync::Arc<feathrise_map_content::MapTerrainObject>,
}

/// 拖动只发布单对象姿态，输入帧不复制整个对象列表。
#[derive(Resource, Default)]
pub struct TerrainObjectTransformPreview {
    pub pose: Option<(feathrise_map_content::MapId, TerrainId, feathrise_map_content::TerrainObjectTransform)>,
    /// 最后预览目标保留至恢复完成，取消后也必须把ECS姿态还原。
    applied: Option<Entity>,
}
impl TerrainObjectTransformPreview {
    pub fn for_object(&self, owner: &feathrise_map_content::MapId, id: &TerrainId) -> Option<feathrise_map_content::TerrainObjectTransform> {
        self.pose.as_ref().filter(|(map, target, _)| map == owner && target == id).map(|(_, _, pose)| *pose)
    }
}

/// 每次只登记一个已初始化的地形路径；不得克隆活动/候选的完整索引补给子调度。
pub(in crate::maps::dynamic_map) fn register_lookup(world: &mut World, batch: Entity, path: &feathrise_map_content::MapInstancePath, terrain: Entity) {
    let Some(instance) = world.get::<super::SubMapInstance>(terrain) else { return; };
    let Some(object) = world.get::<TerrainObjectInstance>(terrain) else { return; };
    if instance.batch != batch || &instance.path != path || path.terrain_id.as_ref() != Some(&object.definition.id) { return; }
    world.resource_mut::<super::resources::TerrainInstanceLookup>().batches.entry(batch).or_default()
        .entities.insert(path.clone(), terrain);
}

/// 节点创建完整后才证明缺失地址为失效；无地形批次也保留这一小型完成证据。
pub(in crate::maps::dynamic_map) fn seal_lookup(world: &mut World, batch: Entity) {
    if world.resource::<super::resources::TerrainInstanceLookup>().contexts_complete(batch) { return; }
    world.resource_mut::<super::resources::TerrainInstanceLookup>().batches.entry(batch).or_default().contexts_complete = true;
}

/// 显示树可能已先despawn；用退役事务保存的路径精确移除，不扫描实体或整张地址表。
pub(in crate::maps::dynamic_map) fn remove_lookup(world: &mut World, batch: Entity, path: &feathrise_map_content::MapInstancePath, terrain: Entity) {
    let mut lookup = world.resource_mut::<super::resources::TerrainInstanceLookup>();
    let remove_batch = if let Some(row) = lookup.batches.get_mut(&batch) {
        if row.entities.get(path) == Some(&terrain) { row.entities.remove(path); }
        row.entities.is_empty()
    } else { false };
    if remove_batch { lookup.batches.remove(&batch); } // 最后一个路径退役后不残留空行或完成信用。
}

/// 真空地图没有可逐节点移除的地形地址；最终批次退役仍须释放其小型完成行。
pub(in crate::maps::dynamic_map) fn remove_empty_lookup(world: &mut World, batch: Entity) {
    let empty = world.resource::<super::resources::TerrainInstanceLookup>().batches.get(&batch)
        .is_some_and(|row| row.entities.is_empty());
    if empty { world.resource_mut::<super::resources::TerrainInstanceLookup>().batches.remove(&batch); }
}

pub(super) fn apply_preview(mut preview: ResMut<TerrainObjectTransformPreview>, runtime: Res<super::resources::SubMapRuntime>,
    mut targets: Query<(&TerrainObjectInstance, &mut Transform, &mut feathrise_scene_core::residency::MapSourceFrame)>) {
    let Some((owner, id, pose)) = &preview.pose else {
        if let Some(entity) = preview.applied.take() && let Ok((object, mut transform, mut frame)) = targets.get_mut(entity) {
            *transform = object.definition.transform.as_transform();
            *frame = feathrise_scene_core::residency::MapSourceFrame::from_transform(*transform);
        }
        return;
    };
    let path = feathrise_map_content::MapInstancePath::root(owner.clone()).terrain(id.clone());
    let Some(entity) = runtime.active_entity(&path) else { return; };
    let Ok((object, mut transform, mut frame)) = targets.get_mut(entity) else { return; };
    let next = pose.as_transform();
    if object.definition.transform == *pose && *transform == next {
        preview.pose = None; preview.applied = None; return; // 清单已采用同一姿态，停止稳定帧重复标记Transform变化。
    }
    if *transform != next {
        *transform = next; *frame = feathrise_scene_core::residency::MapSourceFrame::from_transform(next);
    }
    preview.applied = Some(entity);
}

pub(super) fn initialize(world: &mut World, entity: Entity, node: &ExpandedMapInstance, previous: Option<Entity>) {
    let (catalog, revision, mut handles) = {
        let registry = world.resource::<MapContentRegistry>();
        (registry.catalog.clone(), registry.revision, registry.handles(&node.source_map_id)
            .expect("完整来源树只能包含已登记地图").clone())
    };
    if let Some(object) = &node.terrain {
        super::authoring_target::inherit_previews(world, entity, node, previous);
        world.entity_mut(entity).insert(TerrainObjectInstance { definition: object.clone() });
        let owner = world.resource::<Assets<MapManifestAsset>>().get(&handles.manifest)
            .expect("完整来源树的地图清单必须仍有强句柄").clone();
        let projected=object.source_manifest(&owner);
        if !projected.terrain.resolved_stitches.is_empty() && let Some(pages)=world.resource_mut::<InstanceResourceTable<super::super::clipmap::SparseHeightPageRuntime>>().values.get_mut(&entity) {
            pages.seed_stitches(&projected.terrain.resolved_stitches);
        }
        handles.manifest = world.resource_mut::<Assets<MapManifestAsset>>().add(projected);
        let legacy = object.id == TerrainId::legacy();
        if legacy && node.path.instances.is_empty() {
            super::super::volume_stream::adopt_root_runtime(world, entity);
        }
        let server = world.resource::<AssetServer>();
        handles.surface = object.surface_path.as_ref().map(|path| server.load(path.clone()))
            .or_else(|| legacy.then(|| handles.surface.clone()).flatten());
        handles.vegetation = object.vegetation_path.as_ref().map(|path| server.load(path.clone()))
            .or_else(|| legacy.then(|| handles.vegetation.clone()).flatten());
        handles.water = object.water_path.as_ref().map(|path| server.load(path.clone()))
            .or_else(|| legacy.then(|| handles.water.clone()).flatten());
        handles.characters_path = None;
        handles.boundaries = None;
        // 摆放文件仍只属于地图；地形域按terrain_id筛选关联补丁，不复制普通摆放或吞掉保存的洞穴。
        // 地形调度不运行摆放/传送/角色域；这些句柄不参与该地形的就绪门。
    } else if node.terrain.is_none() {
        let owner = world.resource::<Assets<MapManifestAsset>>().get(&handles.manifest).unwrap();
        if owner.terrains.is_some() {
            let mut container = owner.clone(); container.puddles = None;
            handles.manifest = world.resource_mut::<Assets<MapManifestAsset>>().add(container);
            handles.surface = None; handles.vegetation = None; handles.water = None;
        }
    }
    world.resource_mut::<InstanceResourceTable<MapContentRegistry>>().values.insert(entity,
        MapContentRegistry { catalog, revision, maps: BTreeMap::from([(node.source_map_id.clone(), handles)]), last_error: None });
}
