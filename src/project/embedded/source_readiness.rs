//! 内存来源到生产 Assets 的采用屏障；热重载保留旧 Loaded 时仍需本次真实资产事件。

use super::{asset_overlay::SourceEvidence, resources::Runtime};
use bevy::{
    asset::{AssetEventSystems, AssetPath, UntypedAssetLoadFailedEvent},
    ecs::message::MessageCursor,
    prelude::*,
};
use feathrise_map_content::*;
use std::sync::atomic::Ordering;

pub(super) fn install(app: &mut App) {
    // 仅订阅当前快照会覆盖的正式资产类型；每个 reader 每帧最多消费 128 条。
    // 在生产事件发出之后记录，下一 Update 才能放开模式交接。
    macro_rules! track {
        ($($asset:ty),+ $(,)?) => { $(app.add_systems(PostUpdate,
            applied::<$asset>.after(AssetEventSystems));)+ };
    }
    track!(
        MapCatalogAsset,
        MapManifestAsset,
        MapPlacementsAsset,
        MapPortalsAsset,
        MapBoundariesAsset,
        MapSurfaceAsset,
        MapVegetationAsset,
        MapWaterAsset,
        WorldEnvironmentAsset,
        MapVoxelTerrainSource,
        SparseHeightManifestAsset,
        SparseHeightTileAsset,
        MapWaterMaskAsset,
        MapSurfaceSourceAsset,
        Image,
        Gltf
    );
    app.add_systems(PostUpdate, failed.after(AssetEventSystems));
}

fn applied<A: Asset>(
    events: Res<Messages<AssetEvent<A>>>,
    mut cursor: Local<MessageCursor<AssetEvent<A>>>,
    server: Res<AssetServer>,
    mut runtime: NonSendMut<Runtime>,
) {
    if runtime.sources_settled {
        cursor.clear(&events);
        return;
    }
    if cursor.missed_messages(&events) != 0 {
        runtime.source_evidence_lost = true;
    }
    for event in cursor.read(&events).take(128) {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let Some(path) = server.get_path(*id) else {
            continue;
        };
        if let Some(source) = evidence(&runtime, &path) {
            record_applied(source);
        }
    }
}

fn failed(
    events: Res<Messages<UntypedAssetLoadFailedEvent>>,
    mut cursor: Local<MessageCursor<UntypedAssetLoadFailedEvent>>,
    mut runtime: NonSendMut<Runtime>,
) {
    if runtime.sources_settled {
        cursor.clear(&events);
        return;
    }
    if cursor.missed_messages(&events) != 0 {
        runtime.source_evidence_lost = true;
    }
    for event in cursor.read(&events).take(128) {
        if let Some(source) = evidence(&runtime, &event.path) {
            // 文件未打开也可能因 loader/meta 失败；失败属于终态，可让下一次修复快照接管。
            source.failed.store(true, Ordering::Release);
        }
    }
}

fn evidence<'a>(runtime: &'a Runtime, path: &AssetPath<'_>) -> Option<&'a SourceEvidence> {
    if path.source() != &bevy::asset::io::AssetSourceId::Default {
        return None;
    }
    runtime.source_reads.get(path.path().to_str()?)
}

pub(super) fn record_applied(source: &SourceEvidence) {
    if source.read.load(Ordering::Acquire) {
        source.applied.store(true, Ordering::Release);
    }
}

pub(super) fn loading(server: &AssetServer, path: &str) -> bool {
    let Some(handle) = server.get_handle_untyped(path.to_owned()) else {
        return false;
    };
    server
        .get_load_states(handle.id())
        .is_some_and(|(asset, dependencies, recursive)| {
            asset.is_loading() || dependencies.is_loading() || recursive.is_loading()
        })
}

pub(super) fn adopted(source: &SourceEvidence) -> bool {
    source.read.load(Ordering::Acquire) && source.applied.load(Ordering::Acquire)
}
