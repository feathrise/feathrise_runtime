//! 水下采样、相机隔离及出水恢复回归；不依赖水面AABB猜测整片水域。
use super::{components::UnderwaterCameraOverride, resources::UnderwaterViewState, systems::*, types::*};
use bevy::{prelude::*, render::view::ColorGrading};
use feathrise_atmosphere::weather::{WeatherAuthoringPreview, WeatherPreviewCamera};
use feathrise_scene_core::view::PrimarySceneView;
use feathrise_scene_content::{ActiveDynamicScene, SceneId};
use feathrise_scene_core::state::ScenePhase;

#[test]
fn immersion_rejects_air_and_below_bed_and_uses_deepest_overlapping_water() {
    let mut state = UnderwaterViewState::default();
    for (depth, bottom) in [(-1.0, 10.0), (0.0, 10.0), (3.0, 2.0), (2.0, 2.0), (f32::NAN, 10.0), (8.0, f32::INFINITY)] {
        state.observe(depth, bottom, Color::WHITE);
        assert!(state.sample.is_none());
    }
    state.observe(1.0, 3.0, Color::WHITE);
    state.observe(0.5, 3.0, Color::BLACK);
    assert_eq!(state.sample.unwrap().depth, 1.0);
    state.observe(8.0, 10.0, Color::WHITE); // 只有明确有限水深才能授权更深的介质。
    assert_eq!(state.sample.unwrap().depth, 8.0);
}

#[test]
fn immersion_profile_is_continuous_and_preserves_night_brightness() {
    let water = Color::srgb(0.05, 0.24, 0.29);
    let sample = WaterViewSample { depth: 2.0, color: water };
    let (day, balance) = profile(sample, Color::WHITE);
    let (night, _) = profile(sample, Color::BLACK);
    assert!(balance.x < -0.02 && balance.y < 0.0);
    assert!(day.color.alpha() > 0.9);
    assert_eq!(night.color.to_linear().red, 0.0);
    assert_eq!(night.color.to_linear().green, 0.0);
    let (shore, shift) = profile(WaterViewSample { depth: 0.03, color: water }, Color::WHITE);
    assert_eq!(shore.color.alpha(), 0.0);
    assert_eq!(shift, Vec2::ZERO);
}

fn app() -> App {
    let mut app = App::new();
    app.init_resource::<UnderwaterViewState>();
    app.insert_resource(ClearColor(Color::WHITE));
    app
}

#[test]
fn underwater_override_restores_authored_weather_and_never_changes_exposure() {
    let mut app = app();
    app.add_systems(Update, apply_view);
    let mut grading = ColorGrading::default();
    grading.global.exposure = 1.25;
    grading.global.temperature = 0.01;
    let camera = app.world_mut().spawn((Camera3d::default(), grading,
        DistanceFog { color: Color::srgb(0.4, 0.5, 0.6), ..default() })).id();
    {
        let mut state = app.world_mut().resource_mut::<UnderwaterViewState>();
        state.camera = Some(camera);
        state.observe(2.0, 5.0, Color::srgb(0.05, 0.24, 0.29));
    }
    app.update();
    let first = app.world().get::<ColorGrading>(camera).unwrap().global.temperature;
    assert!(first < 0.0);
    app.update(); // 连续水下帧不得累积色温偏移。
    assert_eq!(app.world().get::<ColorGrading>(camera).unwrap().global.temperature, first);
    assert_eq!(app.world().get::<ColorGrading>(camera).unwrap().global.exposure, 1.25);
    app.world_mut().resource_mut::<UnderwaterViewState>().sample = None;
    app.update();
    assert_eq!(app.world().get::<ColorGrading>(camera).unwrap().global.temperature, 0.01);
    assert_eq!(app.world().get::<DistanceFog>(camera).unwrap().color, Color::srgb(0.4, 0.5, 0.6));
    assert!(app.world().get::<UnderwaterCameraOverride>(camera).is_none());
}

#[test]
fn external_weather_change_is_preserved_on_exit_and_owned_fog_is_removed() {
    let mut app = app();
    app.add_systems(Update, apply_view);
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    {
        let mut state = app.world_mut().resource_mut::<UnderwaterViewState>();
        state.camera = Some(camera);
        state.observe(2.0, 5.0, Color::WHITE);
    }
    app.update();
    assert!(app.world().get::<DistanceFog>(camera).is_some());
    app.world_mut().resource_mut::<UnderwaterViewState>().sample = None;
    app.update();
    assert!(app.world().get::<DistanceFog>(camera).is_none());
    app.world_mut().resource_mut::<UnderwaterViewState>().observe(2.0, 5.0, Color::WHITE);
    app.update();
    app.world_mut().entity_mut(camera).insert(DistanceFog { color: Color::BLACK, ..default() });
    app.world_mut().resource_mut::<UnderwaterViewState>().sample = None;
    app.update();
    assert_eq!(app.world().get::<DistanceFog>(camera).unwrap().color, Color::BLACK);
}

#[test]
fn view_selection_isolates_auxiliary_orthographic_and_inactive_cameras() {
    let mut app = app();
    app.insert_resource(State::new(ScenePhase::Active));
    app.insert_resource(ActiveDynamicScene { scene_id: Some(SceneId::parse("MAP-underwater-view").unwrap()), ..default() });
    app.init_resource::<WeatherAuthoringPreview>();
    app.add_systems(Update, prepare_view);
    let gameplay = app.world_mut().spawn((Camera3d::default(), PrimarySceneView)).id();
    let preview = app.world_mut().spawn((Camera3d::default(), WeatherPreviewCamera)).id();
    let _auxiliary = app.world_mut().spawn(Camera3d::default()).id();
    app.update();
    assert_eq!(app.world().resource::<UnderwaterViewState>().camera, Some(gameplay));
    *app.world_mut().resource_mut::<WeatherAuthoringPreview>() = WeatherAuthoringPreview::new(true);
    app.update();
    assert_eq!(app.world().resource::<UnderwaterViewState>().camera, Some(preview));
    app.world_mut().entity_mut(preview).insert(Projection::Orthographic(OrthographicProjection::default_3d()));
    app.update();
    assert!(app.world().resource::<UnderwaterViewState>().camera.is_none());
}

#[test]
fn external_single_axis_balance_change_does_not_accumulate_the_other_axis() {
    let mut app = app();
    app.add_systems(Update, apply_view);
    let camera = app.world_mut().spawn(Camera3d::default()).id();
    {
        let mut state = app.world_mut().resource_mut::<UnderwaterViewState>();
        state.camera = Some(camera);
        state.observe(2.0, 5.0, Color::WHITE);
    }
    app.update();
    let initial_temperature = app.world().get::<ColorGrading>(camera).unwrap().global.temperature;
    app.world_mut().get_mut::<ColorGrading>(camera).unwrap().global.tint = 0.1;
    app.update();
    assert_eq!(app.world().get::<ColorGrading>(camera).unwrap().global.temperature, initial_temperature);
    app.world_mut().resource_mut::<UnderwaterViewState>().sample = None;
    app.update();
    let balance = &app.world().get::<ColorGrading>(camera).unwrap().global;
    assert_eq!(balance.temperature, 0.0);
    assert_eq!(balance.tint, 0.1);
}
