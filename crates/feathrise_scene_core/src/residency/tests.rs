//! 资源槽保留原值、缺席和公共资产身份；重复登记不改变所有权。

use super::*;

#[test]
fn resident_asset_recheck_reaches_runtime_without_republishing_native_gpu_events() {
    #[derive(Resource, Default)]
    struct Counts {
        runtime: usize,
        native: usize,
    }
    fn consume(
        mut runtime: ResidentAssetEventReader<Image>,
        mut native: MessageReader<AssetEvent<Image>>,
        mut counts: ResMut<Counts>,
    ) {
        counts.runtime += runtime.read().count();
        counts.native += native.read().count();
    }
    let mut app = App::new();
    app.add_message::<AssetEvent<Image>>()
        .init_resource::<Counts>()
        .add_systems(Update, consume);
    let id = Handle::<Image>::default().id();
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<Image>>>()
        .write(AssetEvent::Added { id });
    app.update(); // 独立插件不安装驻留通道也能消费原事件。
    assert_eq!(app.world().resource::<Counts>().runtime, 1);
    app.add_message::<ResidentAssetEvent<Image>>();
    app.world_mut()
        .resource_mut::<Messages<ResidentAssetEvent<Image>>>()
        .write(ResidentAssetEvent(AssetEvent::Modified { id }));
    app.update();
    assert_eq!(app.world().resource::<Counts>().runtime, 2);
    assert_eq!(app.world().resource::<Counts>().native, 1); // WorldSpawner和GPU资产消费者只看到真实原消息。
}
use bevy::prelude::*;
use std::sync::Arc;

#[derive(Resource, Default)]
struct Owned(Arc<Vec<u8>>);
#[derive(Resource)]
struct Optional(usize);
#[derive(Resource, Default)]
struct Shared(usize);

#[test]
fn snapshots_move_original_allocations_and_do_not_capture_shared_services() {
    let mut app = App::new();
    app.init_resident_resource::<Owned>()
        .init_resident_resource::<Owned>()
        .init_resource::<Shared>()
        .register_optional_resident_resource::<Optional>();
    let allocation = Arc::new(vec![7; 1024]);
    app.world_mut().insert_resource(Owned(allocation.clone()));
    app.world_mut().insert_resource(Optional(5));
    let mut snapshot = take_resident_resources(app.world_mut());
    assert!(app.world().resource::<Owned>().0.is_empty());
    assert!(!app.world().contains_resource::<Optional>());
    app.world_mut().resource_mut::<Shared>().0 = 9;
    restore_resident_resources(app.world_mut(), &mut snapshot);
    assert!(Arc::ptr_eq(&allocation, &app.world().resource::<Owned>().0));
    assert_eq!(app.world().resource::<Optional>().0, 5);
    assert_eq!(app.world().resource::<Shared>().0, 9);
}

#[test]
fn absent_optional_resource_removes_loading_placeholder_on_restore() {
    let mut app = App::new();
    app.register_optional_resident_resource::<Optional>();
    let mut snapshot = take_resident_resources(app.world_mut());
    app.world_mut().insert_resource(Optional(100));
    restore_resident_resources(app.world_mut(), &mut snapshot);
    assert!(!app.world().contains_resource::<Optional>());
}
