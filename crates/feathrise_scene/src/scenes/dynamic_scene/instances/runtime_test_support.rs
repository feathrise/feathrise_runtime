//! 下游无窗口交互回归的场景夹具；安装真实身份表和源父，不模拟GPU加载或内容采用流程。

use super::{SubsceneActive, SubsceneInstance, TerrainObjectInstance, components::SubsceneBatch,
    resources::SubsceneRuntime};
use bevy::prelude::*;
use feathrise_scene_content::{SceneId, SceneInstancePath, SceneTerrainObject};
use feathrise_scene_core::residency::SceneSourceFrame;
use std::{collections::BTreeMap, sync::Arc};

pub use crate::shared::placement_data::runtime_test_support::{
    PendingPlacementActivation, pending_placement_instance,
}; // 统一测试入口；正式构建不包含可写的摆放批次门。

/// 下游回归必须调用真实作者根资源交换，不得手设私有applied身份绕过生产所有权。
pub fn apply_authoring_target(world: &mut World) {
    super::authoring_target::sync_preview_owner(world);
}

/// 下游光标与笔刷回归共享本库的非线性坡面；只读CPU显示页不声称发生过GPU采用。
pub fn clipmap_height_query_fixture(owner: SceneId, revision: u64) -> super::super::clipmap::ClipmapGroundSnapshot {
    super::super::clipmap::test_authoring_ground_snapshot(owner, Entity::PLACEHOLDER, Some(revision))
}

/// 冷作者页须经下游真实光标提交需求，才能从既有缓存逻辑发布同源新版快照。
pub fn cold_clipmap_height_query_fixture(owner: SceneId, revision: u64) -> (
    super::super::clipmap::ClipmapGroundSnapshot,
    impl FnOnce() -> super::super::clipmap::ClipmapGroundSnapshot,
) {
    clipmap_height_query_fixture(owner, revision).with_cold_test_authority()
}

/// 增加真实查询薄壳，保持原可见和作者高度页，供下游测试相邻表面。
pub fn clipmap_with_surface_sheet(
    ground: super::super::clipmap::ClipmapGroundSnapshot,
    point: Vec3,
) -> super::super::clipmap::ClipmapGroundSnapshot {
    super::super::clipmap::test_ground_with_surface_sheet(ground, point)
}

/// 下游雕刻输入测试复用真实实例地址与只读作者页；不能作为窗口/GPU验收的完成证据。
pub fn publish_cpu_authoring_height(
    world: &mut World,
    owner: &SceneId,
    id: &feathrise_scene_content::TerrainId,
    spec: feathrise_scene_content::SparseHeightSpec,
) {
    use super::super::{clipmap::{ClipmapGroundSnapshot, DynamicClipmapGroundSurface}, resources::DynamicSceneRuntimeState};
    use feathrise_scene_core::residency::InstanceResourceTable;
    let path = SceneInstancePath::root(owner.clone()).terrain(id.clone());
    let entity = world.resource::<SubsceneRuntime>().active_entity(&path).expect("先安装自有活动地形地址");
    let source_path = world.get::<TerrainObjectInstance>(entity).unwrap().definition.terrain.height_path.clone();
    let snapshot = ClipmapGroundSnapshot::cpu_authoring_fixture(owner.clone(), entity, spec,
        world.resource::<feathrise_scene_content::SparseHeightLiveOverride>());
    world.init_resource::<DynamicClipmapGroundSurface>();
    world.init_resource::<InstanceResourceTable<DynamicClipmapGroundSurface>>();
    world.init_resource::<InstanceResourceTable<DynamicSceneRuntimeState>>();
    world.resource_mut::<InstanceResourceTable<DynamicClipmapGroundSurface>>().values.entry(entity).or_default().snapshot = Some(snapshot);
    world.resource_mut::<InstanceResourceTable<DynamicSceneRuntimeState>>().values.insert(entity, DynamicSceneRuntimeState {
        active_scene_id: Some(owner.clone()), sparse_height_path: source_path, root: Some(entity), ..default()
    });
    world.init_resource::<super::authoring_target::TerrainObjectAuthoringTarget>();
    let mut target = world.resource_mut::<super::authoring_target::TerrainObjectAuthoringTarget>();
    target.scene_id = Some(owner.clone()); target.terrain = Some(id.clone());
    drop(target);
    apply_authoring_target(world); // applied身份仍由正式作者根交换产生，测试不能私设已采用字段。
}

/// 下游整体姿态桥的回归驱动真实单目标消费者，不直接写实体Transform伪造采用。
pub fn apply_terrain_pose_preview(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world.run_system_once(super::terrain_sources::apply_preview)
        .expect("活动地形夹具必须安装原姿态消费者所需资源");
}

/// 在已建的自有批次内登记一个子场景根，供下游稳定ID门回归；不模拟加载/GPU就绪。
pub fn install_active_subscene_instance(world: &mut World, owner: &SceneId,
    placement: &feathrise_scene_content::SceneInstancePlacement) -> Entity {
    let runtime = world.resource::<SubsceneRuntime>();
    assert_eq!(runtime.root_scene_id.as_ref(), Some(owner));
    let batch = runtime.active.as_ref().expect("必须先安装自有活动批次").root;
    let path = SceneInstancePath::root(owner.clone()).child(placement.id.clone());
    assert!(runtime.active_entity(&path).is_none());
    let transform = placement.transform.as_transform();
    let entity = world.spawn((SubsceneInstance { path: path.clone(), source_scene_id: placement.source_scene_id.clone(),
        generation: 1, batch }, SceneSourceFrame::from_transform(transform), transform,
        Visibility::Inherited, ChildOf(batch))).id();
    world.resource_mut::<SubsceneRuntime>().active.as_mut().unwrap().index.insert(path, entity);
    entity
}

/// 构造一次已采用的地形集合，返回与输入同序的真实父实体，供属性/手柄及层级传播回归使用。
/// 只适用于独立测试World；调用方不能把这份地址表当成实际GLB、Clipmap分页或GPU就绪证据。
pub fn install_active_terrain_instances(
    world: &mut World,
    owner: &SceneId,
    objects: &[SceneTerrainObject],
) -> Vec<Entity> {
    assert!(!world.contains_resource::<SubsceneRuntime>(), "地形夹具不能覆盖已有场景运行时");
    assert!(objects.iter().all(|object| object.enabled && object.transform.validate().is_ok()),
        "地形夹具仅接受已启用且姿态有效的定义");
    let batch = world.spawn((SubsceneBatch, SubsceneActive, Transform::IDENTITY, Visibility::Inherited)).id();
    let mut index = BTreeMap::new();
    let mut parents = Vec::with_capacity(objects.len());
    for object in objects {
        let path = SceneInstancePath::root(owner.clone()).terrain(object.id.clone());
        assert!(!index.contains_key(&path), "活动地形夹具不能包含重复来源ID");
        let transform = object.transform.as_transform();
        let parent = world.spawn((
            SubsceneInstance { path: path.clone(), source_scene_id: owner.clone(), generation: 1, batch },
            TerrainObjectInstance { definition: Arc::new(object.clone()) },
            SceneSourceFrame::from_transform(transform),
            transform, Visibility::Inherited, ChildOf(batch),
        )).id();
        index.insert(path, parent);
        parents.push(parent);
    }
    let mut runtime = SubsceneRuntime::test_active(batch, index);
    runtime.root_scene_id = Some(owner.clone());
    runtime.generation = 1;
    world.insert_resource(runtime);
    parents
}
