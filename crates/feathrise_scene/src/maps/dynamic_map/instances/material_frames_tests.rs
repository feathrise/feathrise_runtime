//! Clipmap高度和地表独立换代时的来源框架回归；仅运行真实材质绑定系统，不写作者地图。

use super::*;
use crate::maps::dynamic_map::clipmap::{ClipmapBounds, ClipmapFlatHeightPage, ClipmapHeightExtension};
use feathrise_environment::terrain_surface::{TerrainSurfaceExtension, TerrainSurfaceIndexedDescriptor};
use feathrise_scene_core::residency::{MapRuntimeScope, MapSourceFrame};

fn surface() -> TerrainSurfaceMaterial {
    TerrainSurfaceMaterial {
        base: StandardMaterial::default(),
        extension: TerrainSurfaceExtension::new_indexed(TerrainSurfaceIndexedDescriptor {
            world_min: Vec2::splat(-50.0),
            world_max: Vec2::splat(50.0),
            weight_map: Handle::default(), index_map: Handle::default(),
            base_color_array: Handle::default(), normal_array: Handle::default(),
            arm_height_array: Handle::default(), layer_table: Handle::default(),
            layer_count: 1, zero_weight: Handle::default(),
        }),
    }
}

fn height(frame: Mat4) -> ClipmapHeightExtension {
    let mut height = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::splat(-50.0), Vec2::splat(50.0)).unwrap(),
        UVec2::splat(800),
        &ClipmapFlatHeightPage { heights: Handle::default(), page_table: Handle::default() },
    ).unwrap();
    height.set_source_frame(frame);
    height
}

fn app(frame: MapSourceFrame) -> App {
    let mut app = App::new();
    app.init_resource::<SparseHeightPageRuntime>()
        .init_resource::<feathrise_environment::terrain_surface::SurfacePuddleMask>()
        .init_resource::<crate::shared::surface_weather::PuddleRuntimeState>()
        .init_resource::<Assets<TerrainSurfaceMaterial>>()
        .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .init_resource::<Assets<ClipmapHeightMaterial>>()
        .init_resource::<Assets<ClipmapWaterMaterial>>()
        .insert_resource(MapRuntimeScope { frame, instance: None })
        .add_systems(Update, bind);
    app
}

#[test]
fn scaled_clipmap_rebinds_replaced_surface_even_when_height_already_has_current_frame() {
    let frame = MapSourceFrame::from_transform(Transform::from_scale(Vec3::splat(42.362328)));
    let mut app = app(frame);
    // 复现原错误：高度框架已正确，新刷涂/保存材质却仍是默认单位阵。
    let wrapper = app.world_mut().resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .add(SparseClipmapSurfaceMaterial { base: surface(), extension: height(frame.source_to_world) });
    app.world_mut().spawn(MeshMaterial3d(wrapper.clone()));
    app.update();
    let materials = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>();
    let material = materials.get(&wrapper).unwrap();
    assert_eq!(material.extension.params.source_to_world, frame.source_to_world);
    assert_eq!(material.base.extension.source_frame(), frame.source_to_world,
        "高度保持42倍缩放时，替换后的地表仍必须恢复同一源矩阵");
    // 源图边缘超过世界一千米；两层逆变换必须读取同一纹素，不能退成中央约100米小方块。
    let source = Vec3::new(37.0, 0.0, -29.0);
    let world = frame.to_world(source);
    let sampled = material.base.extension.source_frame().inverse().transform_point3(world);
    assert!(sampled.distance(source) < 0.0001);
}

#[test]
fn clipmap_surface_and_height_follow_full_trs_and_do_not_modify_stable_materials() {
    let frame = MapSourceFrame::from_transform(Transform {
        translation: Vec3::new(71.0, 18.0, -23.0),
        rotation: Quat::from_euler(EulerRot::YXZ, 0.7, 0.3, -0.2),
        scale: Vec3::new(42.362328, 7.0, 13.0),
    });
    let mut app = app(frame);
    let mut base = surface();
    base.extension.set_source_frame(frame.source_to_world);
    // 反向换代：地表正确而高度仍旧，不能只修复其中一个方向。
    let wrapper = app.world_mut().resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .add(SparseClipmapSurfaceMaterial { base, extension: height(Mat4::IDENTITY) });
    app.world_mut().spawn(MeshMaterial3d(wrapper.clone()));
    app.update();
    let material = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>().get(&wrapper).unwrap();
    assert_eq!(material.extension.params.source_to_world, frame.source_to_world);
    assert_eq!(material.base.extension.source_frame(), frame.source_to_world);
    let tick = app.world().resource_ref::<Assets<SparseClipmapSurfaceMaterial>>().last_changed();
    app.update();
    assert_eq!(app.world().resource_ref::<Assets<SparseClipmapSurfaceMaterial>>().last_changed(), tick,
        "稳定帧只检查固定矩阵，不反复发布材质Modified或重建高度页");

    app.world_mut().resource_mut::<MapRuntimeScope>().frame = MapSourceFrame::default();
    app.update();
    let material = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>().get(&wrapper).unwrap();
    assert_eq!(material.extension.params.source_to_world, Mat4::IDENTITY);
    assert_eq!(material.base.extension.source_frame(), Mat4::IDENTITY);
}

#[test]
fn binding_one_clipmap_instance_does_not_reframe_its_sibling_surface() {
    use feathrise_scene_core::residency::{initialize_instance_context, MapInstanceContextAppExt, MapInstanceOwner};
    let frame = MapSourceFrame::from_transform(Transform::from_scale(Vec3::splat(42.362328)));
    let mut app = app(frame);
    app.init_instance_resource::<SparseHeightPageRuntime>()
        .init_instance_resource::<feathrise_environment::terrain_surface::SurfacePuddleMask>()
        .init_instance_resource::<crate::shared::surface_weather::PuddleRuntimeState>();
    let contexts = [0, 1].map(|_| {
        let context = app.world_mut().spawn_empty().id();
        initialize_instance_context(app.world_mut(), context);
        let material = app.world_mut().resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
            .add(SparseClipmapSurfaceMaterial { base: surface(), extension: height(frame.source_to_world) });
        app.world_mut().spawn((MapInstanceOwner(context), MeshMaterial3d(material.clone())));
        (context, material)
    });
    app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(contexts[0].0);
    app.update();
    let materials = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>();
    assert_eq!(materials.get(&contexts[0].1).unwrap().base.extension.source_frame(), frame.source_to_world);
    assert_eq!(materials.get(&contexts[1].1).unwrap().base.extension.source_frame(), Mat4::IDENTITY,
        "另一画布没有参与本次绑定，不能借当前实例的矩阵");
}
