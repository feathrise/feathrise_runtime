//! 无窗口验证共享加载器不需要原游戏UI/PAK，并保留关键资产和真实场景启动门。

use super::*;
use feathrise_scene_core::state::{
    LoadingProgress, LoadingStage, PendingSceneTransition, SceneStartupGate,
};

#[derive(Asset, TypePath)]
struct ReadyAsset;

#[derive(Default, TypePath)]
struct ReadyAssetLoader;
impl bevy::asset::AssetLoader for ReadyAssetLoader {
    type Asset = ReadyAsset;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        _reader: &mut dyn bevy::asset::io::Reader,
        _settings: &Self::Settings,
        _context: &mut bevy::asset::LoadContext<'_>,
    ) -> Result<ReadyAsset, Self::Error> {
        Ok(ReadyAsset)
    }

    fn extensions(&self) -> &[&str] {
        &["loading-gate-test"]
    }
}

fn loading_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        bevy::state::app::StatesPlugin,
    ))
    .init_state::<ScenePhase>()
    .add_plugins(SceneLoadingPlugin)
    .init_asset::<ReadyAsset>()
    .register_asset_loader(ReadyAssetLoader);
    app.update(); // 初始首页清理已执行，后续只测试同一个明确加载事务。
    let transition = PendingSceneTransition::named(ScenePhase::Active, "作者场景");
    app.insert_resource(State::new(ScenePhase::Loading))
        .insert_resource(LoadingProgress::for_transition(&transition))
        .insert_resource(transition);
    app
}

fn submit_assets(app: &mut App, handles: Vec<bevy::asset::UntypedHandle>) {
    let request = app.world().resource::<PendingSceneTransition>();
    let assets = CriticalSceneAssets::for_transition(request, handles);
    app.insert_resource(assets);
}

fn allow_application(app: &mut App) {
    let request = app.world().resource::<PendingSceneTransition>().clone();
    app.world_mut().resource_mut::<SceneLoadingPrerequisites>().allow_transition(&request);
}

#[test]
fn a_submitted_empty_asset_list_still_waits_for_application_preconditions() {
    let mut app = loading_app();
    submit_assets(&mut app, vec![]);
    app.update();
    assert!(app.world().contains_resource::<PendingSceneTransition>());
    assert!(!app.world().contains_resource::<SceneStartupGate>());
    allow_application(&mut app);
    app.update();
    let gate = app.world().resource::<SceneStartupGate>();
    assert_eq!(gate.target, ScenePhase::Active);
    assert!(!gate.collision_ready);
    assert!(!gate.ready); // 切入目标状态只开始场景初始化，不能假装几何和应用领域已就绪。
    assert_eq!(
        app.world().resource::<LoadingProgress>().stage,
        LoadingStage::SceneInit
    );
    assert!(matches!(
        app.world().resource::<NextState<ScenePhase>>(),
        NextState::Pending(ScenePhase::Active)
    ));
}

#[test]
fn application_ready_does_not_skip_a_pending_critical_asset() {
    let mut app = loading_app();
    let queued = app
        .world_mut()
        .resource_mut::<Assets<ReadyAsset>>()
        .reserve_handle();
    submit_assets(&mut app, vec![queued.untyped()]);
    allow_application(&mut app);
    app.update();
    assert!(app.world().contains_resource::<PendingSceneTransition>());
    assert!(!app.world().contains_resource::<SceneStartupGate>());
    let progress = app.world().resource::<LoadingProgress>();
    assert_eq!(progress.stage, LoadingStage::CriticalAssets);
    assert_eq!((progress.completed, progress.total), (0, 1));
}

#[test]
fn ready_assets_wait_for_application_retirement_receipt() {
    let mut app = loading_app();
    let handle = app.world().resource::<AssetServer>().add(ReadyAsset);
    submit_assets(&mut app, vec![handle.untyped()]);
    allow_application(&mut app);
    app.world_mut().resource_mut::<SceneLoadingPrerequisites>().retirement_pending = true;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.update();
        assert!(!app.world().contains_resource::<SceneStartupGate>());
        if app.world().resource::<LoadingProgress>().label == "正在等待上一场景的应用资源退出..." {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "最小资产未及时就绪");
        std::thread::yield_now();
    }
    app.world_mut()
        .resource_mut::<SceneLoadingPrerequisites>()
        .retirement_pending = false;
    app.update();
    assert!(app.world().contains_resource::<SceneStartupGate>());
    assert!(!app.world().contains_resource::<PendingSceneTransition>());
}

#[test]
fn empty_scene_entry_requires_an_explicit_request_and_keeps_real_scene_gate_closed() {
    let mut app = loading_app();
    app.add_plugins(ProjectSceneAssetsPlugin);
    let transition = app.world_mut().remove_resource::<PendingSceneTransition>().unwrap();
    app.update();
    assert!(!app.world().contains_resource::<CriticalSceneAssets>());
    assert!(!app.world().resource::<SceneLoadingPrerequisites>().ready_for(&transition));
    assert!(!app.world().contains_resource::<SceneStartupGate>());
    app.insert_resource(transition);
    app.update();
    assert!(
        app.world()
            .resource::<CriticalSceneAssets>()
            .handles
            .is_empty()
    );
    assert!(app.world().contains_resource::<SceneStartupGate>());
    assert!(!app.world().resource::<SceneStartupGate>().ready);
    assert!(!app.world().resource::<SceneStartupGate>().collision_ready);
    assert_eq!(app.world().resource::<LoadingProgress>().stage, LoadingStage::SceneInit);
}

#[test]
fn failed_critical_asset_cannot_create_a_startup_gate_even_with_ready_application() {
    let mut app = loading_app();
    // 已注册加载器，失败来自真实AssetServer的缺失源文件，不伪造LoadState。
    let missing = app.world().resource::<AssetServer>()
        .load::<ReadyAsset>(format!("__missing_loading_gate_{}.loading-gate-test", std::process::id()));
    submit_assets(&mut app, vec![missing.untyped()]);
    allow_application(&mut app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.update();
        assert!(app.world().contains_resource::<PendingSceneTransition>());
        assert!(!app.world().contains_resource::<SceneStartupGate>());
        if app.world().resource::<LoadingProgress>().stage == LoadingStage::Failed { break; }
        assert!(std::time::Instant::now() < deadline, "真实缺失资产应完成失败回执");
        std::thread::yield_now();
    }
    assert!(app.world().resource::<LoadingProgress>().label.contains("关键资源加载失败"));
}

#[test]
fn scene_gates_survive_until_real_scene_completion_then_release_without_a_menu_overlay() {
    let mut app = loading_app();
    app.insert_resource(SceneStartupGate::waiting_for_collision(
        ScenePhase::Active,
    ))
    .insert_resource(CriticalSceneAssets::default());
    app.world_mut().resource_mut::<LoadingProgress>().stage = LoadingStage::SceneAssets;
    app.update();
    assert!(app.world().contains_resource::<SceneStartupGate>());
    app.world_mut().resource_mut::<LoadingProgress>().stage = LoadingStage::Complete;
    app.update();
    assert!(app.world().contains_resource::<SceneStartupGate>(), "进度标签不能越过真实场景完成门");
    app.world_mut().resource_mut::<SceneStartupGate>().ready = true;
    app.update();
    assert!(app.world().contains_resource::<SceneStartupGate>(), "旧目标完成不能在LoadingScene提前放行");
    app.insert_resource(State::new(ScenePhase::Active));
    app.update();
    assert!(!app.world().contains_resource::<SceneStartupGate>());
    assert!(!app.world().contains_resource::<LoadingProgress>());
    assert!(!app.world().contains_resource::<CriticalSceneAssets>());
}

#[test]
fn completed_label_without_a_gate_is_not_a_scene_completion_receipt() {
    let mut app = loading_app();
    app.world_mut().resource_mut::<LoadingProgress>().stage = LoadingStage::Complete;
    app.insert_resource(State::new(ScenePhase::Active));
    app.update();
    assert!(app.world().contains_resource::<LoadingProgress>());
}


fn restored_loading_app() -> App {
    use feathrise_scene_core::residency::{ResidentSceneToken, SceneResidencyActivity};
    let mut app = loading_app();
    app.world_mut().remove_resource::<PendingSceneTransition>();
    app.insert_resource(State::new(ScenePhase::Active))
        .insert_resource(CriticalSceneAssets::default())
        .insert_resource(SceneResidencyActivity {
            enabled: true,
            switching: false,
            active: Some(ResidentSceneToken(7)),
            restored: Some(ResidentSceneToken(7)),
        });
    let mut progress = app.world_mut().resource_mut::<LoadingProgress>();
    let transaction = progress.transaction();
    assert!(progress.complete_resident_restore(transaction, ScenePhase::Active, ResidentSceneToken(7)));
    app
}

#[test]
fn warm_restore_releases_complete_progress_without_recreating_a_startup_gate() {
    let mut app = restored_loading_app();
    assert!(!app.world().contains_resource::<SceneStartupGate>());
    app.update();
    assert!(!app.world().contains_resource::<LoadingProgress>());
    assert!(!app.world().contains_resource::<CriticalSceneAssets>());
}

#[test]
fn warm_restore_receipt_rejects_another_scene_token_or_incomplete_handoff() {
    use feathrise_scene_core::residency::{ResidentSceneToken, SceneResidencyActivity};
    for mismatch in 0..4 {
        let mut app = restored_loading_app();
        let mut activity = app.world_mut().resource_mut::<SceneResidencyActivity>();
        match mismatch {
            0 => activity.active = Some(ResidentSceneToken(8)),
            1 => activity.restored = Some(ResidentSceneToken(8)),
            2 => activity.switching = true,
            _ => activity.enabled = false,
        }
        app.update();
        assert!(app.world().contains_resource::<LoadingProgress>(), "错误驻留身份/未完成交接不能放行: {mismatch}");
    }
}

#[test]
fn warm_restore_receipt_does_not_bypass_a_pending_gate_or_wrong_state() {
    for with_gate in [true, false] {
        let mut app = restored_loading_app();
        if with_gate {
            app.insert_resource(SceneStartupGate::waiting_for_collision(ScenePhase::Active));
        } else {
            app.insert_resource(State::new(ScenePhase::Loading));
        }
        app.update();
        assert!(app.world().contains_resource::<LoadingProgress>());
    }
}

#[test]
fn a_late_warm_restore_cannot_complete_a_new_loading_transaction_for_the_same_scene() {
    use feathrise_scene_core::residency::ResidentSceneToken;
    let mut app = restored_loading_app();
    let stale = app.world().resource::<LoadingProgress>().transaction();
    let next = PendingSceneTransition::named(ScenePhase::Active, "同一作者场景再次进入");
    let mut progress = LoadingProgress::for_transition(&next);
    assert_ne!(stale, progress.transaction());
    assert!(!progress.complete_resident_restore(stale, ScenePhase::Active, ResidentSceneToken(7)));
    progress.stage = LoadingStage::Complete; // 即使显示层误写Complete，当前事务仍没有完成证明。
    app.insert_resource(progress);
    app.update();
    assert!(app.world().contains_resource::<LoadingProgress>());
}

#[test]
fn replacing_a_pending_request_requires_its_own_assets_and_application_permission() {
    let mut app = restored_loading_app();
    let stale = app.world().resource::<LoadingProgress>().transaction();
    let previous = PendingSceneTransition::named(ScenePhase::Active, "之前场景");
    let mut prerequisites = SceneLoadingPrerequisites::default();
    prerequisites.allow_transition(&previous);
    app.insert_resource(CriticalSceneAssets::for_transition(&previous, vec![]));
    let next = PendingSceneTransition::named(ScenePhase::Active, "后来明确选择的场景");
    let expected = next.transaction();
    app.insert_resource(State::new(ScenePhase::Loading))
        .insert_resource(next)
        .insert_resource(prerequisites);
    app.update(); // 使用正式Poll顺序，不由测试直接替换进度来掩盖请求身份漏接。
    let progress = app.world().resource::<LoadingProgress>();
    assert_ne!(expected, stale);
    assert_eq!(progress.transaction(), expected);
    assert_eq!(progress.target_display_name(ScenePhase::Active), "后来明确选择的场景");
    assert!(!progress.resident_restore_ready(ScenePhase::Active,
        app.world().resource::<feathrise_scene_core::residency::SceneResidencyActivity>()));
    assert!(app.world().contains_resource::<PendingSceneTransition>());
    assert!(!app.world().contains_resource::<SceneStartupGate>());

    // 即使B已经批准，A的有效空清单也不能作为B的资源证据。
    allow_application(&mut app);
    app.update();
    assert!(app.world().contains_resource::<PendingSceneTransition>());
    assert!(!app.world().contains_resource::<SceneStartupGate>());

    let queued = app.world_mut().resource_mut::<Assets<ReadyAsset>>().reserve_handle();
    submit_assets(&mut app, vec![queued.untyped()]);
    app.update();
    assert!(!app.world().contains_resource::<SceneStartupGate>());
    assert_eq!(app.world().resource::<LoadingProgress>().stage, LoadingStage::CriticalAssets);
    assert_eq!(app.world().resource::<LoadingProgress>().completed, 0);

    // 当前请求自己的真实资产已加载后才进入目标，加载期间不能借用暖回执或前一份列表。
    let ready = app.world().resource::<AssetServer>().add(ReadyAsset);
    submit_assets(&mut app, vec![ready.untyped()]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.update();
        if app.world().contains_resource::<SceneStartupGate>() { break; }
        assert!(std::time::Instant::now() < deadline, "当前请求关键资产未及时就绪");
        std::thread::yield_now();
    }
    assert_eq!(app.world().resource::<LoadingProgress>().transaction(), expected);
    assert_eq!(app.world().resource::<LoadingProgress>().stage, LoadingStage::SceneInit);
    assert!(!app.world().contains_resource::<PendingSceneTransition>());
    assert!(!app.world().resource::<SceneStartupGate>().ready);
}

#[test]
fn absent_or_unbound_asset_lists_never_mean_the_current_request_is_ready() {
    for unbound in [false, true] {
        let mut app = loading_app();
        allow_application(&mut app);
        if unbound { app.insert_resource(CriticalSceneAssets::default()); }
        app.update();
        assert!(app.world().contains_resource::<PendingSceneTransition>());
        assert!(!app.world().contains_resource::<SceneStartupGate>());
    }
}

#[test]
fn changing_a_request_target_invalidates_both_preparation_receipts() {
    let request = PendingSceneTransition::named(ScenePhase::Active, "作者场景");
    let assets = CriticalSceneAssets::for_transition(&request, vec![]);
    let mut prerequisites = SceneLoadingPrerequisites::default();
    prerequisites.retirement_pending = true;
    prerequisites.allow_transition(&request);
    let mut changed = request.clone();
    changed.set_target_display_name("改名");
    assert!(assets.is_for(&changed));
    assert!(prerequisites.ready_for(&changed));
    changed.target = ScenePhase::Idle;
    assert!(!assets.is_for(&changed));
    assert!(!prerequisites.ready_for(&changed));
    prerequisites.block_application();
    assert!(prerequisites.retirement_pending, "应用许可不能覆盖独立角色退出门");
}
