//! 唯一玩法/F12透视相机的水下外观接管与恢复；菜单、小地图和预设样品不参与。
use bevy::{prelude::*, render::view::ColorGrading};
use feathrise_atmosphere::weather::{WeatherAuthoringPreview, WeatherPreviewCamera};
use feathrise_scene_core::view::PrimarySceneView;
use feathrise_map_content::ActiveDynamicMap;
use feathrise_scene_core::state::ScenePhase;
use super::{components::UnderwaterCameraOverride, resources::UnderwaterViewState, types::{profile, same_fog}};

pub(in super::super) fn prepare_view(
    scene: Option<Res<State<ScenePhase>>>,
    active: Option<Res<ActiveDynamicMap>>,
    preview: Option<Res<WeatherAuthoringPreview>>,
    cameras: Query<(Entity, &Camera, &Projection, &GlobalTransform, Has<PrimarySceneView>, Has<WeatherPreviewCamera>), With<Camera3d>>,
    mut state: ResMut<UnderwaterViewState>,
) {
    state.camera = None;
    state.position = None;
    state.sample = None;
    state.floor_y = None;
    state.ceiling_y = None;
    if scene.as_deref().is_none_or(|scene| **scene != ScenePhase::Active)
        || active.as_deref().is_none_or(|active| active.map_id.is_none()) {
        return;
    }
    let preview = preview.as_deref().is_some_and(WeatherAuthoringPreview::active);
    for (entity, camera, projection, transform, game, author) in &cameras {
        if !camera.is_active || !matches!(projection, Projection::Perspective(_))
            || !(if preview { author } else { game && !author }) {
            continue;
        }
        if state.camera.is_some() {
            state.camera = None; // 重复主视图不得随查询顺序任意选一台。
            state.position = None;
            return;
        }
        let point = transform.translation();
        if point.is_finite() {
            state.camera = Some(entity);
            state.position = Some(point);
        }
    }
}

pub(in super::super) fn apply_view(
    mut commands: Commands,
    state: Res<UnderwaterViewState>,
    sky: Option<Res<ClearColor>>,
    mut cameras: Query<(Entity, Option<&mut ColorGrading>, Option<&mut DistanceFog>, Option<&mut UnderwaterCameraOverride>), With<Camera3d>>,
) {
    for (entity, mut grading_component, mut fog, owned) in &mut cameras {
        let sample = state.sample.filter(|sample| state.camera == Some(entity) && sample.depth > 0.03);
        // Camera3d并不保证存在ColorGrading；缺省相机也要获得水下雾效和色调。
        // 首次入水才补等价的默认组件，出水恢复其默认值，不改变曝光或其他色彩参数。
        let missing_grading = grading_component.is_none();
        let mut fallback_grading = ColorGrading::default();
        let grading = grading_component.as_deref_mut().unwrap_or(&mut fallback_grading);
        let balance = Vec2::new(grading.global.temperature, grading.global.tint);
        let Some(sample) = sample else {
            if let Some(owned) = owned {
                // 天气或外部工具已更新的值拥有更晚的权威；只恢复仍是本功能写入的字段。
                if balance.x == owned.balance_applied.x {
                    grading.global.temperature = owned.balance_before.x;
                }
                if balance.y == owned.balance_applied.y {
                    grading.global.tint = owned.balance_before.y;
                }
                if fog.as_deref().is_some_and(|fog| same_fog(fog, &owned.fog_applied)) {
                    match &owned.fog_before {
                        Some(before) => { **fog.as_mut().unwrap() = before.clone(); }
                        None => { commands.entity(entity).remove::<DistanceFog>(); }
                    }
                }
                commands.entity(entity).remove::<UnderwaterCameraOverride>();
            }
            continue;
        };
        let (desired, offset) = profile(sample, sky.as_deref().map_or(Color::BLACK, |sky| sky.0));
        let mut ownership = owned.map_or_else(|| UnderwaterCameraOverride {
            fog_before: fog.as_deref().cloned(), fog_applied: desired.clone(),
            balance_before: balance, balance_applied: balance,
        }, |mut owned| {
            // 两个白平衡字段各自保留所有权，外部只改色偏时不能让旧水下色温成为新基线。
            if balance.x != owned.balance_applied.x { owned.balance_before.x = balance.x; }
            if balance.y != owned.balance_applied.y { owned.balance_before.y = balance.y; }
            if fog.as_deref().is_none_or(|fog| !same_fog(fog, &owned.fog_applied)) {
                owned.fog_before = fog.as_deref().cloned();
            }
            UnderwaterCameraOverride { fog_before: owned.fog_before.clone(), fog_applied: owned.fog_applied.clone(),
                balance_before: owned.balance_before, balance_applied: owned.balance_applied }
        });
        let desired_balance = ownership.balance_before + offset;
        if balance != desired_balance {
            grading.global.temperature = desired_balance.x;
            grading.global.tint = desired_balance.y;
        }
        if let Some(fog) = fog.as_deref_mut() {
            if !same_fog(fog, &desired) { *fog = desired.clone(); }
        } else { commands.entity(entity).insert(desired.clone()); }
        ownership.balance_applied = desired_balance;
        ownership.fog_applied = desired;
        if missing_grading { commands.entity(entity).insert(fallback_grading); }
        commands.entity(entity).insert(ownership);
    }
}
