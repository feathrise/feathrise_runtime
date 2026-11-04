//! 单飞高度代理准备：共享 IO、分帧 Arc 捕获、后台网格、完整身份采用与失败光栅托底。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes}; // 每个子地图借用独立状态，父图保留原资源。
use super::super::SparseHeightPageRuntime;
use super::{
    components::ClipmapRaytracingAnchor,
    constants::*,
    mesh::{build_proxy, choose_grid_for_holes, validate_live_shape},
    resources::*,
    types::*,
};
use crate::maps::dynamic_map::{
    resources::{DynamicMapRuntimePhase, DynamicMapRuntimeStatus},
    sparse_runtime::SparseClipmapDisplay,
};
use crate::{DYNAMIC_TERRAIN_MESH_NAME, DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME};
use bevy::ecs::system::SystemParam;
use bevy::{
    asset::LoadState,
    camera::visibility::NoAutoAabb,
    gltf::{GltfMaterialName, GltfMeshName},
    prelude::*,
    tasks::{AsyncComputeTaskPool, futures::check_ready},
};
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;
use feathrise_map_content::{
    ActiveDynamicMap, SparseHeightLiveOverride, SparseHeightManifestAsset,
    SparseHeightSessionCrater, SparseHeightTileAsset, SparseHeightTileKey,

};
use feathrise_scene_core::state::ScenePhase;
use std::{collections::BTreeMap, time::Instant};

#[derive(SystemParam)]
pub(super) struct ProxyAssets<'w> {
    manifests: Res<'w, Assets<SparseHeightManifestAsset>>,
    tiles: Res<'w, Assets<SparseHeightTileAsset>>,
    surfaces: Res<'w, Assets<TerrainSurfaceMaterial>>,
    server: Res<'w, AssetServer>,
    meshes: ResMut<'w, Assets<Mesh>>,
}

fn revoke(
    commands: &mut Commands,
    runtime: &mut ClipmapRaytracingRuntime,
    meshes: &mut Assets<Mesh>,
) {
    if let Some(anchor) = runtime.anchor.take() {
        commands.entity(anchor).try_despawn();
    }
    if let Some(handle) = runtime.mesh.take()
        && let Some(mesh) = meshes.remove(handle.id())
    {
        runtime.retired.push(ProxyGarbage::Mesh(mesh)); // 大数组的析构也属于后台工作。
    }
    if let Some(capture) = runtime.capture.take() {
        runtime.retired.push(ProxyGarbage::Capture(capture));
    }
    runtime.started = None; // task 保留直到真实完成；不靠丢句柄假装释放计算槽。
}

fn fail(
    commands: &mut Commands,
    runtime: &mut ClipmapRaytracingRuntime,
    meshes: &mut Assets<Mesh>,
    status: &mut ClipmapRaytracingStatus,
    detail: String,
) {
    revoke(commands, runtime, meshes);
    runtime.failed = runtime.requested.clone();
    status.phase = ClipmapRaytracingPhase::Failed;
    status.detail = Some(detail);
}

fn reap(runtime: &mut ClipmapRaytracingRuntime) {
    if runtime
        .release
        .as_mut()
        .is_some_and(|task| check_ready(task).is_some())
    {
        runtime.release = None;
    }
    if runtime.release.is_none() && !runtime.retired.is_empty() {
        let garbage = std::mem::take(&mut runtime.retired);
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
            // 显式解构，让页 Arc、Mesh Vec 与迟到结果均在此 worker 上释放。
            for item in garbage {
                match item {
                    ProxyGarbage::Capture(input) => drop(input),
                    ProxyGarbage::Mesh(mesh) => drop(mesh),
                    ProxyGarbage::Result(result) => drop(result),
                }
            }
        }));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn drive_proxy(
    mut commands: Commands,
    request: Res<ClipmapRaytracingRequest>,
    scene: InstanceRes<State<ScenePhase>>,
    active: InstanceRes<ActiveDynamicMap>,
    map_status: InstanceRes<DynamicMapRuntimeStatus>,
    map_runtime: InstanceRes<crate::maps::dynamic_map::resources::DynamicMapRuntimeState>,
    pages: InstanceRes<SparseHeightPageRuntime>,
    topology: Option<InstanceRes<crate::maps::dynamic_map::terrain_topology::TerrainTopologyRuntime>>,
    assets: ProxyAssets,
    live: crate::maps::dynamic_map::author_sources::TerrainAuthorRes<SparseHeightLiveOverride>,
    craters: InstanceRes<SparseHeightSessionCrater>,
    displays: InstanceQuery<
        Entity,
        (
            With<SparseClipmapDisplay>,
            With<MeshMaterial3d<super::super::SparseClipmapSurfaceMaterial>>,
        ),
    >,
    mut runtime: ResMut<ClipmapRaytracingRuntime>,
    mut status: ResMut<ClipmapRaytracingStatus>,
) {
    let ProxyAssets {
        manifests,
        tiles,
        surfaces,
        server,
        mut meshes,
    } = assets;
    reap(&mut runtime);
    runtime.pending_io.retain(|handle| {
        matches!(
            server.load_state(handle.id()),
            LoadState::NotLoaded | LoadState::Loading
        )
    });
    let source = (request.enabled
        && *scene.get() == ScenePhase::Active
        && map_runtime.sparse_height_path.is_some()
        && map_status.phase == DynamicMapRuntimePhase::Ready)
        .then(|| pages.authority_source())
        .flatten()
        .filter(|(map, _, content)| {
            active.map_id.as_ref() == Some(map)
                && map_runtime.root.map(Entity::to_bits) == Some(content.root)
        });
    let desired = source.and_then(|(map, manifest, content)| {
        let live_revision = (live.map_id() == Some(&map)).then_some(live.revision());
        let crater_revision = (craters.map_id() == Some(&map) && !craters.tiles().is_empty())
            .then_some(craters.revision());
        if content.live != live_revision || content.crater != crater_revision {
            return None; // 作者/战斗在本帧晚于显示发布时改变内容，不能把未来高度冻结到旧身份。
        }
        let display = displays.single().ok()?;
        let material = pages.raytracing_material()?;
        surfaces.get(&material)?; // 外观还在高度感知回退时不以白模假冒正式地表。
        let identity = ProxyIdentity {
            map,
            root: Entity::from_bits(content.status_words()[0]),
            display,
            manifest: manifest.id(),
            material: material.id(),
            content: content.status_words(),
            live_revision,
            crater_revision,
            topology_revision: topology.as_deref().map_or(0, |topology| topology.revision)^pages.author_stitches.revision().rotate_left(17),
            ticket: 0,
        };
        Some((identity, manifest, material))
    });
    let changed = match (&runtime.requested, &desired) {
        (Some(old), Some((new, _, _))) => !old.same_source(new),
        (None, None) => false,
        _ => true,
    };
    if changed {
        revoke(&mut commands, &mut runtime, &mut meshes);
        runtime.ticket = runtime.ticket.wrapping_add(1).max(1);
        runtime.requested = desired.as_ref().map(|(identity, _, _)| {
            let mut identity = identity.clone();
            identity.ticket = runtime.ticket;
            identity
        });
        runtime.failed = None;
        status.detail = None;
    }

    if runtime
        .started
        .is_some_and(|start| start.elapsed().as_secs_f32() >= PREPARATION_TIMEOUT_SECONDS)
    {
        fail(
            &mut commands,
            &mut runtime,
            &mut meshes,
            &mut status,
            "Clipmap 光追高度准备超过 30 秒，继续使用光栅".into(),
        );
    } // 先执行墙钟停止线，已完成但迟到轮询的结果也不能越过超时门。
    if let Some(result) = runtime.task.as_mut().and_then(check_ready) {
        runtime.task = None;
        if runtime.requested.as_ref() == Some(&result.identity) && runtime.failed.is_none() {
            match result.mesh {
                Ok(prepared) => {
                    let handle = meshes.add(prepared.mesh);
                    let identity = result.identity;
                    let grid = prepared.grid;
                    // 只供适配器读取的隐藏源，不带物理地形或可见地表标记。
                    // 独立锚点身份避免重复接管，也避免永不绘制的 CPU Mesh 被计入管线预热。
                    let anchor = commands
                        .spawn((
                            Name::new("Clipmap世界固定光追高度源"),
                            Mesh3d(handle.clone()),
                            MeshMaterial3d(result.source.clone()),
                            GltfMeshName(DYNAMIC_TERRAIN_MESH_NAME.to_owned()),
                            GltfMaterialName(DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.to_owned()),
                            Visibility::Hidden,
                            NoAutoAabb, // CPU 源不做光栅剔除，禁止 Bevy 在主线程扫描整网格计算包围盒。
                            Transform::IDENTITY,
                            ChildOf(identity.root),
                            ClipmapRaytracingAnchor {
                                axes: prepared.axes,
                                identity,
                                grid,
                                source: result.source,
                            },
                        ))
                        .id();
                    runtime.anchor = Some(anchor);
                    runtime.mesh = Some(handle);
                    runtime.started = None;
                    status.phase = ClipmapRaytracingPhase::Ready;
                    status.mip_level = grid.level;
                    status.grid_cells = grid.cells;
                    status.detail = None;
                }
                Err(detail) => fail(
                    &mut commands,
                    &mut runtime,
                    &mut meshes,
                    &mut status,
                    detail,
                ),
            }
        } else {
            runtime.retired.push(ProxyGarbage::Result(result));
        }
    }
    if desired.is_none() {
        status.phase = if request.enabled
            && *scene.get() == ScenePhase::Active
            && map_runtime.sparse_height_path.is_some()
        {
            ClipmapRaytracingPhase::WaitingForTerrain
        } else {
            ClipmapRaytracingPhase::Disabled
        };
        if !request.enabled {
            runtime.failed = None;
            status.detail = None;
        }
        return;
    }
    if runtime.failed.is_some() || runtime.anchor.is_some() {
        return;
    }
    if runtime.task.is_some() {
        status.phase = ClipmapRaytracingPhase::Building;
        return;
    }
    if runtime.release.is_some() || !runtime.retired.is_empty() {
        return;
    }
    if runtime.capture.is_none() {
        let (_, manifest_handle, source) = desired.expect("已核对当前正式地形源");
        let Some(manifest) = manifests.get(&manifest_handle).map(|asset| &asset.manifest) else {
            return;
        };
        let grid = match choose_grid_for_holes(
            &manifest.spec,
            topology
                .as_deref()
                .is_some_and(|value| !value.applied.is_empty()),
            pages.author_stitches.active(),
        ) {
            Ok(grid) => grid,
            Err(detail) => {
                fail(
                    &mut commands,
                    &mut runtime,
                    &mut meshes,
                    &mut status,
                    detail,
                );
                return;
            }
        };
        let live_count = if live.map_id() == Some(&manifest.map_id) {
            live.tile_count()
        } else {
            0
        };
        if live_count > MAX_LIVE_PAGES {
            fail(
                &mut commands,
                &mut runtime,
                &mut meshes,
                &mut status,
                format!(
                    "Clipmap 光追实时覆盖 {live_count} 页超过 {MAX_LIVE_PAGES} 页输入预算，继续使用光栅"
                ),
            );
            return;
        }
        let counts = manifest
            .spec
            .mip_tile_counts(grid.level)
            .expect("格数已验证");
        let mut refs = Vec::new();
        for z in 0..counts[1] {
            for x in 0..counts[0] {
                let key = SparseHeightTileKey { x, z };
                let hash = if grid.level == 0 {
                    manifest.tile_ref(key).map(|r| &r.sha256)
                } else {
                    manifest.mip_tile_ref(grid.level, key).map(|r| &r.sha256)
                };
                if let Some(hash) = hash {
                    refs.push(FormalPage {
                        key,
                        hash: hash.clone(),
                        handle: None,
                    });
                }
            }
        }
        debug_assert!(refs.len() <= MAX_FORMAL_PAGES);
        let crater_count = if craters.map_id() == Some(&manifest.map_id) {
            craters.tiles().len()
        } else {
            0
        };
        runtime.capture = Some(ProxyCapture {
            stitches:pages.author_stitches.clone(),
            holes: topology
                .as_deref()
                .map(|topology| topology.applied.clone())
                .unwrap_or_default(),
            identity: runtime.requested.clone().expect("正式身份已建立"),
            manifest: manifest_handle,
            source,
            grid,
            spec: manifest.spec.clone(),
            refs,
            formal: BTreeMap::new(),
            live: BTreeMap::new(),
            live_cursor: 0,
            live_count,
            craters: BTreeMap::new(),
            crater_cursor: 0,
            crater_count,
        });
        runtime.started = Some(Instant::now());
        status.mip_level = grid.level;
        status.grid_cells = grid.cells;
    }
    let mut capture = runtime.capture.take().expect("上方已建立捕获");
    let Some(manifest) = manifests.get(&capture.manifest).map(|a| &a.manifest) else {
        runtime.capture = Some(capture);
        return;
    };
    for reference in &mut capture.refs {
        if capture.formal.contains_key(&reference.key) {
            continue;
        }
        if let Some(handle) = &reference.handle {
            if let Some(asset) = tiles.get(handle) {
                if let Err(detail) = asset.validate_page_against(
                    manifest,
                    capture.grid.level,
                    reference.key,
                    &reference.hash,
                ) {
                    runtime.retired.push(ProxyGarbage::Capture(capture));
                    fail(
                        &mut commands,
                        &mut runtime,
                        &mut meshes,
                        &mut status,
                        detail,
                    );
                    return;
                }
                capture.formal.insert(reference.key, asset.tile.clone());
            } else if matches!(server.load_state(handle.id()), LoadState::Failed(_)) {
                runtime.retired.push(ProxyGarbage::Capture(capture));
                fail(
                    &mut commands,
                    &mut runtime,
                    &mut meshes,
                    &mut status,
                    "Clipmap 光追登记高度页加载失败，禁止按平地采用".into(),
                );
                return;
            }
        } else if runtime.pending_io.is_empty() && pages.raytracing_io_available() {
            let path = if capture.grid.level == 0 {
                manifest.tile_asset_path(reference.key, &reference.hash)
            } else {
                manifest.mip_tile_asset_path(capture.grid.level, reference.key, &reference.hash)
            };
            let handle = server.load::<SparseHeightTileAsset>(path);
            runtime.pending_io.push(handle.clone());
            reference.handle = Some(handle);
        }
    }
    // 捕获仅复制固定数量的 Arc；不得深拷贝全图高度或在启动帧扫描全部作者页。
    let selected = live
        .tiles()
        .skip(capture.live_cursor)
        .take(CAPTURE_PAGES_PER_FRAME)
        .take(capture.live_count.saturating_sub(capture.live_cursor));
    for (key, tile) in selected {
        if let Err(detail) = validate_live_shape(&capture, tile) {
            runtime.retired.push(ProxyGarbage::Capture(capture));
            fail(
                &mut commands,
                &mut runtime,
                &mut meshes,
                &mut status,
                detail,
            );
            return;
        }
        capture.live.insert(key, tile.clone());
        capture.live_cursor += 1;
    }
    for (key, stamps) in craters
        .tiles()
        .iter()
        .skip(capture.crater_cursor)
        .take(CAPTURE_PAGES_PER_FRAME)
        .take(capture.crater_count.saturating_sub(capture.crater_cursor))
    {
        capture.craters.insert(*key, stamps.clone());
        capture.crater_cursor += 1;
    }
    if capture.formal.len() < capture.refs.len() {
        status.phase = ClipmapRaytracingPhase::LoadingPages;
        runtime.capture = Some(capture);
        return;
    }
    if capture.live_cursor != capture.live_count || capture.crater_cursor != capture.crater_count {
        status.phase = ClipmapRaytracingPhase::Capturing;
        runtime.capture = Some(capture);
        return;
    }
    status.phase = ClipmapRaytracingPhase::Building;
    runtime.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let mesh = build_proxy(&capture);
        ProxyResult {
            identity: capture.identity,
            source: capture.source,
            mesh,
        }
        // capture 的其余大页引用随闭包在 worker 上释放，Solari 适配开始前已归还输入峰值。
    }));
}
