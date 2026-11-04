// 游戏设置 — 全局阴影资源与灯光应用系统。

use bevy::{
    light::{
        CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap,
        PointLightShadowMap,
    }, // 方向光级联与全局阴影贴图资源。
    prelude::*, // ECS 与三类实时灯光。
};

use crate::{
    RenderSettings, GraphicsRuntimeLeaseRegistry, ShadowDistanceQuality, ShadowQuality,
    components::AuthoredShadowMapsEnabled,
}; // 画质模型与灯光作者状态备份。

/// 调整全局阴影贴图，并在关闭阴影期间保存每盏灯的作者资格。
#[allow(clippy::too_many_arguments)] // Bevy 系统参数分别代表全局贴图与三类灯光查询，保持分离才能避免互斥借用冲突。
pub(crate) fn apply_shadow_quality(
    mut commands: Commands,      // 新灯光首次出现时附加作者资格标记。
    settings: Res<RenderSettings>, // 当前画质设置。
    runtime_leases: Option<Res<GraphicsRuntimeLeaseRegistry>>, // 独立系统测试未装完整插件时等价于无租约。
    mut directional_shadow_map: ResMut<DirectionalLightShadowMap>, // 方向光与聚光灯共享分辨率。
    mut point_shadow_map: ResMut<PointLightShadowMap>,         // 点光六面体阴影分辨率。
    mut directional_lights: Query<(
        Entity,
        &mut DirectionalLight,
        Option<&AuthoredShadowMapsEnabled>,
    )>, // 方向光作者状态。
    mut point_lights: Query<(Entity, &mut PointLight, Option<&AuthoredShadowMapsEnabled>)>, // 点光作者状态。
    mut spot_lights: Query<(Entity, &mut SpotLight, Option<&AuthoredShadowMapsEnabled>)>, // 聚光灯作者状态。
) {
    let quality = settings.graphics.shadow_quality; // 本帧所有光源使用同一贴图质量。
    let shadows_enabled = quality != ShadowQuality::Off
        && !runtime_leases
            .as_deref()
            .is_some_and(GraphicsRuntimeLeaseRegistry::standard_shadows_suspended); // 租约不改玩家档位，释放帧立即按最新档恢复。
    let (directional_size, point_size) = quality.shadow_map_sizes(); // 获取规范二次幂尺寸。
    if directional_shadow_map.size != directional_size {
        directional_shadow_map.size = directional_size; // 同时应用方向光与聚光灯尺寸。
    }
    if point_shadow_map.size != point_size {
        point_shadow_map.size = point_size; // 应用点光六面阴影尺寸。
    }

    for (entity, mut light, authored) in &mut directional_lights {
        let desired = desired_light_shadow_state(
            &mut commands,
            entity,
            light.shadow_maps_enabled,
            authored,
            shadows_enabled,
        ); // 新旧方向光统一继承当前状态。
        if light.shadow_maps_enabled != desired {
            light.shadow_maps_enabled = desired; // 仅在实际变化时修改方向光。
        }
    }
    for (entity, mut light, authored) in &mut point_lights {
        let desired = desired_light_shadow_state(
            &mut commands,
            entity,
            light.shadow_maps_enabled,
            authored,
            shadows_enabled,
        ); // 新旧点光统一继承当前状态。
        if light.shadow_maps_enabled != desired {
            light.shadow_maps_enabled = desired; // 仅在实际变化时修改点光。
        }
    }
    for (entity, mut light, authored) in &mut spot_lights {
        let desired = desired_light_shadow_state(
            &mut commands,
            entity,
            light.shadow_maps_enabled,
            authored,
            shadows_enabled,
        ); // 新旧聚光灯统一继承当前状态。
        if light.shadow_maps_enabled != desired {
            light.shadow_maps_enabled = desired; // 仅在实际变化时修改聚光灯。
        }
    }
}

/// 将方向光阴影距离与级联数应用到所有现有和后续生成的方向光。
pub(crate) fn apply_shadow_distance(
    settings: Res<RenderSettings>, // 当前画质设置。
    mut directional_lights: Query<&mut CascadeShadowConfig, With<DirectionalLight>>, // 所有方向光都由配置组件承载级联参数。
) {
    let desired = cascade_config(settings.graphics.shadow_distance); // 每帧只构造一次固定小型配置。
    for mut current in &mut directional_lights {
        if !cascade_config_matches(&current, &desired) {
            *current = desired.clone(); // 档位变化或新方向光出现时才重建级联配置。
        }
    }
}

/// 共享三类灯光的作者状态捕获与目标值推导逻辑。
fn desired_light_shadow_state(
    commands: &mut Commands,                      // 用于首次附加作者状态。
    entity: Entity,                               // 当前灯光实体。
    current: bool,                                // 当前实时阴影开关。
    authored: Option<&AuthoredShadowMapsEnabled>, // 已捕获的原始作者资格。
    shadows_enabled: bool,                        // 当前全局阴影是否开启。
) -> bool {
    let authored_enabled = authored.map_or(current, |value| value.0); // 新灯光先记录覆盖前的作者值。
    if authored.is_none() {
        commands
            .entity(entity)
            .insert(AuthoredShadowMapsEnabled(authored_enabled)); // 每盏灯只捕获一次原始资格。
    }
    shadows_enabled && authored_enabled // 重开时只恢复作者原本允许投影的灯光。
}

/// 使用项目固定的近界、首级联边界与重叠比例构建质量档配置。
fn cascade_config(quality: ShadowDistanceQuality) -> CascadeShadowConfig {
    let (num_cascades, maximum_distance) = quality.cascade_parameters(); // 读取公开档位参数。
    CascadeShadowConfigBuilder {
        num_cascades,                  // 低中档减少级联绘制次数。
        minimum_distance: 0.1,         // 保持 Bevy 0.19.1 默认近界。
        maximum_distance,              // 应用 50/100/150/300 米范围。
        first_cascade_far_bound: 10.0, // 保持近景阴影密度基线。
        overlap_proportion: 0.2,       // 保持 Bevy 默认级联过渡比例。
    }
    .build()
}

/// 比较完整级联配置，避免稳定帧重复标记组件并重算阴影视锥。
fn cascade_config_matches(current: &CascadeShadowConfig, desired: &CascadeShadowConfig) -> bool {
    current.bounds == desired.bounds
        && current.overlap_proportion == desired.overlap_proportion
        && current.minimum_distance == desired.minimum_distance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<RenderSettings>();
        app.init_resource::<DirectionalLightShadowMap>();
        app.init_resource::<PointLightShadowMap>();
        app.add_systems(Update, (apply_shadow_quality, apply_shadow_distance));
        app
    }

    #[test]
    fn shadow_off_restores_authored_state_and_tracks_new_lights() {
        let mut app = test_app();
        let authored_on = app
            .world_mut()
            .spawn(DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            })
            .id();
        let authored_off = app
            .world_mut()
            .spawn(PointLight {
                shadow_maps_enabled: false,
                ..default()
            })
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_quality = ShadowQuality::Off;
        app.update();
        assert!(
            !app.world()
                .get::<DirectionalLight>(authored_on)
                .unwrap()
                .shadow_maps_enabled
        );
        let added_while_off = app
            .world_mut()
            .spawn(SpotLight {
                shadow_maps_enabled: true,
                ..default()
            })
            .id();
        app.update();
        assert!(
            !app.world()
                .get::<SpotLight>(added_while_off)
                .unwrap()
                .shadow_maps_enabled
        );
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_quality = ShadowQuality::Ultra;
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(authored_on)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(
            !app.world()
                .get::<PointLight>(authored_off)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(
            app.world()
                .get::<SpotLight>(added_while_off)
                .unwrap()
                .shadow_maps_enabled
        );
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            4096
        );
        assert_eq!(app.world().resource::<PointLightShadowMap>().size, 2048);
    }

    #[test]
    fn shadow_distance_updates_cascades_and_new_directional_lights() {
        let mut app = test_app();
        let first = app.world_mut().spawn(DirectionalLight::default()).id();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_distance = ShadowDistanceQuality::Low;
        app.update();
        let first_config = app.world().get::<CascadeShadowConfig>(first).unwrap();
        assert_eq!(first_config.bounds.len(), 2);
        assert!((first_config.bounds.last().unwrap() - 50.0).abs() < 0.001);
        let added_later = app.world_mut().spawn(DirectionalLight::default()).id();
        app.update();
        let added_config = app.world().get::<CascadeShadowConfig>(added_later).unwrap();
        assert_eq!(added_config.bounds.len(), 2);
        assert!((added_config.bounds.last().unwrap() - 50.0).abs() < 0.001);
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_distance = ShadowDistanceQuality::Medium;
        app.update();
        let medium_config = app.world().get::<CascadeShadowConfig>(first).unwrap();
        assert_eq!(medium_config.bounds.len(), 3);
        assert!((medium_config.bounds.last().unwrap() - 100.0).abs() < 0.001);
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_distance = ShadowDistanceQuality::High;
        app.update();
        let high_config = app.world().get::<CascadeShadowConfig>(first).unwrap();
        assert_eq!(high_config.bounds.len(), 4);
        assert!((high_config.bounds.last().unwrap() - 150.0).abs() < 0.001);
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_distance = ShadowDistanceQuality::Ultra;
        app.update();
        let ultra_config = app.world().get::<CascadeShadowConfig>(first).unwrap();
        assert_eq!(ultra_config.bounds.len(), 4);
        assert!((ultra_config.bounds.last().unwrap() - 300.0).abs() < 0.001);
    }

    #[test]
    fn stable_shadow_frame_does_not_mark_light_or_cascade_changed_again() {
        let mut app = test_app();
        let light = app
            .world_mut()
            .spawn(DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            })
            .id();
        app.update();
        app.world_mut().clear_trackers();
        app.update();
        assert!(
            !app.world()
                .entity(light)
                .get_ref::<DirectionalLight>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记灯光组件变更",
        );
        assert!(
            !app.world()
                .entity(light)
                .get_ref::<CascadeShadowConfig>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记级联配置变更",
        );
    }
}
