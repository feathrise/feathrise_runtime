//! 只认领明确地图静态来源；骨骼、形变、水体与已有专用LOD从祖先链排除。
use super::{resources::MeshLodRuntime, types::Owner};
use bevy::{mesh::skinning::SkinnedMesh, prelude::*};
use feathrise_environment::vegetation::{VegetationMeshBatchNode, VegetationMeshLodNode};
use feathrise_map_content::MapTerrainKind;

pub(super) fn owner(world: &World, entity: Entity) -> Option<Owner> {
    let mut cursor = entity;
    let mut active = true;
    for _ in 0..128 {
        let node = world.get_entity(cursor).ok()?;
        active &= !node.contains::<bevy::ecs::entity_disabling::Disabled>();
        if node
            .get::<avian3d::prelude::RigidBody>()
            .is_some_and(|body| *body != avian3d::prelude::RigidBody::Static)
        {
            return None;
        }
        if node.contains::<SkinnedMesh>()
            || node.contains::<bevy::mesh::morph::MorphWeights>()
            || node.contains::<VegetationMeshLodNode>()
            || node.contains::<VegetationMeshBatchNode>()
            || node.contains::<crate::shared::large_map::DynamicWaterSurface>()
            || node
                .get::<Name>()
                .is_some_and(|name| name.as_str().starts_with("规划_"))
        {
            return None;
        }
        if let Some(instance) = node.get::<crate::MapPlacementInstance>() {
            if instance.is_terrain_patch() {
                return None;
            }
            return Some(Owner {
                root: cursor,
                policy: instance.mesh_lod_policy(),
                building: instance.is_building(),
                active: active && instance.is_active(),
            });
        }
        if let Some(instance) = node.get::<crate::TerrainObjectInstance>() {
            if !instance.definition.enabled
                || !matches!(
                    instance.definition.terrain.kind,
                    MapTerrainKind::Gltf | MapTerrainKind::Heightfield
                )
            {
                return None;
            }
            // 可雕刻GLB的隐藏输入保持原始身份，仅正式采用显示进入通用LOD。
            if instance.definition.terrain.kind == MapTerrainKind::Heightfield
                && world
                    .get::<crate::DynamicMapTerrainPrimitive>(entity)
                    .is_none()
                && world.get::<avian3d::prelude::Collider>(entity).is_none()
            {
                return None;
            }
            return Some(Owner {
                root: cursor,
                policy: instance.definition.mesh_lod.unwrap_or_default(),
                building: false,
                active: active && node.contains::<crate::SubMapActive>(),
            });
        }
        cursor = node.get::<ChildOf>()?.parent();
    }
    None // 层级过深或歧义时失败关闭，不把其他地图/角色认作静态网格。
}

pub(super) fn hierarchy_changed(
    event: On<Insert<ChildOf>>,
    mut runtime: Option<ResMut<MeshLodRuntime>>,
) {
    if let Some(runtime) = runtime.as_mut() {
        runtime.discovery.push_back((event.entity, 0));
    }
}

pub(super) fn owner_inserted<T: Component>(
    event: On<Insert<T>>,
    mut runtime: Option<ResMut<MeshLodRuntime>>,
) {
    if let Some(runtime) = runtime.as_mut() {
        runtime.discovery.push_back((event.entity, 0));
    }
}

pub(super) fn world_ready(
    event: On<bevy::world_serialization::WorldInstanceReady>,
    mut runtime: Option<ResMut<MeshLodRuntime>>,
) {
    if let Some(runtime) = runtime.as_mut() {
        runtime.discovery.push_back((event.entity, 0));
    }
}
