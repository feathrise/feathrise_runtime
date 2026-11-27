//! 按活动环境方案采用时间和天气配置；仅修改发生变化的字段，保持正常推进与读档状态。

use crate::{WeatherDebugOverride, WorldClock};
use bevy::prelude::*;
use feathrise_scene_content::{ActiveEnvironmentSimulation, EnvironmentSimulation};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ApplyEnvironmentSimulation;

pub fn apply_environment_simulation(
    active: Res<ActiveEnvironmentSimulation>,
    mut previous: Local<Option<(u64, Option<EnvironmentSimulation>)>>,
    mut clock: ResMut<WorldClock>,
    mut weather: Option<ResMut<WeatherDebugOverride>>,
) {
    let identity = active.identity_revision();
    let Some(configuration) = active.configuration else {
        if previous.as_ref().is_some_and(|(_, value)| value.is_some())
            && let Some(weather) = weather.as_deref_mut()
        {
            weather.clear(); // 归还旧场景或菜单时释放此前方案的强制天气。
        }
        *previous = Some((identity, None));
        return;
    };
    if configuration.validate().is_err() {
        return;
    }
    let before = previous
        .as_ref()
        .filter(|(revision, _)| *revision == identity)
        .and_then(|(_, value)| *value);
    if before.is_none_or(|old| (old.month, old.day) != (configuration.month, configuration.day)) {
        clock.set_date(configuration.month, configuration.day);
    }
    if before.is_none_or(|old| old.hour != configuration.hour) {
        clock.set_hour(configuration.hour);
    }
    // 天空、暂停、风摆或未引用方案的修订不能把正在推进/读档后的时钟拉回初始时间。
    if before.is_none_or(|old| {
        (old.weather_mode_index, old.weather_transition)
            != (
                configuration.weather_mode_index,
                configuration.weather_transition,
            )
    }) && let Some(weather) = weather.as_deref_mut()
    {
        weather.set_mode_index(usize::from(configuration.weather_mode_index));
        if configuration.weather_mode_index != 0 || configuration.weather_transition < 1.0 {
            weather.set_preview(configuration.weather_transition);
        } else {
            weather.clear(); // 自动模式的完整进度使用自然状态机，保留其运行和存档连续性。
        }
    }
    *previous = Some((identity, Some(configuration)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use feathrise_scene_content::{SceneEnvironmentSource, SceneId};

    #[test]
    fn configuration_changes_are_local_and_source_switches_restore_initial_time() {
        let mut app = App::new();
        app.init_resource::<ActiveEnvironmentSimulation>()
            .init_resource::<WorldClock>()
            .init_resource::<WeatherDebugOverride>()
            .add_systems(Update, apply_environment_simulation);
        let scene = SceneId::parse("SCN-simulation").unwrap();
        let first = EnvironmentSimulation {
            hour: 6.0,
            month: 9,
            weather_mode_index: 3,
            ..default()
        };
        app.world_mut()
            .resource_mut::<ActiveEnvironmentSimulation>()
            .publish(
                scene.clone(),
                SceneEnvironmentSource::Shared("morning".into()),
                Some(first),
            );
        app.update();
        assert_eq!(app.world().resource::<WorldClock>().hour_for_display(), 6.0);
        assert_eq!(
            app.world().resource::<WeatherDebugOverride>().mode_index(),
            3
        );
        app.world_mut().resource_mut::<WorldClock>().set_hour(13.0); // 模拟已推进或读档恢复的时刻。
        let changed = EnvironmentSimulation {
            paused: true,
            grass_wind_tip_strength: 0.3,
            weather_transition: 0.5,
            ..first
        };
        app.world_mut()
            .resource_mut::<ActiveEnvironmentSimulation>()
            .publish(
                scene.clone(),
                SceneEnvironmentSource::Shared("morning".into()),
                Some(changed),
            );
        app.update();
        assert_eq!(
            app.world().resource::<WorldClock>().hour_for_display(),
            13.0
        );
        assert_eq!(
            app.world().resource::<WeatherDebugOverride>().preview(),
            0.5
        );
        app.update();
        assert_eq!(
            app.world().resource::<WorldClock>().hour_for_display(),
            13.0
        );
        app.world_mut()
            .resource_mut::<ActiveEnvironmentSimulation>()
            .publish(
                scene.clone(),
                SceneEnvironmentSource::Shared("other".into()),
                Some(changed),
            );
        app.update();
        assert_eq!(app.world().resource::<WorldClock>().hour_for_display(), 6.0);
        app.world_mut()
            .resource_mut::<ActiveEnvironmentSimulation>()
            .publish(scene, SceneEnvironmentSource::Legacy, None);
        app.update();
        assert_eq!(
            app.world().resource::<WeatherDebugOverride>().mode_index(),
            0
        );
        assert_eq!(app.world().resource::<WorldClock>().hour_for_display(), 6.0);
        app.world_mut()
            .resource_mut::<ActiveEnvironmentSimulation>()
            .clear();
        app.update();
        assert_eq!(
            app.world()
                .resource::<ActiveEnvironmentSimulation>()
                .configuration,
            None
        );
    }
}
