//! 原生块共用对象地表候选；代表网格走原有五纹理预热，全部块原子刷新同一材质资产。
use super::super::{
    DYNAMIC_TERRAIN_MESH_NAME, DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME, surface::DynamicSurfaceState,
};
use bevy::{
    gltf::{GltfMaterialName, GltfMeshName},
    prelude::*,
    render::storage::ShaderBuffer,
};
use feathrise_environment::terrain_surface::{
    TerrainSurfaceExtension, TerrainSurfaceFallbackImages, TerrainSurfaceMaterial,
};

pub(super) struct Binding {
    pub material: Handle<TerrainSurfaceMaterial>,
    fields: Handle<ShaderBuffer>,
    step: f32,
    top: f32,
    source: Option<AssetId<TerrainSurfaceMaterial>>,
}

/// 正式源只保留一份小型代表网格；显示块不伪装成多个GLTF同名primitive。
/// 它一直隐藏，原地表状态机复制同布局的离屏网格获得真实GPU证明。
pub(super) fn spawn_proxy(
    commands: &mut Commands,
    root: Entity,
    base: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
) -> Entity {
    let mesh = feathrise_environment::terrain_surface::build_volume_surface_mesh(
        &[[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]],
        &[0, 2, 1],
        &[[0., 1., 0.]; 3],
        |_, _| None,
        |_| None,
    )
    .expect("固定地表代表三角必须有效");
    let white = base.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.9,
        ..default()
    });
    commands
        .spawn((
            Name::new("独立体素地表代表源"),
            GltfMeshName(DYNAMIC_TERRAIN_MESH_NAME.into()),
            GltfMaterialName(DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.into()),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(white),
            Transform::IDENTITY,
            Visibility::Hidden,
            crate::DynamicMapTerrainPrimitive,
            ChildOf(root),
        ))
        .id()
}

pub(super) fn create(
    fields: &crate::VolumeSurfaceFields,
    source: &DynamicSurfaceState,
    frame: Mat4,
    top: f32,
    fallback: &TerrainSurfaceFallbackImages,
    materials: &mut Assets<TerrainSurfaceMaterial>,
    buffers: &mut Assets<ShaderBuffer>,
) -> Result<Binding, String> {
    let field_buffer = fields
        .1
        .try_lock()
        .map_err(|_| "体素字段缓冲尚未交接")?
        .take()
        .ok_or("体素字段缓冲已经被另一候选采用")?;
    let field_buffer = buffers.add(field_buffer);
    let mut material = source
        .effective_material()
        .and_then(|handle| materials.get(handle))
        .cloned()
        .unwrap_or_else(|| bevy::pbr::ExtendedMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(0.62, 0.55, 0.43),
                perceptual_roughness: 0.9,
                ..default()
            },
            extension: TerrainSurfaceExtension::new_passthrough(fallback),
        });
    configure(
        &mut material,
        field_buffer.clone(),
        fields.0.cell_size,
        top,
        frame,
    );
    Ok(Binding {
        material: materials.add(material),
        fields: field_buffer,
        step: fields.0.cell_size,
        top,
        source: source.effective_material().map(Handle::id),
    })
}
fn configure(
    material: &mut TerrainSurfaceMaterial,
    fields: Handle<ShaderBuffer>,
    step: f32,
    top: f32,
    frame: Mat4,
) {
    material.extension.set_source_frame(frame);
    material
        .extension
        .set_dynamic_terrain_shadow_terminator_data(false);
    material.extension.set_volume_projection(true);
    material.extension.set_native_box_top(top);
    material
        .extension
        .set_volume_fields(fields, Vec3::ZERO, step);
}
pub(super) fn refresh(
    binding: &mut Binding,
    source: &DynamicSurfaceState,
    frame: Mat4,
    materials: &mut Assets<TerrainSurfaceMaterial>,
) -> bool {
    let source_id = source.effective_material().map(Handle::id);
    let Some(mut next) = source
        .effective_material()
        .and_then(|handle| materials.get(handle))
        .cloned()
    else {
        return false;
    };
    configure(
        &mut next,
        binding.fields.clone(),
        binding.step,
        binding.top,
        frame,
    );
    let changed = binding.source != source_id
        || materials
            .get(&binding.material)
            .is_none_or(|current| current.extension != next.extension);
    if changed {
        // 共享句柄的全部块同时改变，不逐块暴露新旧涂绘；模板的纹理和管线已经真实预热。
        if let Some(mut material) = materials.get_mut(&binding.material) {
            *material = next;
        }
        binding.source = source_id;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_surface_preserves_the_complete_object_frame_and_volume_attributes() {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<ShaderBuffer>>();
        let fallback = TerrainSurfaceFallbackImages::from_world(&mut world);
        let frame = Mat4::from_scale_rotation_translation(
            Vec3::new(2., 3., 4.),
            Quat::from_rotation_y(0.7),
            Vec3::new(10., 20., 30.),
        );
        let mut material = bevy::pbr::ExtendedMaterial {
            base: StandardMaterial::default(),
            extension: TerrainSurfaceExtension::new_passthrough(&fallback),
        };
        configure(
            &mut material,
            fallback.neutral_layer_table.clone(),
            0.25,
            0.,
            frame,
        );
        assert_eq!(material.extension.source_frame(), frame);
        assert!(material.extension.volume_projection_enabled());
        let mesh = feathrise_environment::terrain_surface::build_volume_surface_mesh(
            &[[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]],
            &[0, 2, 1],
            &[[0., 1., 0.]; 3],
            |_, _| None,
            |_| Some(2.),
        )
        .unwrap();
        assert!(mesh.contains_attribute(Mesh::ATTRIBUTE_UV_1));
        assert!(mesh.contains_attribute(Mesh::ATTRIBUTE_COLOR));
    }
}
