//! 资产日志区分真实失效与首载/退役通知，并验证轮转实例迟读和真正溢出的恢复合同。

use super::*;
use crate::residency::{SceneRuntimeScope, ResidentAssetEventReader};
use bevy::{asset::AssetPlugin, ecs::message::MessageCursor};

#[derive(Asset, TypePath)]
struct JournalAsset(u32);

fn fixture() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)))
        .init_asset::<JournalAsset>();
    install_instance_asset_journal::<JournalAsset>(&mut app);
    app
}

fn flush(app: &mut App) {
    // 原生Assets在PostUpdate发布，日志在下一帧捕获；继续两帧也证明不是依赖短期消息保留。
    for _ in 0..4 { app.update(); }
}

#[test]
fn retired_native_assets_do_not_accumulate_notices_or_restart_instances() {
    let mut app = fixture();
    for _ in 0..64 {
        let handles: Vec<_> = {
            let mut assets = app.world_mut().resource_mut::<Assets<JournalAsset>>();
            (0..96).map(|_| {
                let handle = assets.add(JournalAsset(1));
                assets.get_mut(&handle).unwrap().0 += 1;
                handle
            }).collect()
        };
        flush(&mut app);
        assert_eq!(app.world().resource::<InstanceAssetJournal<JournalAsset>>().records.len(), handles.len());
        assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 0,
            "首载Added与Modified混合超过128条，不应把可完整保留的96条失效误判为丢失");
        drop(handles); // 使用真实最后强句柄释放产生的Unused、Removed，不能手工只发送想要的事件类型。
        flush(&mut app);
        let journal = app.world().resource::<InstanceAssetJournal<JournalAsset>>();
        assert!(journal.records.is_empty() && journal.ids.is_empty());
        assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 0,
            "累计退役6144个资产也不能使固定容量日志进入重建->退役->再重建循环");
    }
}

#[test]
fn unused_marker_never_hides_reinserted_assets_or_explicit_removal() {
    let mut app = fixture();
    let id = AssetId::<JournalAsset>::default();
    let send = |app: &mut App, event| {
        app.world_mut().resource_mut::<Messages<AssetEvent<JournalAsset>>>().write(event);
        app.update();
    };
    send(&mut app, AssetEvent::Modified { id });
    send(&mut app, AssetEvent::Unused { id });
    assert!(app.world().resource::<InstanceAssetJournal<JournalAsset>>().records.is_empty());
    // Unused若没有配对Removed，后续Added/Modified必须结清这个单次标记，不能维护永久墓碑。
    send(&mut app, AssetEvent::Added { id });
    send(&mut app, AssetEvent::Modified { id });
    send(&mut app, AssetEvent::Removed { id });
    let journal = app.world().resource::<InstanceAssetJournal<JournalAsset>>();
    assert_eq!(journal.records.values().copied().collect::<Vec<_>>(), [AssetEvent::Removed { id }]);
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 0);

    send(&mut app, AssetEvent::Unused { id });
    flush(&mut app); // 配对跨空闲帧仍有效，但只消费它紧接的那一个原生GC Removed。
    send(&mut app, AssetEvent::Removed { id });
    assert!(app.world().resource::<InstanceAssetJournal<JournalAsset>>().records.is_empty());
    send(&mut app, AssetEvent::Modified { id });
    send(&mut app, AssetEvent::Removed { id });
    assert_eq!(app.world().resource::<InstanceAssetJournal<JournalAsset>>().records.values().copied().collect::<Vec<_>>(),
        [AssetEvent::Removed { id }]);
    send(&mut app, AssetEvent::Unused { id });
    send(&mut app, AssetEvent::Modified { id });
    send(&mut app, AssetEvent::Removed { id });
    assert_eq!(app.world().resource::<InstanceAssetJournal<JournalAsset>>().records.values().copied().collect::<Vec<_>>(),
        [AssetEvent::Removed { id }], "Modified也必须单独解除GC配对，不能要求先出现Added");
}

#[derive(Resource, Default)]
struct Received(Vec<AssetEvent<JournalAsset>>);

fn receive(mut events: ResidentAssetEventReader<JournalAsset>, mut received: ResMut<Received>) {
    received.0.extend(events.read().copied());
}

#[test]
fn load_burst_keeps_real_modification_and_explicit_removal_for_a_late_instance() {
    let mut app = fixture();
    app.init_resource::<Received>();
    let handle = app.world_mut().resource_mut::<Assets<JournalAsset>>().add(JournalAsset(0));
    flush(&mut app);
    let id = handle.id();
    {
        let mut events = app.world_mut().resource_mut::<Messages<AssetEvent<JournalAsset>>>();
        for _ in 0..CAPTURE_BUDGET * 4 { events.write(AssetEvent::Added { id }); }
        events.write(AssetEvent::Modified { id });
    }
    flush(&mut app);
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 0);
    let instance = app.world_mut().spawn_empty().id();
    app.insert_resource(SceneRuntimeScope { instance: Some(instance), ..default() });
    let mut schedule = Schedule::default();
    schedule.add_systems(receive);
    schedule.run(app.world_mut());
    let received = &app.world().resource::<Received>().0;
    assert_eq!(received.len(), 1, "晚轮转实例应只补收持久失效，不能仍读到首载原生消息");
    assert_eq!(received[0], AssetEvent::Modified { id });

    app.world_mut().resource_mut::<Assets<JournalAsset>>().remove(id).unwrap();
    flush(&mut app); // 强句柄仍在，真实显式Removed必须与GC回收区别处理。
    assert!(MessageCursor::<AssetEvent<JournalAsset>>::default()
        .is_empty(app.world().resource::<Messages<AssetEvent<JournalAsset>>>()));
    schedule.run(app.world_mut());
    let received = &app.world().resource::<Received>().0;
    assert_eq!(received.len(), 2);
    assert_eq!(received.as_slice(),
        [AssetEvent::Modified { id }, AssetEvent::Removed { id }]);
}

#[test]
fn real_invalidation_and_capacity_overflow_still_request_bounded_recovery() {
    let mut app = fixture();
    let handles: Vec<_> = {
        let assets = app.world().resource::<Assets<JournalAsset>>();
        (0..MAX_ASSET_NOTICES + 1).map(|_| assets.reserve_handle()).collect()
    };
    for chunk in handles[..MAX_ASSET_NOTICES].chunks(CAPTURE_BUDGET) {
        {
            let mut events = app.world_mut().resource_mut::<Messages<AssetEvent<JournalAsset>>>();
            for handle in chunk { events.write(AssetEvent::Modified { id: handle.id() }); }
        }
        app.update();
    }
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 0);
    app.world_mut().resource_mut::<Messages<AssetEvent<JournalAsset>>>()
        .write(AssetEvent::Removed { id: handles[MAX_ASSET_NOTICES].id() });
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 1,
        "真正丢失旧缓存失效信息仍须使协调器恢复完整候选");
    assert_eq!(app.world().resource::<InstanceAssetJournal<JournalAsset>>().records.len(), MAX_ASSET_NOTICES);

    {
        let mut events = app.world_mut().resource_mut::<Messages<AssetEvent<JournalAsset>>>();
        for handle in &handles[..CAPTURE_BUDGET + 1] { events.write(AssetEvent::Modified { id: handle.id() }); }
    }
    app.update();
    assert_eq!(app.world().resource::<InstanceAssetJournalEpoch>().0, 2,
        "超过128次失效写入时只提交一次溢出身份，不能静默吞掉真正热更");
    assert!(app.world().resource::<InstanceAssetJournal<JournalAsset>>().records.len() <= MAX_ASSET_NOTICES);
}
