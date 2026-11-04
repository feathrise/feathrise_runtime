//! 将正式三维地形加入同一整批事务；保留16实例/32MiB全域硬预算。
use crate::{
    constants::*,
    resources::MeshRevisionRegistry,
    types::{RaytracingBudgetSnapshot, SourceCandidate},
};
use bevy::{
    gltf::{GltfMaterialName, GltfMeshName},
    platform::collections::HashSet,
    prelude::*,
};
use feathrise_environment::terrain_surface::{TerrainSurfaceMaterial, TerrainVolumeSurface};

pub(super) type StaticSourceData<'a> = (
    &'a Mesh3d,
    Option<&'a GltfMeshName>,
    Option<&'a GltfMaterialName>,
    Option<&'a MeshMaterial3d<StandardMaterial>>,
    Option<&'a MeshMaterial3d<TerrainSurfaceMaterial>>,
    Option<&'a TerrainVolumeSurface>,
);

/// 只读取已经联合采用的有限集合，第17个实例即拒绝，不扫描普通摆放。
pub(super) fn append(
    candidates: &mut Vec<SourceCandidate>,
    published: &feathrise_scene::StaticTerrainRaytracingSources,
    sources: &Query<StaticSourceData<'_>>,
    meshes: &Assets<Mesh>,
    revisions: &mut MeshRevisionRegistry,
) -> Result<u32, String> {
    append_meshes(candidates, published.meshes(), sources, meshes, revisions)
}

fn append_meshes<'a>(
    candidates: &mut Vec<SourceCandidate>,
    published: impl Iterator<Item = &'a (Entity, Handle<Mesh>)>,
    sources: &Query<StaticSourceData<'_>>,
    meshes: &Assets<Mesh>,
    revisions: &mut MeshRevisionRegistry,
) -> Result<u32, String> {
    // 材质类型也属于资产身份；高度源和体积地表可能都是扩展材质，不能再固定加一。
    let mut standard_materials = HashSet::new();
    let mut surface_materials = HashSet::new();
    let (_, _, _, standard, surface, _) = sources
        .get(candidates.first().ok_or("缺少正式高度光追源")?.entity)
        .map_err(|_| "正式高度光追源材质尚未就绪")?;
    match (standard, surface) {
        (Some(material), None) => {
            standard_materials.insert(material.id());
        }
        (None, Some(material)) => {
            surface_materials.insert(material.id());
        }
        _ => return Err("正式高度源材质类型不唯一或缺失".into()),
    }
    for (entity, handle) in published {
        if candidates.len() >= MAX_INSTANCE_COUNT as usize {
            return Err(
                "高度代理与三维地形合计超过16个光追primitive，请合并模型材质或减少三维地形".into(),
            );
        }
        let (current, name, material_name, standard, surface, carrier) = sources
            .get(*entity)
            .map_err(|_| "正式三维地形源实体已换代")?;
        if current.0.id() != handle.id() {
            return Err("三维地形Mesh与已采用碰撞不属于同一代".into());
        }
        let mesh = meshes.get(handle).ok_or("三维地形源Mesh尚未加载")?;
        if mesh.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX) || mesh.has_morph_targets() {
            return Err("三维地形光追只接受静态网格".into());
        }
        let (vertices, triangles) = super::validate_source_mesh(mesh, false)?;
        let bytes = vertices * 48 + triangles * 12;
        if bytes > MAX_UPLOAD_BYTES_PER_FRAME {
            return Err("三维地形单项光追上传超过20MiB".into());
        }
        let terrain_surface = match (standard, surface) {
            (Some(material), None) => {
                standard_materials.insert(material.id());
                false
            }
            (None, Some(material)) if carrier.is_some() => {
                surface_materials.insert(material.id());
                true
            }
            _ => return Err("三维地形缺少唯一已登记的不透明材质".into()),
        }; // 普通GLB保留原材质；雕刻地形复用通用地表和既有Deferred租约。
        candidates.push(SourceCandidate {
            contract_index: candidates.len(),
            mesh_name: name.map_or_else(String::new, |name| name.0.clone()),
            material_name: material_name.map_or_else(String::new, |name| name.0.clone()),
            requires_source_uv: false,
            terrain_surface,
            entity: *entity,
            mesh: handle.clone(),
            mesh_id: handle.id(),
            source_revision: revisions.revision(handle.id()),
            vertex_count: vertices,
            triangle_count: triangles,
            derived_bytes: bytes,
            adaptation_ticket: None,
            derived_mesh: None,
        });
    }
    Ok((standard_materials.len() + surface_materials.len()) as u32)
}

pub(super) fn validate_budget(b: RaytracingBudgetSnapshot) -> Result<(), String> {
    if b.instances > MAX_INSTANCE_COUNT
        || b.vertices > MAX_VERTEX_COUNT
        || b.triangles > MAX_TRIANGLE_COUNT
        || b.unique_materials > MAX_UNIQUE_MATERIAL_COUNT
        || b.derived_bytes > MAX_DERIVED_MESH_BYTES
        || b.cpu_working_set_bytes > MAX_CPU_WORKING_SET_BYTES
    {
        return Err(format!(
            "三维地形光追整体预算超限：{}实例/{}顶点/{}三角形/{}材质/{:.2}MiB派生载荷",
            b.instances,
            b.vertices,
            b.triangles,
            b.unique_materials,
            b.derived_bytes as f64 / 1048576.0
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(entity: Entity, mesh: Handle<Mesh>) -> SourceCandidate {
        SourceCandidate {
            contract_index: 0,
            mesh_name: String::new(),
            material_name: String::new(),
            requires_source_uv: false,
            terrain_surface: true,
            entity,
            mesh_id: mesh.id(),
            mesh,
            source_revision: 0,
            vertex_count: 24,
            triangle_count: 12,
            derived_bytes: 24 * 48 + 12 * 12,
            adaptation_ticket: None,
            derived_mesh: None,
        }
    }

    #[test]
    fn textured_volume_and_ordinary_glb_join_same_transaction_with_typed_material_count() {
        use bevy::ecs::system::SystemState;
        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mesh = meshes.add(Cuboid::default());
        let height: Handle<TerrainSurfaceMaterial> =
            bevy::asset::uuid_handle!("7a8c735c-cbb2-42c8-b9bb-200000000001");
        let volume: Handle<TerrainSurfaceMaterial> =
            bevy::asset::uuid_handle!("7a8c735c-cbb2-42c8-b9bb-200000000002");
        let standard: Handle<StandardMaterial> = Handle::default();
        let root = world
            .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(height)))
            .id();
        let near = world
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(volume.clone()),
                TerrainVolumeSurface,
            ))
            .id();
        let far = world
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(volume),
                TerrainVolumeSurface,
            ))
            .id();
        let glb = world
            .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(standard)))
            .id();
        let sources = [
            (near, mesh.clone()),
            (far, mesh.clone()),
            (glb, mesh.clone()),
        ];
        let mut state = SystemState::<Query<StaticSourceData<'_>>>::new(&mut world);
        let query = state.get(&world).unwrap();
        let mut candidates = vec![candidate(root, mesh)];
        let count = append_meshes(
            &mut candidates,
            sources.iter(),
            &query,
            &meshes,
            &mut MeshRevisionRegistry::default(),
        )
        .unwrap();
        assert_eq!(count, 3); // 一个高度地表、一份共用体积地表、一份原GLB标准材质。
        assert_eq!(candidates.len(), 4);
        assert!(candidates[1].terrain_surface && candidates[2].terrain_surface);
        assert!(!candidates[3].terrain_surface);
        assert_eq!(candidates[1].entity, near);
        assert_eq!(candidates[2].entity, far);
        assert_eq!(candidates[3].entity, glb);
    }

    #[test]
    fn unregistered_or_ambiguous_surface_does_not_gain_static_terrain_credit() {
        use bevy::ecs::system::SystemState;
        for ambiguous in [false, true] {
            let mut world = World::new();
            let mut meshes = Assets::<Mesh>::default();
            let mesh = meshes.add(Cuboid::default());
            let surface: Handle<TerrainSurfaceMaterial> = Handle::default();
            let root = world
                .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(surface.clone())))
                .id();
            let volume = world
                .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(surface)))
                .id();
            if ambiguous {
                world.entity_mut(volume).insert((
                    MeshMaterial3d(Handle::<StandardMaterial>::default()),
                    TerrainVolumeSurface,
                ));
            }
            let sources = [(volume, mesh.clone())];
            let mut state = SystemState::<Query<StaticSourceData<'_>>>::new(&mut world);
            let query = state.get(&world).unwrap();
            let mut candidates = vec![candidate(root, mesh)];
            assert!(
                append_meshes(
                    &mut candidates,
                    sources.iter(),
                    &query,
                    &meshes,
                    &mut MeshRevisionRegistry::default()
                )
                .is_err()
            );
            assert_eq!(candidates.len(), 1);
        }
    }

    #[test]
    fn static_terrain_retains_global_budget() {
        let mut b = RaytracingBudgetSnapshot {
            instances: 16,
            vertices: MAX_VERTEX_COUNT,
            triangles: MAX_TRIANGLE_COUNT,
            unique_materials: 16,
            derived_bytes: MAX_DERIVED_MESH_BYTES,
            ..default()
        };
        assert!(validate_budget(b).is_ok());
        b.instances = 17;
        assert!(validate_budget(b).is_err());
        b.instances = 16;
        b.derived_bytes += 1;
        assert!(validate_budget(b).is_err());
    }
}
