//! 多实例地图的完整GPU源证明及实际高度实例路由，沿用官方binder的查询顺序。
use crate::{resources::RaytracingRuntime, types::RaytracingPrototypeTarget};
use bevy::{
    asset::AssetId,
    pbr::PreviousGlobalTransform,
    prelude::*,
    render::{
        extract_resource::ExtractResource,
        mesh::{RenderMesh, allocator::MeshAllocator},
        render_asset::RenderAssets,
        texture::GpuImage,
    },
};
use bevy_solari::prelude::RaytracingMesh3d;

#[derive(Resource, Clone, Default, PartialEq, Eq, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct TerrainGeometryTargets {
    generation: u64,
    meshes: Vec<AssetId<Mesh>>,
    materials: Vec<(AssetId<StandardMaterial>, [Option<AssetId<Image>>; 4])>,
}

pub(crate) fn publish_geometry_targets(
    runtime: Res<RaytracingRuntime>,
    materials: Res<Assets<StandardMaterial>>,
    mut targets: ResMut<TerrainGeometryTargets>,
    mut route: ResMut<feathrise_scene::ClipmapRaytracingInstanceRoute>,
) {
    let mut next = TerrainGeometryTargets::default();
    if runtime.target == RaytracingPrototypeTarget::DynamicMapTerrain
        && runtime.gpu_assets_prepared
        && runtime.pending_rollback.is_none()
    {
        next.generation = runtime.generation;
        for (candidate, prepared) in runtime.candidates.iter().zip(&runtime.prepared_materials) {
            if let Some(mesh) = candidate.derived_mesh.as_ref() {
                next.meshes.push(mesh.id());
            }
            let handle = prepared.binding.proxy_material();
            if let Some(material) = materials.get(&handle) {
                next.materials.push((
                    handle.id(),
                    [
                        material.base_color_texture.as_ref().map(Handle::id),
                        material.normal_map_texture.as_ref().map(Handle::id),
                        material.emissive_texture.as_ref().map(Handle::id),
                        material.metallic_roughness_texture.as_ref().map(Handle::id),
                    ],
                ));
            }
        }
    }
    let index = if runtime.render_telemetry.geometry_generation == runtime.generation
        && next.meshes.len() == next.materials.len()
    {
        runtime.render_telemetry.height_instance_index
    } else {
        None
    };
    let routed = feathrise_scene::ClipmapRaytracingInstanceRoute {
        anchor: index.and(runtime.root),
        index: index.unwrap_or(0),
    };
    if *route != routed {
        *route = routed;
    }
    if *targets != next {
        *targets = next;
    } // 成员不变时不触发重复提取或材质Modified。
}

/// 与Bevy 0.19.1 binder使用完全相同的Query签名，不假设高度位于实例零。
pub(super) fn geometry_proof(
    targets: &TerrainGeometryTargets,
    instances: &Query<(
        Entity,
        &RaytracingMesh3d,
        &MeshMaterial3d<StandardMaterial>,
        &GlobalTransform,
        Option<&PreviousGlobalTransform>,
    )>,
    meshes: &RenderAssets<RenderMesh>,
    allocator: &MeshAllocator,
    textures: &RenderAssets<GpuImage>,
) -> (u64, u32, Option<u32>) {
    if targets.meshes.is_empty()
        || targets.meshes.len() != targets.materials.len()
        || instances.iter().len() != targets.meshes.len()
    {
        return (targets.generation, 0, None);
    }
    // 全部派生Mesh为同一四属性格式；官方PrepareAssets先构造BLAS，再发布RenderMesh。
    // 全部几何和四类纹理完整时binder不会跳槽，此时查询顺序才是可用于GBuffer的实例顺序。
    let mut ids = Vec::with_capacity(16);
    for (_, mesh, material, _, _) in instances.iter().take(17) {
        if ids.len() >= 16
            || meshes.get(mesh.id()).is_none()
            || allocator.mesh_vertex_slice(&mesh.id()).is_none()
            || allocator.mesh_index_slice(&mesh.id()).is_none()
        {
            return (targets.generation, 0, None);
        }
        let Some((_, images)) = targets
            .materials
            .iter()
            .find(|(id, _)| *id == material.id())
        else {
            return (targets.generation, 0, None);
        };
        if images
            .iter()
            .flatten()
            .any(|id| textures.get(*id).is_none())
        {
            return (targets.generation, 0, None);
        }
        ids.push(mesh.id());
    }
    let index = complete_instance_route(&targets.meshes, &ids);
    (
        targets.generation,
        index.map_or(0, |_| ids.len() as u32),
        index,
    )
}

fn complete_instance_route(expected: &[AssetId<Mesh>], actual: &[AssetId<Mesh>]) -> Option<u32> {
    if expected.is_empty() || expected.len() != actual.len() || expected.len() > 16 {
        return None;
    }
    // 每个源产生独立派生Mesh；精确集合比较可拒绝缺项、重复项与其他地图的代理。
    if actual.iter().any(|id| !expected.contains(id))
        || actual
            .iter()
            .enumerate()
            .any(|(i, id)| actual[..i].contains(id))
    {
        return None;
    }
    actual
        .iter()
        .position(|id| *id == expected[0])
        .map(|index| index as u32)
}

pub(crate) fn geometry_ready(runtime: &RaytracingRuntime) -> bool {
    runtime.target != RaytracingPrototypeTarget::DynamicMapTerrain
        || runtime.expected_instance_count() == 1
        || (runtime.render_telemetry.geometry_generation == runtime.generation
            && runtime.render_telemetry.geometry_count == runtime.expected_instance_count()
            && runtime.render_telemetry.height_instance_index.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_follows_binding_order_and_rejects_partial_or_duplicate_sets() {
        let mut meshes = Assets::<Mesh>::default();
        let a = meshes.add(Mesh::from(Cuboid::default())).id();
        let b = meshes.add(Mesh::from(Cuboid::default())).id();
        let c = meshes.add(Mesh::from(Cuboid::default())).id();
        assert_eq!(complete_instance_route(&[a, b, c], &[b, c, a]), Some(2));
        assert_eq!(complete_instance_route(&[a, b, c], &[a, b]), None);
        assert_eq!(complete_instance_route(&[a, b, c], &[a, b, b]), None);
    }
}
