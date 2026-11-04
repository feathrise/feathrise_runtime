//! F1编辑器档使用原始作者段的贴地轮廓；保留型Gizmo避免每帧重发整张地图线条。
mod geometry;
use feathrise_scene_core::residency::{InstanceRes}; // 每个子地图借用独立状态，父图保留原资源。
use super::super::resources::{
    DynamicMapBoundaryPhase, DynamicMapBoundaryState, DynamicMapBoundaryStatus,
};
use crate::maps::dynamic_map::terrain_derived::DynamicTerrainHeightCache;
use bevy::{
    camera::visibility::RenderLayers,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_scene_core::view::PrimarySceneView;
use feathrise_scene_core::constants::render_layers::{
    AIR_WALL_GAMEPLAY_LAYER, AIR_WALL_OUTLINE_GAMEPLAY_LAYER,
};
use feathrise_map_content::{MapBoundariesAsset, MapId, MapManifestAsset};
use feathrise_settings::{AirWallDisplayMode, SceneDisplaySettings};
use feathrise_scene_core::state::ScenePhase;
use std::sync::Arc;

#[derive(Clone, PartialEq)]
struct Identity {
    map: MapId,
    wall: u64,
    terrain: u64,
}

#[derive(Default)]
pub(in crate::maps::dynamic_map) struct OutlineState {
    identity: Option<Identity>,
    document: Option<Arc<MapBoundariesAsset>>,
    manifest: Option<Arc<MapManifestAsset>>, // 每个输入版本冻结一次清单，批次之间共享。
    generation: u64,
    cursor: Option<geometry::Cursor>,
    task: Option<Task<(u64, geometry::Batch)>>, // single-flight，过期任务仍保留直至完成。
    entities: Vec<Entity>,
    retiring: bool,
    complete: bool, // 全部轮廓登记后一次开放主相机层，不显示跨版本/半套轮廓。
}

/// 保留其他系统层位，只切换空气墙两种显示通道；retained Gizmo不读取Visibility。
fn camera_layers(layers: &RenderLayers, mode: AirWallDisplayMode, complete: bool) -> RenderLayers {
    let layers = layers
        .clone()
        .without(AIR_WALL_GAMEPLAY_LAYER)
        .without(AIR_WALL_OUTLINE_GAMEPLAY_LAYER);
    match mode {
        AirWallDisplayMode::Full => layers.with(AIR_WALL_GAMEPLAY_LAYER),
        AirWallDisplayMode::Editor if complete => layers.with(AIR_WALL_OUTLINE_GAMEPLAY_LAYER),
        _ => layers,
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::maps::dynamic_map) fn show_air_wall_outlines(
    mut commands: Commands,
    settings: Res<SceneDisplaySettings>,
    boundary: InstanceRes<DynamicMapBoundaryState>,
    status: InstanceRes<DynamicMapBoundaryStatus>,
    registry: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>,
    manifests: Option<Res<Assets<MapManifestAsset>>>,
    heights: Option<InstanceRes<DynamicTerrainHeightCache>>,
    mut assets: Option<ResMut<Assets<GizmoAsset>>>,
    mut cameras: Query<&mut RenderLayers, With<PrimarySceneView>>,
    mut state: Local<OutlineState>,
) {
    let mode = settings.air_wall_display_mode;
    let input = boundary.derivation.document.as_ref().and_then(|document| {
        if boundary.dirty || status.phase != DynamicMapBoundaryPhase::Ready {
            return None;
        }
        let map = &document.map_id;
        let terrain = heights.as_deref()?.ready_for(map)?;
        let handles = registry.as_deref()?.handles(map)?;
        let manifest = manifests.as_deref()?.get(&handles.manifest)?;
        Some((document, terrain, manifest))
    }); // 原始作者源含disabled但visible墙，完全不依赖碰撞切分端点。
    let identity = input.as_ref().map(|(document, terrain, _)| Identity {
        map: document.map_id.clone(),
        wall: boundary.generation,
        terrain: terrain.generation,
    });
    let same_document = match (&state.document, input.as_ref()) {
        (Some(old), Some((new, _, _))) => Arc::ptr_eq(old, new),
        (None, None) => true,
        _ => false,
    };
    if state.identity != identity || !same_document {
        state.identity = identity;
        state.document = input.as_ref().map(|(document, _, _)| Arc::clone(document));
        state.manifest = input
            .as_ref()
            .map(|(_, _, manifest)| Arc::new((*manifest).clone()));
        state.generation = state.generation.wrapping_add(1);
        state.cursor = input.as_ref().map(|_| geometry::Cursor::default());
        state.complete = false; // 当前帧立刻从主相机移除旧轮廓层。
        state.retiring = true;
    }
    if state.retiring {
        for _ in 0..32 {
            let Some(entity) = state.entities.pop() else {
                break;
            };
            commands.entity(entity).try_despawn(); // 固定登记/回收预算，避免整图实体释放峰值。
        }
        state.retiring = !state.entities.is_empty();
    }
    if let Some((generation, batch)) = state.task.as_mut().and_then(check_ready) {
        state.task = None;
        if generation == state.generation && !state.retiring {
            if let Some(assets) = assets.as_deref_mut() {
                state.cursor = batch.next;
                if !batch.asset.buffer().list_positions.is_empty() {
                    let entity = commands
                        .spawn((
                            Gizmo {
                                handle: assets.add(batch.asset),
                                line_config: GizmoLineConfig {
                                    width: 3.0,
                                    perspective: false,
                                    ..default()
                                },
                                depth_bias: -0.02,
                            }, // 与F12作者Gizmo宽度/深度完全一致。
                            Name::new("空气墙地表轮廓"), // 不生成墙面或控制点。
                            Transform::IDENTITY,
                            RenderLayers::layer(AIR_WALL_OUTLINE_GAMEPLAY_LAYER), // F12明确排除本层。
                            DespawnOnExit(ScenePhase::Active),
                        ))
                        .id();
                    state.entities.push(entity);
                }
                state.complete = state.cursor.is_none();
            }
        } // 迟到批次不产生资产/ECS副作用。
    }
    if mode == AirWallDisplayMode::Editor
        && !state.retiring
        && state.task.is_none()
        && assets.is_some()
        && let Some(cursor) = state.cursor
        && let Some((document, terrain, _)) = input
    {
        let document = Arc::clone(document);
        let heights = Arc::clone(&terrain.heights);
        let manifest = Arc::clone(state.manifest.as_ref().expect("当前输入清单")); // 全部批次复用一次冻结的元数据。
        let generation = state.generation;
        state.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            (
                generation,
                geometry::build(&document, &manifest, &heights, cursor),
            )
        }));
    }
    for mut layers in &mut cameras {
        let desired = camera_layers(&layers, mode, state.complete);
        if *layers != desired {
            *layers = desired;
        } // 模式变化或来源失效只改主相机，不扫描全部保留线实体。
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_layer_is_removed_immediately_when_source_becomes_incomplete() {
        let initial = RenderLayers::from_layers(&[0, 6, 99]);
        let ready = camera_layers(&initial, AirWallDisplayMode::Editor, true);
        assert!(ready.intersects(&RenderLayers::layer(AIR_WALL_OUTLINE_GAMEPLAY_LAYER)));
        let stale = camera_layers(&ready, AirWallDisplayMode::Editor, false);
        assert!(!stale.intersects(&RenderLayers::layer(AIR_WALL_OUTLINE_GAMEPLAY_LAYER)));
        assert!(stale.intersects(&RenderLayers::from_layers(&[99]))); // 不覆盖其他系统分配的层。
        let full = camera_layers(&ready, AirWallDisplayMode::Full, true);
        assert!(!full.intersects(&RenderLayers::layer(AIR_WALL_OUTLINE_GAMEPLAY_LAYER)));
        assert!(full.intersects(&RenderLayers::layer(AIR_WALL_GAMEPLAY_LAYER)));
    }
}
