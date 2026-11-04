//! 下游无窗口交互回归的场景夹具；安装真实身份表和源父，不模拟GPU加载或内容采用流程。

use super::{SubMapActive, SubMapInstance, TerrainObjectInstance, components::SubMapBatch,
    resources::SubMapRuntime};
use bevy::prelude::*;
use feathrise_map_content::{MapId, MapInstancePath, MapTerrainObject};
use feathrise_scene_core::residency::MapSourceFrame;
use std::{collections::BTreeMap, sync::Arc};

pub use crate::shared::placement_data::runtime_test_support::{
    PendingPlacementActivation, pending_placement_instance,
}; // 统一测试入口；正式构建不包含可写的摆放批次门。

/// 下游回归必须调用真实作者根资源交换，不得手设私有applied身份绕过生产所有权。
pub fn apply_authoring_target(world: &mut World) {
    super::authoring_target::sync_preview_owner(world);
}

/// 下游整体姿态桥的回归驱动真实单目标消费者，不直接写实体Transform伪造采用。
pub fn apply_terrain_pose_preview(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world.run_system_once(super::terrain_sources::apply_preview)
        .expect("活动地形夹具必须安装原姿态消费者所需资源");
}

/// 在已建的自有批次内登记一个子地图根，供下游稳定ID门回归；不模拟加载/GPU就绪。
pub fn install_active_sub_map_instance(world: &mut World, owner: &MapId,
    placement: &feathrise_map_content::MapInstancePlacement) -> Entity {
    let runtime = world.resource::<SubMapRuntime>();
    assert_eq!(runtime.root_map_id.as_ref(), Some(owner));
    let batch = runtime.active.as_ref().expect("必须先安装自有活动批次").root;
    let path = MapInstancePath::root(owner.clone()).child(placement.id.clone());
    assert!(runtime.active_entity(&path).is_none());
    let transform = placement.transform.as_transform();
    let entity = world.spawn((SubMapInstance { path: path.clone(), source_map_id: placement.source_map_id.clone(),
        generation: 1, batch }, MapSourceFrame::from_transform(transform), transform,
        Visibility::Inherited, ChildOf(batch))).id();
    world.resource_mut::<SubMapRuntime>().active.as_mut().unwrap().index.insert(path, entity);
    entity
}

/// 构造一次已采用的地形集合，返回与输入同序的真实父实体，供属性/手柄及层级传播回归使用。
/// 只适用于独立测试World；调用方不能把这份地址表当成实际GLB、Clipmap分页或GPU就绪证据。
pub fn install_active_terrain_instances(
    world: &mut World,
    owner: &MapId,
    objects: &[MapTerrainObject],
) -> Vec<Entity> {
    assert!(!world.contains_resource::<SubMapRuntime>(), "地形夹具不能覆盖已有场景运行时");
    assert!(objects.iter().all(|object| object.enabled && object.transform.validate().is_ok()),
        "地形夹具仅接受已启用且姿态有效的定义");
    let batch = world.spawn((SubMapBatch, SubMapActive, Transform::IDENTITY, Visibility::Inherited)).id();
    let mut index = BTreeMap::new();
    let mut parents = Vec::with_capacity(objects.len());
    for object in objects {
        let path = MapInstancePath::root(owner.clone()).terrain(object.id.clone());
        assert!(!index.contains_key(&path), "活动地形夹具不能包含重复来源ID");
        let transform = object.transform.as_transform();
        let parent = world.spawn((
            SubMapInstance { path: path.clone(), source_map_id: owner.clone(), generation: 1, batch },
            TerrainObjectInstance { definition: Arc::new(object.clone()) },
            MapSourceFrame::from_transform(transform),
            transform, Visibility::Inherited, ChildOf(batch),
        )).id();
        index.insert(path, parent);
        parents.push(parent);
    }
    let mut runtime = SubMapRuntime::test_active(batch, index);
    runtime.root_map_id = Some(owner.clone());
    runtime.generation = 1;
    world.insert_resource(runtime);
    parents
}
