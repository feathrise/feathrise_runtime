//! 正式协调器的发布、退役和原生资产事件回灌回归；稳定阶段不能自发生成下一套隐藏候选。

use super::*;
use feathrise_scene_core::residency::{InstanceAssetJournalEpoch, install_instance_asset_journal};

#[derive(Component)]
struct BatchImages(Vec<Handle<Image>>);

fn attach_batch_images(mut commands: Commands,
    roots: Query<Entity, (With<SubMapBatch>, Without<BatchImages>)>, mut images: ResMut<Assets<Image>>) {
    for root in &roots {
        // 每个实际发布批次都生成超过旧总预算的内存页；退役despawn真实释放全部强引用，形成完整反馈链。
        let handles = (0..160).map(|_| images.add(Image::default())).collect();
        commands.entity(root).insert(BatchImages(handles));
    }
}

fn feedback_fixture() -> App {
    let (mut app, map) = fixture();
    let old = app.world_mut().resource_mut::<SubMapRuntime>().active.take().unwrap();
    let graph = old.graph;
    app.world_mut().entity_mut(old.root).despawn();
    app.insert_resource(SubMapRuntime::default())
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)))
        .init_resource::<SubMapRuntimeStatus>()
        .init_resource::<feathrise_scene_core::state::SceneFallBoundary>()
        .insert_resource(MapInstanceRootRequest { map_id: Some(map.clone()) })
        .insert_resource(MapInstanceGraph { root_map_id: Some(map), revision: 1,
            structural_revision: 1, instances: graph, phase: MapInstanceGraphPhase::Ready, last_error: None })
        .init_asset::<Image>()
        .add_systems(Update, (drive, attach_batch_images).chain());
    install_instance_asset_journal::<Image>(&mut app);
    app
}

fn active_identity(app: &App) -> (Entity, u64) {
    let runtime = app.world().resource::<SubMapRuntime>();
    (runtime.active.as_ref().unwrap().root, runtime.generation)
}

fn tick(app: &mut App) {
    app.update();
    std::thread::sleep(std::time::Duration::from_millis(1)); // 仅让正式后台释放席位完成，不替换协调器的非阻塞轮询。
}

#[test]
fn coordinator_native_asset_feedback_settles_after_publication_and_retirement() {
    let mut app = feedback_fixture();
    tick(&mut app); // 从冷状态使用正式drive发布第一套完整批次。
    let first = active_identity(&app);
    assert_eq!(first.1, 1);
    assert_eq!(app.world().get::<BatchImages>(first.0).unwrap().0.len(), 160);
    for _ in 0..16 {
        tick(&mut app);
        assert_eq!(active_identity(&app), first,
            "初载Added不能耗尽失效预算并反向创建下一批次");
        assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 0);
    }

    // 使用真实Assets写入一次129个不同页的Modified，使超过128次失效写入的恢复路径确实执行。
    let handles = app.world().get::<BatchImages>(first.0).unwrap().0[..129].to_vec();
    {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        for handle in &handles { images.get_mut(handle).unwrap().texture_descriptor.mip_level_count = 1; }
    }
    drop(handles); // 不能由夹具继续保活旧页，否则无法覆盖退役后的原生Unused/Removed反馈。
    for _ in 0..8 { tick(&mut app); }
    let second = active_identity(&app);
    assert_eq!(second.1, 2, "真正溢出应恢复一次完整候选，然后停止自行换代");
    assert_ne!(second.0, first.0);
    assert!(app.world().get_entity(first.0).is_err(), "旧批次必须经过正式retire回收");
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1);
    for _ in 0..64 {
        tick(&mut app);
        let runtime = app.world().resource::<SubMapRuntime>();
        assert_eq!(active_identity(&app), second,
            "旧页退役后回灌的Unused/Removed不能启动新候选或重置已发布状态");
        assert!(runtime.candidate.is_none() && runtime.retired.is_empty());
        assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1);
    }
    assert_eq!(app.world().resource::<Assets<Image>>().len(), 160,
        "原生资产存储只剩当前批次页，不能靠保活已退役资产阻止循环");

    // 坏热更仍保留上一完整批次；修复图状态也不能凭空多建一套身份相同的内容。
    {
        let mut graph = app.world_mut().resource_mut::<MapInstanceGraph>();
        graph.phase = MapInstanceGraphPhase::Failed;
        graph.last_error = Some("坏热更保留上一完整实例".into());
    }
    for _ in 0..4 { tick(&mut app); assert_eq!(active_identity(&app), second); }
    assert_eq!(app.world().resource::<SubMapRuntimeStatus>().last_error.as_deref(), Some("坏热更保留上一完整实例"));
    app.world_mut().resource_mut::<MapInstanceGraph>().phase = MapInstanceGraphPhase::Ready;
    for _ in 0..4 { tick(&mut app); assert_eq!(active_identity(&app), second); }
}
