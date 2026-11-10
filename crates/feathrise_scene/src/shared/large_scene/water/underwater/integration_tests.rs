//! 从真实水体材质遍历到相机外观的无窗口回归，验证水域边界、整体变换和出水恢复。
use bevy::{prelude::*, render::view::ColorGrading};
use feathrise_scene_core::view::PrimarySceneView;
use feathrise_scene_content::{ActiveDynamicScene, SceneId};
use feathrise_scene_core::state::ScenePhase;
use super::{resources::UnderwaterViewState, systems::{prepare_view, apply_view}, sample_column};
use super::super::{
    components::{DynamicWaterCpuInteraction, DynamicWaterSurface, WaterJumpCycle},
    footprint::{PaintedWaterFootprint, WaterInteractionFootprint},
    material::{DynamicWaterMaterial, create_dynamic_water_material},
    splash::SplashWaterTarget, systems::update_dynamic_water_surfaces,
};

#[test]
fn actual_water_footprint_drives_camera_and_clears_on_exit_hide_and_scene_change() {
    let mut app = App::new();
    app.init_resource::<Time>().init_resource::<UnderwaterViewState>()
        .init_resource::<avian3d::collider_tree::ColliderTrees>()
        .init_resource::<Assets<DynamicWaterMaterial>>().init_resource::<SplashWaterTarget>()
        .insert_resource(ClearColor(Color::WHITE))
        .insert_resource(State::new(ScenePhase::Active))
        .insert_resource(ActiveDynamicScene { scene_id: Some(SceneId::parse("MAP-underwater-flow").unwrap()), ..default() })
        .add_systems(PostUpdate, (prepare_view, sample_column, update_dynamic_water_surfaces, apply_view).chain());
    let camera = app.world_mut().spawn((Camera3d::default(), PrimarySceneView,
        GlobalTransform::from_translation(Vec3::new(10.0, 3.0, 20.0)))).id();
    let material = app.world_mut().resource_mut::<Assets<DynamicWaterMaterial>>()
        .add(create_dynamic_water_material(StandardMaterial {
            base_color: Color::srgb(0.05, 0.24, 0.29), ..default()
        }, 0.0, Vec2::X, 1.0, None));
    // 水局部Y=0，整体抬到5m并缩放；相机不能拿世界原点或未变换矩形判断入水。
    let frame = Transform::from_xyz(10.0, 5.0, 20.0)
        .with_rotation(Quat::from_rotation_y(0.5)).with_scale(Vec3::splat(2.0));
    let water = app.world_mut().spawn((DynamicWaterCpuInteraction, MeshMaterial3d(material),
        Visibility::Visible, InheritedVisibility::VISIBLE, GlobalTransform::from(frame),
        DynamicWaterSurface {
            interaction_footprint: WaterInteractionFootprint::Painted(PaintedWaterFootprint::from_visible_mesh(
                [1, 1], [-2.0; 2], [2.0; 2], vec![1.0; 4])),
            rest_water_level: 0.0, flow_direction: Vec2::X, flow_speed: 1.0,
            player_ripples: Vec::new(), last_player_takeoff_sequence: None,
            previous_player_local_xz: None, previous_player_local_y: None,
            distance_since_ripple: 0.0, player_was_in_water: false, jump_cycle: WaterJumpCycle::Idle,
        })).id();
    app.update();
    assert!(app.world().resource::<UnderwaterViewState>().sample.is_none(),
        "没有真实河床的薄水面不得被猜成无限向下的水柱");
    assert!(app.world().get::<DistanceFog>(camera).is_none());
    super::volume::test_insert_terrain_collider(app.world_mut(), Vec3::new(10.0, 0.0, 20.0),
        avian3d::prelude::Collider::cuboid(40.0, 2.0, 40.0)); // 真正河床顶面为Y=1，水位为Y=5。
    super::volume::test_insert_terrain_collider(app.world_mut(), Vec3::new(10.0, -1000.0, 20.0),
        avian3d::prelude::Collider::cuboid(40.0, 2.0, 40.0)); // 地底另有地层也不能穿过上方河床把相机算进湖水。
    app.update();
    assert_eq!(app.world().resource::<UnderwaterViewState>().sample.unwrap().depth, 2.0);
    assert!(app.world().get::<DistanceFog>(camera).is_some());
    assert!(app.world().get::<ColorGrading>(camera).unwrap().global.temperature < 0.0);
    for position in [Vec3::new(30.0, 3.0, 20.0), Vec3::new(10.0, 6.0, 20.0),
        Vec3::new(10.0, 0.5, 20.0), Vec3::new(10.0, -500.0, 20.0)] {
        app.world_mut().entity_mut(camera).insert(GlobalTransform::from_translation(position));
        app.update(); // 水域外、水面上方、河床内部及地底远处均恢复原镜头。
        assert!(app.world().get::<DistanceFog>(camera).is_none());
        assert_eq!(app.world().get::<ColorGrading>(camera).unwrap().global.temperature, 0.0);
    }
    app.world_mut().entity_mut(camera).insert(GlobalTransform::from_translation(Vec3::new(10.0, 3.0, 20.0)));
    app.update();
    assert!(app.world().get::<DistanceFog>(camera).is_some());
    app.world_mut().entity_mut(water).insert(Visibility::Hidden);
    app.update();
    assert!(app.world().get::<DistanceFog>(camera).is_none());
    app.world_mut().entity_mut(water).insert(Visibility::Visible);
    app.update();
    assert!(app.world().get::<DistanceFog>(camera).is_some());
    app.insert_resource(State::new(ScenePhase::Loading));
    app.update();
    assert!(app.world().get::<DistanceFog>(camera).is_none());
}

#[test]
fn clipmap_inherited_hidden_surface_does_not_authorize_underwater_view() {
    use crate::scenes::dynamic_scene::{
        test_ground_snapshot, test_clipmap_water_material, test_clipmap_water_surface, ClipmapWaterMaterial,
    };
    let mut app = App::new();
    let ground = test_ground_snapshot();
    let mut surface = test_clipmap_water_surface();
    surface.scene_id = ground.scene_id.clone();
    surface.height_revision = ground.revision;
    surface.water_level = 24.0;
    let interaction = DynamicWaterSurface::from_clipmap(&surface, &ground, Vec2::X, 1.0);
    app.init_resource::<Time>().init_resource::<UnderwaterViewState>()
        .init_resource::<Assets<DynamicWaterMaterial>>().init_resource::<Assets<ClipmapWaterMaterial>>()
        .init_resource::<SplashWaterTarget>()
        .insert_resource(ClearColor(Color::WHITE))
        .insert_resource(State::new(ScenePhase::Active))
        .insert_resource(ActiveDynamicScene { scene_id: Some(ground.scene_id.clone()), ..default() })
        .add_systems(PostUpdate, (prepare_view, update_dynamic_water_surfaces, apply_view).chain());
    let camera = app.world_mut().spawn((Camera3d::default(), PrimarySceneView,
        GlobalTransform::from_translation(Vec3::new(16.0, 23.0, 16.0)))).id();
    let material = app.world_mut().resource_mut::<Assets<ClipmapWaterMaterial>>()
        .add(test_clipmap_water_material());
    let parent = app.world_mut().spawn((Transform::IDENTITY, Visibility::Inherited)).id();
    let water = app.world_mut().spawn((surface, interaction, DynamicWaterCpuInteraction,
        MeshMaterial3d(material), Visibility::Inherited, InheritedVisibility::VISIBLE, ChildOf(parent))).id();
    app.update();
    assert!(app.world().resource::<UnderwaterViewState>().sample.is_some(), "真实Clipmap足迹须先证明相机确实在水中");
    assert!(app.world().get::<DistanceFog>(camera).is_some());

    app.world_mut().entity_mut(parent).insert(Visibility::Hidden);
    // 使用正式传播完成后的组件状态；本夹具只运行真实水更新链，不安装整套渲染插件。
    app.world_mut().entity_mut(water).insert(InheritedVisibility::HIDDEN);
    app.update();
    assert_eq!(app.world().get::<Visibility>(water), Some(&Visibility::Inherited));
    assert!(app.world().resource::<UnderwaterViewState>().sample.is_none());
    assert!(app.world().get::<DistanceFog>(camera).is_none());
    assert_eq!(app.world().get::<ColorGrading>(camera).unwrap().global.temperature, 0.0);

    app.world_mut().entity_mut(parent).insert(Visibility::Inherited);
    app.world_mut().entity_mut(water).insert(InheritedVisibility::VISIBLE);
    app.update();
    assert!(app.world().resource::<UnderwaterViewState>().sample.is_some());
    assert!(app.world().get::<DistanceFog>(camera).is_some());
}
