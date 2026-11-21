//! 内存来源到生产 Assets 的采用屏障；热重载保留旧 Loaded 时仍需本次真实资产事件。

use super::{
    asset_overlay::{SourceEvidence, source_key},
    resources::Runtime,
};
use bevy::{
    asset::{AssetEventSystems, AssetPath, UntypedAssetLoadFailedEvent},
    ecs::message::MessageCursor,
    prelude::*,
};
use feathrise_scene_content::*;
use std::sync::atomic::Ordering;

pub(super) fn install(app: &mut App) {
    // 仅订阅当前快照会覆盖的正式资产类型；每个 reader 每帧最多消费 128 条。
    // 在生产事件发出之后记录，下一 Update 才能放开模式交接。
    macro_rules! track {
        ($($asset:ty),+ $(,)?) => { $(app.add_systems(PostUpdate,
            applied::<$asset>.after(AssetEventSystems));)+ };
    }
    track!(
        SceneCatalogAsset,
        SceneManifestAsset,
        ScenePlacementsAsset,
        ScenePortalsAsset,
        SceneBoundariesAsset,
        SceneSurfaceAsset,
        SceneVegetationAsset,
        SceneWaterAsset,
        WorldEnvironmentAsset,
        SceneVoxelTerrainSource,
        SparseHeightManifestAsset,
        SparseHeightTileAsset,
        SceneWaterMaskAsset,
        SceneSurfaceSourceAsset,
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
    runtime.source_reads.get(source_key(path.path())?.as_ref())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectContext, assets, embedded};
    use bevy::asset::{AssetLoader, LoadContext, io::Reader};
    use embedded::{
        EmbeddedProjectState,
        asset_overlay::{AssetOverlay, Files, Publication, ReadEvidence},
        snapshot::PreparedSnapshot,
        systems::drive,
        tool_brush::HeightSourceBindings,
        transport::{Inbound, test_runtime},
    };
    use feathrise_editor_protocol::{Message, Mode};
    use feathrise_scene_core::input::EditorInputState;
    use std::{
        path::Path,
        sync::{Arc, mpsc},
        time::{Duration, Instant},
    };

    const SOURCE: &str = "embedded-source-probe/root.readiness-probe";
    const MISSING: &str = "embedded-source-probe/missing.readiness-probe";

    #[derive(Asset, TypePath)]
    struct ProbeAsset {
        #[dependency]
        dependency: Option<Handle<ProbeAsset>>,
    }

    #[derive(TypePath)]
    struct ProbeLoader(async_channel::Receiver<()>);

    impl AssetLoader for ProbeLoader {
        type Asset = ProbeAsset;
        type Settings = ();
        type Error = bevy::ecs::error::BevyError;

        async fn load(&self, reader: &mut dyn Reader, _: &(), context: &mut LoadContext<'_>)
            -> Result<Self::Asset, Self::Error> {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            let dependency = if bytes == b"missing" {
                Some(context.load_builder().load(MISSING))
            } else {
                // 新版读取后才让测试释放解码，固定住旧依赖失败仍可见的时机。
                self.0.recv().await?;
                None
            };
            Ok(ProbeAsset { dependency })
        }

        fn extensions(&self) -> &[&str] {
            &["readiness-probe"]
        }
    }

    fn update_until(app: &mut App, condition: impl Fn(&World) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(app.world()) {
            assert!(Instant::now() < deadline, "Bevy 资产未推进到预期状态");
            app.update();
            std::thread::yield_now();
        }
    }

    #[test]
    fn adopted_source_dependency_failure_settles_and_repair_waits_for_the_new_root() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/support")
            .canonicalize()
            .unwrap();
        let project = ProjectContext {
            asset_roots: vec![root.join("assets")],
            root,
            id: "source-readiness-test".into(),
            name: "来源就绪夹具".into(),
        };
        let mut app = App::new();
        app.init_resource::<AssetOverlay>();
        let mut plugin = assets::install(&mut app, &project).unwrap();
        plugin.watch_for_changes_override = Some(false);
        let (release, repair_gate) = async_channel::bounded(1);
        app.add_plugins((MinimalPlugins, plugin))
            .init_asset::<ProbeAsset>()
            .register_asset_loader(ProbeLoader(repair_gate))
            .init_resource::<EditorInputState>()
            .insert_resource(EmbeddedProjectState {
                mode: Mode::Editing,
                revision: 1,
                transitioning: false,
                world_id: 1,
            })
            .add_systems(Update, drive)
            .add_systems(PostUpdate, (applied::<ProbeAsset>, failed).after(AssetEventSystems));

        let scene = SceneId("SOURCE-TEST".into());
        let mut active = ActiveDynamicScene::default();
        active.select(scene.clone());
        app.insert_resource(active);
        let files = |body: &[u8]| Arc::new(Files::from([(SOURCE.into(), Arc::new(body.to_vec()))]));
        let evidence = || Arc::new(ReadEvidence::from([(SOURCE.into(), SourceEvidence::default())]));
        let initial = evidence();
        assert!(app.world().resource::<AssetOverlay>().publish(Publication {
            files: files(b"missing"),
            reads: initial.clone(),
        }).is_ok());

        let (mut runtime, input, output) = test_runtime();
        let (retire, _retired) = mpsc::sync_channel(1);
        runtime.transport.retire = retire;
        runtime.paths.push(SOURCE.into());
        runtime.source_reads = initial.clone();
        runtime.sources_settled = false;
        runtime.sources_ready = false;
        app.insert_non_send(runtime);
        // PathBuf 在 Windows 写入原生分隔符，回执需匹配快照的斜杠键。
        let native_path = Path::new("embedded-source-probe").join("root.readiness-probe");
        let server = app.world().resource::<AssetServer>().clone();
        let handle = server.load::<ProbeAsset>(native_path.clone());
        assert_eq!(server.get_path(handle.id()).unwrap().path().as_os_str(), native_path.as_os_str());
        let dependencies_failed = || server.get_recursive_dependency_load_state(handle.id())
            .is_some_and(|state| state.is_failed());
        update_until(&mut app, |_| dependencies_failed());
        app.update();
        assert!(adopted(&initial[SOURCE]));
        assert!(!initial[SOURCE].failed.load(Ordering::Acquire));
        let runtime = app.world().non_send::<Runtime>();
        assert!(runtime.sources_settled && runtime.source_failed && !runtime.sources_ready);

        input.try_send(Inbound::Control(Message::SetMode {
            request_id: 7,
            mode: Mode::Playing,
            required_revision: 1,
        })).unwrap();
        app.update();
        assert!(matches!(output.try_recv().unwrap().message,
            Message::Failure { request_id: 7, code, .. } if code == "source-load-failed"));

        let repaired = evidence();
        input.try_send(Inbound::Snapshot(PreparedSnapshot {
            transaction: 2,
            revision: 2,
            generation: 1,
            scene_id: scene.clone(),
            name: "修复后的场景".into(),
            spawn: [0.0; 3],
            files: files(b"repaired"),
            paths: vec![SOURCE.into()],
            reload_paths: vec![SOURCE.into()],
            reads: repaired.clone(),
            height_bindings: HeightSourceBindings::new(scene, 2),
            heights: default(),
            height_overrides: default(),
            placements: None,
            scene_motion: None,
            clear_vegetation: false,
            clear_water: false,
        })).unwrap();
        update_until(&mut app, |_| repaired[SOURCE].read.load(Ordering::Acquire));
        app.update();
        assert_eq!(app.world().resource::<EmbeddedProjectState>().revision, 2);
        assert!(dependencies_failed() && !adopted(&repaired[SOURCE]));
        let runtime = app.world().non_send::<Runtime>();
        assert!(!runtime.sources_settled && !runtime.source_failed);
        assert!(matches!(output.try_recv().unwrap().message,
            Message::SnapshotApplied { transaction: 2, revision: 2 }));

        release.try_send(()).unwrap();
        update_until(&mut app, |world| world.non_send::<Runtime>().sources_ready);
        assert!(adopted(&repaired[SOURCE]));
        assert!(server.is_loaded_with_dependencies(handle.id()));
        let fixed = app.world().resource::<Assets<ProbeAsset>>().get(&handle).unwrap();
        assert!(fixed.dependency.is_none());
    }
}
