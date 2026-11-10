//! 可见嵌套地表必须同帧取得/归还隐藏源的 Deferred 租约，稳定帧不写资产。

use super::super::{
    ClipmapBounds, SparseHeightPageRuntime,
    material::{ClipmapFlatHeightPage, ClipmapHeightExtension, SparseClipmapSurfaceMaterial},
};
use super::{
    components::ClipmapRaytracingAnchor, materials::sync_visible_material,
    resources::ClipmapRaytracingRuntime,
};
use crate::scenes::dynamic_scene::sparse_runtime::SparseClipmapDisplay;
use bevy::{material::OpaqueRendererMethod, pbr::ExtendedMaterial, prelude::*};
use feathrise_environment::terrain_surface::{
    TerrainSurfaceDescriptor, TerrainSurfaceExtension, TerrainSurfaceLayer, TerrainSurfaceMaterial,
    TerrainSurfaceMaterialOverride,
};

pub(super) fn terrain() -> TerrainSurfaceMaterial {
    ExtendedMaterial {
        base: StandardMaterial::default(),
        extension: TerrainSurfaceExtension::new(TerrainSurfaceDescriptor {
            world_min: Vec2::ZERO,
            world_max: Vec2::splat(64.0),
            weight_maps: [Handle::default(), Handle::default()],
            weight_map_count: 1,
            base_color_array: Handle::default(),
            arm_array: Handle::default(),
            normal_array: Handle::default(),
            index_map: Handle::default(),
            layer_table: Handle::default(),
            layers: [TerrainSurfaceLayer::default(); 8],
            layer_count: 1,
            residual_layer: 0,
            grass_layer: 0,
            grass_micro_normal_strength: 0.0,
            grass_variation_cell_meters: 1.0,
        }),
    }
}

#[test]
fn visible_nested_material_tracks_exact_lease_and_restores_without_stable_writes() {
    let mut app = App::new();
    app.init_resource::<Assets<TerrainSurfaceMaterial>>()
        .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .init_resource::<SparseHeightPageRuntime>()
        .init_resource::<ClipmapRaytracingRuntime>()
        .init_resource::<super::ClipmapRaytracingInstanceRoute>()
        .add_systems(PostUpdate, sync_visible_material);
    let author = terrain();
    let source = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(author.clone());
    let mut deferred = author.clone();
    deferred.base.opaque_render_method = OpaqueRendererMethod::Deferred;
    deferred
        .extension
        .set_dynamic_terrain_shadow_terminator_data(true);
    let leased = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(deferred);
    let height = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::ZERO, Vec2::splat(64.0)).unwrap(),
        UVec2::splat(64),
        &ClipmapFlatHeightPage {
            heights: Handle::default(),
            page_table: Handle::default(),
        },
    )
    .unwrap();
    let wrapper = app
        .world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .add(ExtendedMaterial {
            base: author,
            extension: height.clone(),
        });
    let display = app
        .world_mut()
        .spawn((
            SparseClipmapDisplay,
            MeshMaterial3d(wrapper.clone()),
            Transform::IDENTITY,
        ))
        .id();
    app.world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .get_mut(&wrapper)
        .unwrap()
        .base
        .extension
        .set_flat_shading(true); // F12 当前模式属于可见材质，不属于隐藏源的旧快照。
    let mut input = super::tests::input(
        feathrise_scene_content::SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -10.0, 20.0).unwrap(),
    );
    input.identity.display = display;
    let anchor = app
        .world_mut()
        .spawn((
            ClipmapRaytracingAnchor {
                axes: super::super::material::ClipmapProxyAxes::default(),
                identity: input.identity,
                grid: input.grid,
                source: source.clone(),
            },
            MeshMaterial3d(leased.clone()),
            TerrainSurfaceMaterialOverride::new(&source, &leased),
        ))
        .id();
    app.world_mut()
        .resource_mut::<ClipmapRaytracingRuntime>()
        .anchor = Some(anchor);
    app.world_mut()
        .resource_mut::<SparseHeightPageRuntime>()
        .set_raytracing_material_for_test(source.clone());
    app.update();
    let material = app
        .world()
        .resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .get(&wrapper)
        .unwrap();
    assert_eq!(
        material.base.base.opaque_render_method,
        OpaqueRendererMethod::Deferred
    );
    assert_eq!(material.extension.params.reserved, input.grid.uniform());
    assert_eq!(material.extension.heights, height.heights);
    assert!(
        material
            .base
            .extension
            .dynamic_terrain_shadow_terminator_data_enabled()
    );
    assert!(material.base.extension.flat_shading_enabled());
    let tick = app
        .world()
        .get_resource_ref::<Assets<SparseClipmapSurfaceMaterial>>()
        .unwrap()
        .last_changed();
    app.world_mut()
        .entity_mut(display)
        .insert(Transform::from_xyz(12.0, 0.0, 8.0));
    app.update();
    assert_eq!(
        tick,
        app.world()
            .get_resource_ref::<Assets<SparseClipmapSurfaceMaterial>>()
            .unwrap()
            .last_changed()
    );
    // 句柄、页修订和租约都未变化的原地重包，也必须恢复光追门，不能只靠缓存身份跳过。
    app.world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .get_mut(&wrapper)
        .unwrap()
        .base = terrain();
    app.world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .get_mut(&wrapper)
        .unwrap()
        .base
        .extension
        .set_flat_shading(true);
    app.update();
    assert_eq!(
        app.world()
            .resource::<Assets<SparseClipmapSurfaceMaterial>>()
            .get(&wrapper)
            .unwrap()
            .base
            .base
            .opaque_render_method,
        OpaqueRendererMethod::Deferred
    );
    assert!(
        app.world()
            .resource::<Assets<SparseClipmapSurfaceMaterial>>()
            .get(&wrapper)
            .unwrap()
            .base
            .extension
            .dynamic_terrain_shadow_terminator_data_enabled()
    );
    // 分页重包外层高度后，即使没有额外发布身份变化，也应恢复固定三角映射。
    app.world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .get_mut(&wrapper)
        .unwrap()
        .extension = height.clone();
    app.update();
    assert_eq!(
        app.world()
            .resource::<Assets<SparseClipmapSurfaceMaterial>>()
            .get(&wrapper)
            .unwrap()
            .extension
            .params
            .reserved,
        input.grid.uniform()
    );
    app.world_mut()
        .entity_mut(anchor)
        .remove::<TerrainSurfaceMaterialOverride>();
    app.update();
    let restored = app
        .world()
        .resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .get(&wrapper)
        .unwrap();
    assert_eq!(
        restored.base.base.opaque_render_method,
        OpaqueRendererMethod::Auto
    );
    assert_eq!(restored.extension.params.reserved, Vec4::ZERO);
    assert!(
        !restored
            .base
            .extension
            .dynamic_terrain_shadow_terminator_data_enabled()
    );
    assert!(restored.base.extension.flat_shading_enabled());
    assert_eq!(restored.extension.heights, height.heights); // 光追关闭不能撤销真实 GPU 高度页。
}
