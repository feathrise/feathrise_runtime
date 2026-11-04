//! 通用加载事务：应用提交关键清单后才允许跨状态，场景内真实启动门仍由scene领域完成。

use bevy::prelude::*;
use feathrise_scene_core::state::{
    ScenePhase, LoadingProgress, LoadingStage, PendingSceneTransition, SceneStartupGate,
};

use super::{CriticalSceneAssets, SceneLoadingPrerequisites, has_failed_asset};

pub(super) fn begin_scene_loading(
    mut commands: Commands,
    pending: Option<Res<PendingSceneTransition>>,
) {
    commands.insert_resource(
        pending
            .as_deref()
            .map(LoadingProgress::for_transition)
            .unwrap_or_default(),
    );
    commands.remove_resource::<CriticalSceneAssets>();
    commands.remove_resource::<SceneStartupGate>();
    commands.insert_resource(SceneLoadingPrerequisites::default());
}

pub(super) fn clear_scene_loading(mut commands: Commands) {
    commands.remove_resource::<LoadingProgress>();
    commands.remove_resource::<CriticalSceneAssets>();
    commands.remove_resource::<SceneStartupGate>();
    commands.remove_resource::<PendingSceneTransition>();
}

/// 地图专用入口明确不安装应用角色域；空关键清单仍需后续真实地图/碰撞启动门。
pub(super) fn prepare_project_scene_assets(
    mut commands: Commands,
    pending: Option<Res<PendingSceneTransition>>,
    critical: Option<Res<CriticalSceneAssets>>,
    mut prerequisites: ResMut<SceneLoadingPrerequisites>,
) {
    prerequisites.block_application();
    let Some(pending) = pending else { return; };
    if critical.as_deref().is_none_or(|assets| !assets.is_for(&pending)) {
        commands.insert_resource(CriticalSceneAssets::for_transition(&pending, Vec::new()));
    }
    prerequisites.allow_transition(&pending);
}

pub(super) fn poll_scene_loading(
    mut commands: Commands,
    pending: Option<Res<PendingSceneTransition>>,
    critical: Option<Res<CriticalSceneAssets>>,
    prerequisites: Res<SceneLoadingPrerequisites>,
    assets: Res<AssetServer>,
    mut progress: Option<ResMut<LoadingProgress>>,
    mut next: ResMut<NextState<ScenePhase>>,
) {
    let Some(transition) = pending else { return; };
    if let Some(progress) = progress.as_deref_mut()
        && progress.transaction() != transition.transaction()
    {
        progress.remember_transition(&transition); // 加载期间替换请求也必须换代，不能领取上一请求的暖回执。
    }
    if !prerequisites.ready_for(&transition) { return; }
    let Some(critical) = critical.filter(|assets| assets.is_for(&transition)) else {
        return; // 同为Loading时也必须等当前请求清单，上一请求已就绪不能放行后来的地图。
    };
    let target = transition.target;
    let name = progress
        .as_deref()
        .map(|value| value.target_display_name(target))
        .unwrap_or_else(|| transition.display_name())
        .to_owned();
    let total = critical.handles.len(); // 有效空清单必须通过；不能将进度条的占位分母作为加载门。
    let completed = critical
        .handles
        .iter()
        .filter(|handle| assets.is_loaded_with_dependencies(*handle))
        .count();
    let (stage, label) = if has_failed_asset(&assets, &critical.handles) {
        (
            LoadingStage::Failed,
            format!("{name}关键资源加载失败，请查看日志。"),
        )
    } else if completed < total {
        (
            LoadingStage::CriticalAssets,
            format!("正在预加载{name}关键资源..."),
        )
    } else if prerequisites.retirement_pending {
        (
            LoadingStage::CriticalAssets,
            "正在等待上一场景的应用资源退出...".to_owned(),
        )
    } else {
        commands.insert_resource(SceneStartupGate::waiting_for_collision(target));
        commands.remove_resource::<PendingSceneTransition>();
        next.set(target);
        (LoadingStage::SceneInit, format!("正在初始化{name}场景..."))
    };
    if let Some(progress) = progress.as_deref_mut() {
        progress.stage = stage;
        progress.completed = if stage == LoadingStage::SceneInit {
            0
        } else {
            completed
        };
        progress.total = if stage == LoadingStage::SceneInit {
            1
        } else {
            total.max(1)
        };
        progress.label = label;
    }
}

pub(super) fn finish_scene_loading(
    mut commands: Commands,
    progress: Option<Res<LoadingProgress>>,
    gate: Option<Res<SceneStartupGate>>,
    scene: Option<Res<State<ScenePhase>>>,
    residency: Option<Res<feathrise_scene_core::state::SceneResidencyActivity>>,
) {
    // 运行生命周期和应用遮罩共用同一冷／暖证明，不能把进度文本当成真实就绪。
    if progress.as_deref().zip(scene.as_deref()).is_some_and(|(progress, scene)| {
        progress.completion_ready(*scene.get(), gate.as_deref(), residency.as_deref())
    })
    {
        commands.remove_resource::<LoadingProgress>();
        commands.remove_resource::<CriticalSceneAssets>();
        commands.remove_resource::<SceneStartupGate>();
    }
}
