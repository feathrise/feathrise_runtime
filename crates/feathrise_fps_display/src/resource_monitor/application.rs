//! 应用诊断只在F3总览刷新时提供文字；共享面板不认识角色、阵营或技能类型。

use bevy::prelude::*;
use crate::resources::{ResourceMonitorPage, ResourceMonitorRefreshTimer, ResourceMonitorState};

/// 应用采样必须位于共享显隐/计时更新之后、正文发布之前，避免晚一轮或每帧扫描。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceMonitorSet { Prepare, Collect, Present }

/// 由当前应用提供的总览附加行；没有适配器的编辑器和空工程保持空白。
#[derive(Resource, Default)]
pub struct ResourceMonitorApplicationReport(pub String);

#[derive(Resource, Default)]
pub struct ResourceMonitorRefresh {
    pub(super) due: bool,
    overview: bool,
}

pub(super) fn prepare_refresh(
    time: Res<Time>,
    mut timer: ResMut<ResourceMonitorRefreshTimer>,
    mut state: ResMut<ResourceMonitorState>,
    mut refresh: ResMut<ResourceMonitorRefresh>,
    mut report: ResMut<ResourceMonitorApplicationReport>,
) {
    timer.0.tick(time.delta());
    refresh.due = state.visible && (timer.0.just_finished() || state.force_refresh);
    refresh.overview = refresh.due && state.page == ResourceMonitorPage::Overview;
    if refresh.due {
        state.force_refresh = false;
        // 每轮先清空，应用卸载或不再提供采样时不能沿用旧游戏统计。
        report.0.clear();
    }
}

/// 游戏采样系统的run_if；隐藏、其他分页和非半秒刷新帧都不访问游戏查询。
pub fn resource_monitor_overview_refresh(refresh: Res<ResourceMonitorRefresh>) -> bool {
    refresh.overview
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{ResourceMonitorGroupButton, ResourceMonitorPageText};
    use crate::resource_monitor::{clipmap::ClipmapMonitorState, systems};
    use crate::{LodControlContent, LodControlPresetBindings};
    use crate::resources::ResourceMonitorGroup;
    use bevy::diagnostic::DiagnosticsStore;
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct ApplicationSamples(usize);

    fn sample_application(
        mut samples: ResMut<ApplicationSamples>,
        mut report: ResMut<ResourceMonitorApplicationReport>,
    ) {
        samples.0 += 1;
        report.0 = format!("应用采样 {}\n", samples.0);
    }

    /// 直接运行正式输入、刷新门与正文发布；手动推进时间排除墙钟抖动和GPU依赖。
    fn fixture(with_provider: bool) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ResourceMonitorState>()
            .init_resource::<ResourceMonitorRefresh>()
            .init_resource::<ResourceMonitorApplicationReport>()
            .init_resource::<ApplicationSamples>()
            .init_resource::<ClipmapMonitorState>()
            .init_resource::<DiagnosticsStore>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<AnimationClip>>()
            .insert_resource(ResourceMonitorRefreshTimer(Timer::from_seconds(
                0.5,
                TimerMode::Repeating,
            )))
            .configure_sets(
                Update,
                (ResourceMonitorSet::Prepare, ResourceMonitorSet::Collect, ResourceMonitorSet::Present).chain(),
            )
            .add_systems(
                Update,
                (
                    systems::toggle_resource_monitor_panel,
                    systems::select_resource_monitor_page,
                    prepare_refresh,
                )
                    .chain()
                    .in_set(ResourceMonitorSet::Prepare),
            )
            .add_systems(
                Update,
                systems::update_resource_monitor_panel.in_set(ResourceMonitorSet::Present),
            );
        if with_provider {
            app.add_systems(
                Update,
                sample_application
                    .in_set(ResourceMonitorSet::Collect)
                    .run_if(resource_monitor_overview_refresh),
            );
        }
        let text = app.world_mut().spawn((
            Text::default(),
            ResourceMonitorPageText { page: ResourceMonitorPage::Overview },
        )).id();
        (app, text)
    }

    fn step(app: &mut App, milliseconds: u64) {
        app.world_mut().resource_mut::<Time>().advance_by(Duration::from_millis(milliseconds));
        app.update();
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
    }

    fn text(app: &App, entity: Entity) -> &str {
        &app.world().get::<Text>(entity).unwrap().0
    }

    #[test]
    fn hidden_and_non_overview_pages_never_sample_application_queries() {
        let (mut app, _) = fixture(true);
        // 本用例验证真实可选内容页的采样门；无绑定工程会按产品规则退回总览。
        let mut bindings = LodControlPresetBindings::default();
        for content in LodControlContent::ALL {
            bindings.bind(content, format!("test_{content:?}"));
        }
        app.insert_resource(bindings);
        // 即使遗留强刷标志或计时到期，隐藏状态也没有查询授权。
        app.world_mut().resource_mut::<ResourceMonitorState>().force_refresh = true;
        step(&mut app, 500);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 0);
        for page in ResourceMonitorPage::ALL.into_iter().filter(|page| *page != ResourceMonitorPage::Overview) {
            *app.world_mut().resource_mut::<ResourceMonitorState>() = ResourceMonitorState {
                visible: true,
                force_refresh: true,
                group: page.group(),
                page,
            };
            step(&mut app, 500);
            assert_eq!(app.world().resource::<ResourceMonitorState>().page, page);
        }
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 0);
        *app.world_mut().resource_mut::<ResourceMonitorState>() = ResourceMonitorState {
            visible: true,
            force_refresh: true,
            ..default()
        };
        step(&mut app, 1);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 1);
        app.world_mut().resource_mut::<ResourceMonitorState>().visible = false;
        step(&mut app, 500);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 1);
    }

    #[test]
    fn overview_samples_only_at_half_second_boundaries() {
        let (mut app, entity) = fixture(true);
        app.world_mut().resource_mut::<ResourceMonitorState>().visible = true;
        step(&mut app, 499);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 0);
        step(&mut app, 1);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 1);
        assert!(text(&app, entity).contains("应用采样 1"));
        step(&mut app, 1);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 1);
        step(&mut app, 499);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 2);
        assert!(text(&app, entity).contains("应用采样 2"));
    }

    #[test]
    fn opening_and_returning_to_overview_present_the_new_sample_in_the_same_frame() {
        let (mut app, entity) = fixture(true);
        let mut bindings = LodControlPresetBindings::default();
        bindings.bind(LodControlContent::OrdinaryGrass, "test_meadow");
        app.insert_resource(bindings);
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F3);
        step(&mut app, 1);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 1);
        assert!(text(&app, entity).contains("应用采样 1"));
        assert!(!app.world().resource::<ResourceMonitorState>().force_refresh);

        // 使用真实页签交互切出再切回；总时间只有3ms，不能依赖半秒自然到期。
        let button = app.world_mut().spawn((

            bevy::ui::FocusPolicy::Block,
            ResourceMonitorGroupButton { group: ResourceMonitorGroup::OrdinaryGrass },
            Interaction::Pressed,
            BackgroundColor::default(),
            BorderColor::default(),
            Node::default(), // 与正式按钮相同，页签同步需要更新其布局显隐。
        )).id();
        step(&mut app, 1);
        assert_eq!(app.world().resource::<ResourceMonitorState>().page, ResourceMonitorPage::OrdinaryGrass);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 1);
        assert!(app.world().resource::<ResourceMonitorApplicationReport>().0.is_empty());
        app.world_mut().get_mut::<ResourceMonitorGroupButton>(button).unwrap().group = ResourceMonitorGroup::Overview;
        step(&mut app, 1);
        assert_eq!(app.world().resource::<ApplicationSamples>().0, 2);
        assert!(text(&app, entity).contains("应用采样 2"));
        assert!(!text(&app, entity).contains("应用采样 1"));
    }

    #[test]
    fn overview_without_a_provider_clears_previous_application_text() {
        let (mut app, entity) = fixture(false);
        app.world_mut().resource_mut::<ResourceMonitorApplicationReport>().0 = "旧游戏诊断".into();
        app.world_mut().entity_mut(entity).insert(Text::new("旧游戏诊断"));
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F3);
        step(&mut app, 1);
        assert!(app.world().resource::<ResourceMonitorApplicationReport>().0.is_empty());
        assert!(!text(&app, entity).contains("旧游戏诊断"));
        assert!(!text(&app, entity).is_empty(), "通用总览仍须正常发布");
    }
}
