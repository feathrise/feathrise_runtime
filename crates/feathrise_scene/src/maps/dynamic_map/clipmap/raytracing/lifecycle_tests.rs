//! 最小真实 ECS/AssetServer 回归：准备、取消、迟到、内容换代和关闭态清理。

use super::super::{
    ClipmapBounds, SparseHeightPageRuntime,
    material::{ClipmapFlatHeightPage, ClipmapHeightExtension, SparseClipmapSurfaceMaterial},
};
use super::{ClipmapRaytracingAnchor, ClipmapRaytracingPlugin, resources::*};
use crate::maps::dynamic_map::{
    resources::{DynamicMapRuntimePhase, DynamicMapRuntimeState, DynamicMapRuntimeStatus},
    sparse_runtime::SparseClipmapDisplay,
};
use bevy::{
    asset::AssetPlugin, camera::visibility::VisibilityPlugin, ecs::system::RunSystemOnce,
    mesh::MeshPlugin, pbr::ExtendedMaterial, prelude::*, state::app::StatesPlugin,
};
use feathrise_atmosphere::DynamicMapWeatherClimate;
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;
use feathrise_map_content::{
    ActiveDynamicMap, MapContentHandles, MapContentRegistry, MapId, MapManifestAsset,
    SparseHeightLiveOverride, SparseHeightManifest, SparseHeightManifestAsset,
    SparseHeightSessionCrater, SparseHeightSpec, SparseHeightTileAsset,
};
use feathrise_scene_core::state::{ScenePhase, SceneFallBoundary};

struct Fixture {
    app: App,
    root: Entity,
    display: Entity,
    manifest: SparseHeightManifest,
    handle: Handle<SparseHeightManifestAsset>,
    source: Handle<TerrainSurfaceMaterial>,
}

fn fixture() -> Fixture {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        AssetPlugin::default(),
        MeshPlugin,
        TransformPlugin,
        VisibilityPlugin,
    ))
    .init_state::<ScenePhase>()
    .init_resource::<ActiveDynamicMap>()
    .init_resource::<DynamicMapWeatherClimate>()
    .init_resource::<SceneFallBoundary>()
    .init_resource::<DynamicMapRuntimeStatus>()
    .init_resource::<DynamicMapRuntimeState>()
    .init_resource::<SparseHeightPageRuntime>()
    .init_resource::<SparseHeightLiveOverride>()
    .init_resource::<SparseHeightSessionCrater>()
    .init_asset::<MapManifestAsset>()
    .init_asset::<TerrainSurfaceMaterial>()
    .init_asset::<SparseClipmapSurfaceMaterial>()
    .init_asset::<SparseHeightManifestAsset>()
    .init_asset::<SparseHeightTileAsset>()
    .add_plugins(ClipmapRaytracingPlugin);
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Active);
    let spec = SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -10.0, 20.0).unwrap();
    let map = MapId::parse("rt_fixture_test").unwrap();
    let manifest = SparseHeightManifest::empty(map.clone(), spec).unwrap();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<SparseHeightManifestAsset>>()
        .add(SparseHeightManifestAsset {
            manifest: manifest.clone(),
        });
    let author = super::material_tests::terrain();
    let source = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(author.clone());
    let height = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::ZERO, Vec2::splat(64.0)).unwrap(),
        UVec2::splat(64),
        &ClipmapFlatHeightPage {
            heights: Handle::default(),
            page_table: Handle::default(),
        },
    )
    .unwrap();
    let material = app
        .world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .add(ExtendedMaterial {
            base: author,
            extension: height,
        });
    // 必须通过生产清单准备系统建立父根，手工补齐组件会掩盖真实层级的 B0004。
    let map_manifest: MapManifestAsset = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "id": map,
        "display_name": "光追层级回归",
        "runtime_scene": "DynamicMapScene",
        "world_min": [0.0, 0.0],
        "world_max": [64.0, 64.0],
        "default_spawn": [32.0, 0.0, 32.0],
        "terrain": {
            "kind": "sparse_clipmap",
            "glb_path": "",
            "height_path": feathrise_map_content::sparse_height_manifest_asset_path(&map),
            "resolution": [65, 65],
            "base_height": -10.0,
            "height_scale": 20.0
        }
    }))
    .unwrap();
    let map_handle = app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .add(map_manifest);
    app.insert_resource(MapContentRegistry {
        catalog: Handle::default(),
        revision: 1,
        maps: std::collections::BTreeMap::from([(
            map.clone(),
            MapContentHandles {
                characters_path: None,
                manifest: map_handle,
                portals: Handle::default(),
                placements: Handle::default(),
                surface: None,
                vegetation: None,
                water: None,
                boundaries: None,
            },
        )]),
        last_error: None,
    });
    app.world_mut().resource_mut::<ActiveDynamicMap>().map_id = Some(map.clone());
    app.world_mut()
        .run_system_once(crate::maps::dynamic_map::systems::prepare_dynamic_map_terrain)
        .unwrap();
    let root = app
        .world()
        .resource::<DynamicMapRuntimeState>()
        .root
        .expect("生产准备系统必须建立稀疏地图根");
    let display = app
        .world_mut()
        .spawn((
            SparseClipmapDisplay,
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::Inherited,
            ChildOf(root),
        ))
        .id();
    app.world_mut().resource_mut::<ActiveDynamicMap>().map_id = Some(map.clone());
    let mut status = app.world_mut().resource_mut::<DynamicMapRuntimeStatus>();
    status.phase = DynamicMapRuntimePhase::Ready;
    status.map_id = Some(map);
    status.revision = 1;
    app.world_mut()
        .resource_mut::<SparseHeightPageRuntime>()
        .adopt_raytracing_source_for_test(
            root,
            handle.clone(),
            &manifest,
            source.clone(),
            1,
            None,
            None,
        );
    app.world_mut()
        .resource_mut::<ClipmapRaytracingRequest>()
        .enabled = true;
    Fixture {
        app,
        root,
        display,
        manifest,
        handle,
        source,
    }
}

fn ready(app: &mut App) -> Entity {
    let stop = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < stop {
        app.update();
        if let Some(anchor) = app.world().resource::<ClipmapRaytracingRuntime>().anchor {
            return anchor;
        }
        std::thread::yield_now(); // 仅测试驱动事件循环，运行时没有等待或忙轮询。
    }
    panic!(
        "代理未准备：{:?}",
        app.world().resource::<ClipmapRaytracingStatus>().detail
    );
}

#[test]
fn sparse_map_root_propagates_visibility_without_exposing_raytracing_anchor() {
    let mut f = fixture();
    let anchor = ready(&mut f.app);
    assert_eq!(
        f.app.world().get::<ChildOf>(anchor).unwrap().parent(),
        f.root
    );
    assert!(
        f.app
            .world()
            .get::<InheritedVisibility>(f.root)
            .expect("地图根必须具备可见性继承组件，不能触发 B0004")
            .get()
    );
    assert!(f.app.world().get::<ViewVisibility>(f.root).is_some());
    // 使用真实可见性传播验证整个父子链；隐藏高度源不能成为重复光栅地形。
    for visibility in [Visibility::Hidden, Visibility::Inherited] {
        f.app.world_mut().entity_mut(f.root).insert(visibility);
        f.app.update();
        assert_eq!(
            f.app
                .world()
                .get::<InheritedVisibility>(f.display)
                .unwrap()
                .get(),
            visibility != Visibility::Hidden
        );
        assert_eq!(
            f.app.world().get::<Visibility>(anchor),
            Some(&Visibility::Hidden)
        );
        assert!(
            !f.app
                .world()
                .get::<InheritedVisibility>(anchor)
                .unwrap()
                .get()
        );
        assert!(!f.app.world().get::<ViewVisibility>(anchor).unwrap().get());
        assert_eq!(
            f.app.world().resource::<ClipmapRaytracingRuntime>().anchor,
            Some(anchor)
        );
    }
}

#[test]
fn fixed_anchor_survives_view_movement_and_rebuilds_only_for_content_then_cleans_up() {
    let mut f = fixture();
    let first = ready(&mut f.app);
    let mesh = f.app.world().get::<Mesh3d>(first).unwrap().id();
    assert_eq!(
        f.app.world().get::<Visibility>(first),
        Some(&Visibility::Hidden)
    );
    assert!(
        f.app
            .world()
            .get::<crate::DynamicMapTerrainPrimitive>(first)
            .is_none()
    );
    assert!(
        f.app
            .world()
            .get::<feathrise_environment::terrain_surface::TerrainSurfaceCarrier>(first)
            .is_none()
    ); // 隐藏 CPU 源不参与可见地表的管线预热计数。
    assert!(
        f.app
            .world()
            .get::<bevy::camera::visibility::NoAutoAabb>(first)
            .is_some()
    );
    assert!(
        f.app
            .world()
            .get::<bevy::camera::primitives::Aabb>(first)
            .is_none()
    );
    // 执行真实 Bevy 包围盒系统后仍无 Aabb，证明不会隐式扫描 CPU 源顶点。
    f.app
        .world_mut()
        .entity_mut(f.display)
        .insert(Transform::from_xyz(24.0, 0.0, 17.0));
    for _ in 0..4 {
        f.app.update();
    }
    assert_eq!(
        f.app.world().resource::<ClipmapRaytracingRuntime>().anchor,
        Some(first)
    );
    assert_eq!(f.app.world().get::<Mesh3d>(first).unwrap().id(), mesh);
    f.app
        .world_mut()
        .resource_mut::<SparseHeightPageRuntime>()
        .adopt_raytracing_source_for_test(
            f.root,
            f.handle.clone(),
            &f.manifest,
            f.source.clone(),
            2,
            None,
            None,
        );
    f.app.update();
    assert!(f.app.world().get_entity(first).is_err());
    assert!(
        f.app
            .world()
            .resource::<ClipmapRaytracingRuntime>()
            .anchor
            .is_none()
    );
    let second = ready(&mut f.app);
    assert_ne!(first, second);
    f.app
        .world_mut()
        .resource_mut::<ClipmapRaytracingRequest>()
        .enabled = false;
    f.app.update();
    assert_eq!(
        f.app.world().resource::<ClipmapRaytracingStatus>().phase,
        ClipmapRaytracingPhase::Disabled
    );
    assert!(f.app.world().get_entity(second).is_err());
    assert!(f.app.world().resource::<Assets<Mesh>>().get(mesh).is_none());
}

#[test]
fn cancelled_background_completion_cannot_publish_and_slot_is_retained_until_finished() {
    let mut f = fixture();
    f.app.update(); // 空图无需 IO，已启动真实后台 Mesh 任务但尚未轮询采用。
    assert!(
        f.app
            .world()
            .resource::<ClipmapRaytracingRuntime>()
            .task
            .is_some()
    );
    f.app
        .world_mut()
        .resource_mut::<ClipmapRaytracingRequest>()
        .enabled = false;
    for _ in 0..10 {
        f.app.update();
        std::thread::yield_now();
    }
    assert!(
        f.app
            .world_mut()
            .query::<&ClipmapRaytracingAnchor>()
            .iter(f.app.world())
            .next()
            .is_none()
    );
    assert!(
        f.app
            .world()
            .resource::<ClipmapRaytracingRuntime>()
            .anchor
            .is_none()
    );
    f.app
        .world_mut()
        .resource_mut::<ClipmapRaytracingRequest>()
        .enabled = true;
    ready(&mut f.app); // 真正结束后归还槽位，重新开启必须有新票据才可采用。
}

#[test]
fn wall_clock_timeout_rejects_completed_result_even_with_virtual_time_paused_and_allows_retry() {
    let mut f = fixture();
    f.app.update();
    f.app.world_mut().resource_mut::<Time<Virtual>>().pause();
    f.app
        .world_mut()
        .resource_mut::<ClipmapRaytracingRuntime>()
        .started = Some(std::time::Instant::now() - std::time::Duration::from_secs(31));
    f.app.update();
    let status = f.app.world().resource::<ClipmapRaytracingStatus>();
    assert_eq!(status.phase, ClipmapRaytracingPhase::Failed);
    assert!(status.detail.as_ref().unwrap().contains("30 秒"));
    assert!(
        f.app
            .world()
            .resource::<ClipmapRaytracingRuntime>()
            .anchor
            .is_none()
    );
    for _ in 0..4 {
        f.app.update();
    }
    assert!(
        f.app
            .world()
            .resource::<ClipmapRaytracingRuntime>()
            .failed
            .is_some()
    );
    assert_eq!(
        f.app.world().resource::<DynamicMapRuntimeStatus>().phase,
        DynamicMapRuntimePhase::Ready
    );
    f.app
        .world_mut()
        .resource_mut::<ClipmapRaytracingRequest>()
        .enabled = false;
    f.app.update();
    f.app
        .world_mut()
        .resource_mut::<ClipmapRaytracingRequest>()
        .enabled = true;
    ready(&mut f.app); // 游戏暂停不影响后台计算与非阻塞采用。
}

#[test]
fn future_live_revision_cannot_be_frozen_under_previous_visible_revision() {
    let mut f = fixture();
    let first = ready(&mut f.app);
    f.app
        .world_mut()
        .resource_mut::<SparseHeightLiveOverride>()
        .retarget(Some(f.manifest.map_id.clone()), 0);
    f.app.update();
    assert!(f.app.world().get_entity(first).is_err());
    assert!(
        f.app
            .world()
            .resource::<ClipmapRaytracingRuntime>()
            .anchor
            .is_none()
    );
    // None 和 Some(0) 不能因 status_words 的零哨兵合并；正式显示尚未接收该作者会话。
    f.app
        .world_mut()
        .resource_mut::<SparseHeightPageRuntime>()
        .adopt_raytracing_source_for_test(
            f.root,
            f.handle.clone(),
            &f.manifest,
            f.source.clone(),
            1,
            Some(0),
            None,
        );
    ready(&mut f.app);
}
