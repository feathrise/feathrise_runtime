//! 动态地图绘制水域的R8快照、可选真实河床输入冻结、single-flight构建和原子采用系统。

use feathrise_scene_core::residency::{InstanceFrame, InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use std::collections::BTreeMap; // JSON层ID与R8作者源路径保持确定顺序。

#[cfg(test)]
use std::sync::Arc; // 纯构建回归创建最小共享量化高度测试输入。

#[cfg(test)]
use crate::shared::large_map::build_painted_water_mesh;
use crate::shared::large_map::{DynamicWaterMaterial, PaintedWaterTerrainInput};
use bevy::asset::LoadState; // JSON与R8作者源异步加载状态。
use bevy::prelude::*; // ECS、Assets和日志。
use bevy::tasks::AsyncComputeTaskPool; // 遮罩降采样、岸距和索引生成离开帧关键路径。
use feathrise_map_content::{
    ActiveDynamicMap, MAP_SCHEMA_VERSION, MapManifestAsset, MapTerrainKind,
    MapWaterAsset, MapWaterMaskAsset, WaterPresetRegistry,
}; // 当前MapId、目录强句柄、水域作者资产与动态地形清单。 // 安装批次复用正式GPU水材质；纯测试锁定空白新地图的合法结果。

use super::build::{DynamicWaterBuildInput, build_dynamic_water_network};
use super::resources::{DynamicClipmapWaterDraft, DynamicWaterPhase, DynamicWaterState, DynamicWaterStatus};
use super::runtime_batch::freeze_spline_water_network; // 三类作者值冻结和分帧GPU采用不继续膨胀系统编排文件。
use crate::maps::dynamic_map::terrain_derived::DynamicTerrainHeightCache; // 水域直接O(1)克隆GLB派生CPU高度Arc。

mod adoption; // GPU资产分帧登记、真实管线回执和整批原子采用保持独立职责。

use adoption::remove_installed_water; // 无水清单和同场景切图复用统一已安装批次回收。
pub(crate) use adoption::{
    clear_dynamic_water, commit_dynamic_water_for_terrain, install_dynamic_water,
}; // 独立热更可直接采用，GLB换代则参与四域共同提交。

/// 进入动态地图时建立新代次；若旧CPU任务仍在运行，保留句柄直到完成并按代次拒收。
pub(crate) fn begin_dynamic_water(
    active_map: InstanceRes<ActiveDynamicMap>,
    mut state: InstanceResMut<DynamicWaterState>,
    mut status: InstanceResMut<DynamicWaterStatus>,
) {
    state.mask_handles.clear();
    state.mask_asset_ids.clear();
    state.mask_paths.clear();
    state.terrain_generation = None; // 新地图重新取得自身输入身份；独立根无地形使用明确的零代次。
    state.desired_map_id = active_map.map_id.clone();
    state.installed_map_id = None;
    state.dirty = false;
    state.mark_dirty(&mut status);
    status.phase = DynamicWaterPhase::Loading;
    status.map_id = active_map.map_id.clone();
    status.applied_generation = 0; // 切图后上一地图的成功代次不再具有交接含义。
    status.height_binding_revision = [0; 3]; // GLB路径不发布Clipmap分页修订。
    status.input_draft_revision = None; // 新图尚未选择磁盘或F12草稿来源。
    status.layer_count = 0;
    status.triangle_count = 0;
    status.detail = "正在读取动态地图水域文件".to_owned();
}

fn fail_preparation(
    state: &mut DynamicWaterState,
    status: &mut DynamicWaterStatus,
    detail: String,
) {
    state.dirty = false;
    status.phase = DynamicWaterPhase::Failed;
    status.detail = detail;
    error!("{}", status.detail);
}

/// 冻结清单、完整R8与作者值后提交唯一后台任务；旧地形水另需同代真实河床，独立根水无需高度。
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_dynamic_water(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    active_map: InstanceRes<ActiveDynamicMap>,
    registry: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>,
    manifests: Res<Assets<MapManifestAsset>>,
    waters: Res<Assets<MapWaterAsset>>,
    masks: Res<Assets<MapWaterMaskAsset>>, // 无Mip作者源在IO任务内冻结，Update只取得Arc快照。
    water_presets: Option<Res<WaterPresetRegistry>>, // 静水在主线程冻结当前完整外观与微法线句柄。
    root_draft: Res<DynamicClipmapWaterDraft>, // 独立根静水使用自己的发布槽，不能继承当前地形作者源。
    frame: InstanceFrame,
    terrain_cache: InstanceRes<DynamicTerrainHeightCache>, // 读取完整原子采用的共享Arc<u16>，不再加载磁盘R16。
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<DynamicWaterMaterial>>,
    mut state: InstanceResMut<DynamicWaterState>,
    mut status: InstanceResMut<DynamicWaterStatus>,
) {
    let observed_preset_revision = water_presets.as_deref().map_or(0, |value| value.revision);
    if state.preset_revision != observed_preset_revision {
        state.preset_revision = observed_preset_revision;
        state.mark_dirty(&mut status); // 目录原子热更必须生成同修订的新材质批次。
    }
    if state.desired_map_id.as_ref() != active_map.map_id.as_ref() {
        state.desired_map_id = active_map.map_id.clone(); // MapId只在真实切图时克隆，稳定帧不分配String。
        state.mark_dirty(&mut status);
        status.applied_generation = 0; // 未经新地图原子安装前不得沿用旧图完成证据。
    }
    if state.installed_map_id.is_some()
        && state.installed_map_id.as_ref() != active_map.map_id.as_ref()
    {
        remove_installed_water(&mut commands, &mut meshes, &mut materials, &mut state);
        status.applied_generation = 0; // 同场景切图也立即撤销上一地图的原子采用证据。
        status.layer_count = 0;
        status.triangle_count = 0;
    }
    let independent = active_map.map_id.as_ref()
        .and_then(|id| registry.as_deref()?.handles(id))
        .and_then(|handles| manifests.get(&handles.manifest))
        .is_some_and(|manifest| manifest.terrain.kind == MapTerrainKind::Empty);
    let author = independent.then(|| active_map.map_id.as_ref()
        .filter(|_| frame.instance().is_none()).and_then(|id| root_draft.root_for_map(id))).flatten();
    let root_revision = author.and_then(|_| active_map.map_id.as_ref())
        .and_then(|id| root_draft.root_revision_for_map(id));
    if independent && status.input_draft_revision != root_revision {
        status.input_draft_revision = root_revision;
        state.mark_dirty(&mut status); // 未保存根R8与水位沿用同一single-flight与迟到拒收门。
    }
    let observed_terrain_generation = if independent { Some(0) } else { active_map
        .map_id
        .as_ref()
        .and_then(|map_id| terrain_cache.target_for(map_id))
        .map(|terrain| terrain.generation) }; // 零仅表示明确无地形的输入身份，不代表地面或高度Ready。
    if state.terrain_generation != observed_terrain_generation {
        state.terrain_generation = observed_terrain_generation; // 新GLB候选完成后即准备同代隐藏水域，正式切换仍等待共同提交。
        state.mark_dirty(&mut status); // 全部水层必须基于同一新高度代次重建。
    }
    if !state.dirty || state.task.is_some() || state.pending.is_some() {
        return;
    }
    let Some(map_id) = active_map.map_id.clone() else {
        fail_preparation(&mut state, &mut status, "尚未选择动态地图".to_owned());
        return;
    };
    status.map_id = Some(map_id.clone());
    let Some(registry) = registry.as_deref() else {
        status.phase = DynamicWaterPhase::Loading;
        status.detail = "地图目录仍在加载".to_owned();
        return;
    };
    let Some(handles) = registry.handles(&map_id) else {
        fail_preparation(&mut state, &mut status, format!("地图目录不存在{map_id}"));
        return;
    };
    if manifests.get(&handles.manifest).is_some_and(|manifest| {
        manifest.id == map_id && matches!(manifest.terrain.kind,MapTerrainKind::SparseClipmap|MapTerrainKind::Voxel)
    }) {
        // Clipmap专用系统消费同一dirty/generation事务，但不等待GLB整图高度缓存。
        // 此处仅保证旧GLB批次已撤销，随后同一Update由专用后端准备共享环带水面。
        remove_installed_water(&mut commands, &mut meshes, &mut materials, &mut state);
        state.terrain_generation = None;
        return;
    }
    if manifests.get(&handles.manifest).is_some_and(|manifest| {
        manifest.id == map_id && manifest.terrain.kind != MapTerrainKind::Empty && !manifest.terrain.kind.supports_spline_water()
    }) {
        remove_installed_water(&mut commands, &mut meshes, &mut materials, &mut state);
        state.mask_handles.clear();
    state.mask_asset_ids.clear();
        state.mask_paths.clear();
        state.terrain_generation = None;
        state.dirty = false;
        status.phase = DynamicWaterPhase::Disabled;
        status.desired_generation = 0;
        status.applied_generation = 0;
        status.layer_count = 0;
        status.triangle_count = 0;
        status.detail = format!("地图{map_id}当前地形路线不支持数据驱动水域");
        return;
    }
    let water_handle = handles.water.as_ref();
    if water_handle.is_none() && author.is_none() {
        remove_installed_water(&mut commands, &mut meshes, &mut materials, &mut state);
        state.mask_handles.clear();
    state.mask_asset_ids.clear();
        state.mask_paths.clear();
        state.terrain_generation = if independent { Some(0) } else { None }; // 独立画布保留无地形身份，避免稳定空图反复标脏。
        state.dirty = false;
        status.phase = DynamicWaterPhase::Disabled;
        status.desired_generation = 0; // 未声明水域没有等待中的输入事务。
        status.applied_generation = 0; // Disabled不对保存交接宣称任何水域采用代次。
        status.layer_count = 0;
        status.triangle_count = 0;
        status.detail = format!("地图{map_id}未声明数据驱动水域");
        return;
    }
    if matches!(
        asset_server.load_state(handles.manifest.id()),
        LoadState::Failed(_)
    ) {
        fail_preparation(
            &mut state,
            &mut status,
            format!("地图{map_id}清单加载失败，无法取得水面地形参数"),
        );
        return;
    }
    if author.is_none() && water_handle.is_some_and(|handle| matches!(asset_server.load_state(handle.id()), LoadState::Failed(_))) {
        fail_preparation(
            &mut state,
            &mut status,
            format!("地图{map_id}水域JSON加载失败"),
        );
        return;
    }
    let (Some(manifest), Some(water)) =
        (manifests.get(&handles.manifest), author.map(|author| &author.definition)
            .or_else(|| water_handle.and_then(|handle| waters.get(handle))))
    else {
        status.phase = DynamicWaterPhase::Loading;
        status.detail = format!("地图{map_id}水域JSON或清单仍在加载");
        return;
    };
    if water.schema_version != MAP_SCHEMA_VERSION {
        fail_preparation(
            &mut state,
            &mut status,
            format!(
                "地图{map_id}水域版本{}与运行时版本{}不一致",
                water.schema_version, MAP_SCHEMA_VERSION
            ),
        );
        return;
    }
    if let Err(error) = water.validate(&map_id) {
        fail_preparation(&mut state, &mut status, error);
        return;
    }
    if manifest.id != map_id
        || !manifest.is_data_driven()
        || manifest.world_bounds().is_none()
        || !matches!(manifest.terrain.kind, MapTerrainKind::Heightfield | MapTerrainKind::Empty)
        || (manifest.terrain.kind == MapTerrainKind::Heightfield && (manifest.terrain.resolution[0] < 2
            || manifest.terrain.resolution[1] < 2 || !manifest.terrain.base_height.is_finite()
            || !manifest.terrain.height_scale.is_finite() || manifest.terrain.height_scale <= 0.0))
    {
        fail_preparation(
            &mut state,
            &mut status,
            format!("地图{map_id}不是有效的动态水域来源清单"),
        );
        return;
    }
    let (terrain, terrain_generation) = if independent { (None, 0) } else {
    let Some(terrain_ready) = terrain_cache.target_for(&map_id) else {
        status.phase = DynamicWaterPhase::Loading; // 保留上一完整水面并等待GLB派生任务完成。
        status.detail = format!("正在等待地图{map_id}从GLB生成内存河床高度");
        return;
    };
    if terrain_ready.spec.resolution != manifest.terrain.resolution
        || terrain_ready.spec.world_min != manifest.world_min
        || terrain_ready.spec.world_max != manifest.world_max
        || terrain_ready.spec.base_height != manifest.terrain.base_height
        || terrain_ready.spec.height_scale != manifest.terrain.height_scale
    {
        fail_preparation(
            &mut state,
            &mut status,
            format!("地图{map_id}当前GLB派生高度与清单采样规格不一致"),
        );
        return;
    }
    let expected_height_count = (manifest.terrain.resolution[0] as usize)
        .checked_mul(manifest.terrain.resolution[1] as usize); // 防止损坏尺寸乘法溢出后误接半张派生高度网格。
    if expected_height_count != Some(terrain_ready.heights.len()) {
        fail_preparation(
            &mut state,
            &mut status,
            format!(
                "地图{map_id}的内存高度样本数{}与清单分辨率{}×{}不一致",
                terrain_ready.heights.len(),
                manifest.terrain.resolution[0],
                manifest.terrain.resolution[1]
            ),
        );
        return;
    }
    let terrain = match PaintedWaterTerrainInput::new(
        terrain_ready.heights.clone(), // O(1)冻结一份Arc供本批全部层共享。
        manifest.terrain.resolution,   // 权威内存高度顶点尺寸来自同一清单修订。
        manifest.world_min,            // 采样端点与水域世界范围完全一致。
        manifest.world_max,            // 采样端点与水域世界范围完全一致。
        manifest.terrain.base_height,  // u16零值映射到正式世界Y。
        manifest.terrain.height_scale, // u16最大值映射到正式米制量程。
    ) {
        Ok(terrain) => terrain, // 只有完整受检输入才能进入后台水面批次。
        Err(error) => {
            fail_preparation(
                &mut state,
                &mut status,
                format!("地图{map_id}的GLB派生水面输入无效：{error}"),
            );
            return;
        }
    };
    (Some(terrain), terrain_ready.generation) // 完成值携带真实GLB派生代次，安装前再次核对。
    };
    let paths = if author.is_some() { BTreeMap::new() } else { water
        .layers
        .iter()
        .map(|layer| (layer.id.clone(), layer.mask_source_path.clone()))
        .collect::<BTreeMap<_, _>>() };
    if paths != state.mask_paths {
        state.mask_handles = paths
            .iter()
            .map(|(layer_id, path)| {
                (
                    layer_id.clone(),
                    asset_server.load::<MapWaterMaskAsset>(path.clone()),
                )
            })
            .collect();
        state.mask_asset_ids = state.mask_handles.values().map(Handle::id).collect();
        state.mask_paths = paths;
    }
    let mut inputs = Vec::with_capacity(water.layers.len());
    for layer in &water.layers {
        let descriptor = if layer.preset_id.is_empty() {
            crate::shared::large_map::WaterSurfaceDescriptor::from_layer(layer)
        } else {
            let Some(presets) = water_presets.as_deref().filter(|value| value.revision > 0) else {
                status.phase = DynamicWaterPhase::Loading;
                status.detail = format!("正在等待静水体{}引用的全局水体预设目录", layer.label);
                return;
            };
            let Some(preset) = presets.resolve(&layer.preset_id) else {
                fail_preparation(
                    &mut state,
                    &mut status,
                    format!("静水体{}引用不存在的水体预设{}", layer.id, layer.preset_id),
                );
                return;
            };
            let Some(handles) = presets.image_handles(&preset.id) else {
                fail_preparation(
                    &mut state,
                    &mut status,
                    format!("水体预设{}缺少微法线句柄快照", preset.id),
                );
                return;
            };
            for (index, handle) in handles
                .iter()
                .enumerate()
                .filter_map(|(index, value)| value.as_ref().map(|handle| (index, handle)))
            {
                if matches!(asset_server.load_state(handle.id()), LoadState::Failed(_)) {
                    fail_preparation(
                        &mut state,
                        &mut status,
                        format!("水体预设{}的微法线{}加载失败", preset.id, index + 1),
                    );
                    return;
                }
                if !asset_server.is_loaded_with_dependencies(handle.id()) {
                    status.phase = DynamicWaterPhase::Loading;
                    status.detail =
                        format!("正在加载水体预设{}的微法线{}", preset.label, index + 1);
                    return;
                }
            }
            crate::shared::large_map::WaterSurfaceDescriptor::from_layer_with_preset(
                layer,
                preset,
                handles.clone(),
            )
        };
        let source = if let Some(author) = author {
            let Some(source) = author.sources.get(&layer.id) else {
                fail_preparation(&mut state, &mut status, format!("独立静水层{}缺少完整R8作者源", layer.id));
                return;
            };
            source.clone() // 只克隆Arc；在途根草稿不请求尚未保存的正式raw文件。
        } else {
        let Some(handle) = state.mask_handles.get(&layer.id) else {
            fail_preparation(
                &mut state,
                &mut status,
                format!("水域层{}没有R8作者源强句柄", layer.id),
            );
            return;
        };
        if matches!(asset_server.load_state(handle.id()), LoadState::Failed(_)) {
            fail_preparation(
                &mut state,
                &mut status,
                format!("水域层{}的R8作者源加载失败", layer.id),
            );
            return;
        }
        let Some(mask) = masks.get(handle) else {
            status.phase = DynamicWaterPhase::Loading;
            status.detail = format!("正在加载水域层{}的R8作者源", layer.label);
            return;
        };
        mask.shared_pixels()
        };
        let expected = (layer.resolution[0] as usize).checked_mul(layer.resolution[1] as usize); // 即使正式校验已限尺寸，这里仍拒绝乘法截断。
        if expected != Some(source.len()) {
            fail_preparation(
                &mut state,
                &mut status,
                format!(
                    "水域层{}的R8作者源像素数{}与JSON分辨率{}×{}不一致",
                    layer.id,
                    source.len(),
                    layer.resolution[0],
                    layer.resolution[1]
                ),
            );
            return;
        }
        inputs.push(DynamicWaterBuildInput {
            layer: layer.clone(),
            mask: source, // Update只克隆Arc；整张R8不再从Image CPU基础Mip深拷贝。
            descriptor,
        });
    }
    let generation = state.generation;
    let task_map_id = map_id.clone();
    let world_min = manifest.world_min; // 后台闭包不借用主世界清单资产。
    let world_max = manifest.world_max; // 全部水层在同一地图范围内原子构建。
    let (rivers, waterfalls) = freeze_spline_water_network(water); // 河流与瀑布控制点在同一受检水JSON代次冻结。
    let water_body_count = water.layers.len() + rivers.len() + waterfalls.len(); // 状态同时报告静水、河流和瀑布总数。
    state.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let result = build_dynamic_water_network(
            inputs,     // 全部R8静水层共享同一份GLB派生高度快照。
            rivers,     // 三维河流在相同single-flight任务内构建。
            waterfalls, // 瀑布幕布在相同single-flight任务内构建。
            world_min,  // 静水层使用同一地图世界最小角。
            world_max,  // 静水层使用同一地图世界最大角。
            terrain,    // CPU高度Arc只冻结一次并由全部静水层共享。
        );
        (task_map_id, generation, terrain_generation, result) // 完整身份随纯数据完成值返回主线程。
    }));
    state.dirty = false;
    status.phase = DynamicWaterPhase::Building;
    status.detail =
        format!("正在后台原子构建地图{map_id}的{water_body_count}个静水、河流与瀑布水体");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_root_authoring_starts_formal_builder_without_water_file_or_terrain() {
        let map = feathrise_map_content::MapId::parse("MAP-independent-root-runtime").unwrap();
        let manifest: MapManifestAsset = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "id": map, "display_name": "无地形静水",
            "runtime_scene": "DynamicMapScene", "world_min": [-0.5, -0.5],
            "world_max": [0.5, 0.5], "default_spawn": [0., 2., 0.], "terrains": []
        })).unwrap();
        let layer: feathrise_map_content::MapWaterLayer = serde_json::from_value(serde_json::json!({
            "id": "independent", "label": "独立湖泊",
            "mask_source_path": "maps/MAP-independent-root-runtime/water/lake.r8",
            "mask_texture_path": "maps/MAP-independent-root-runtime/water/lake.ktx2",
            "resolution": [2, 2], "canvas_bounds": [[-128., -64.], [128., 64.]], "water_level": -3.
        })).unwrap();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_resource::<Assets<MapManifestAsset>>().init_resource::<Assets<MapWaterAsset>>()
            .init_resource::<Assets<MapWaterMaskAsset>>().init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<DynamicWaterMaterial>>().init_resource::<DynamicTerrainHeightCache>()
            .init_resource::<DynamicWaterState>().init_resource::<DynamicWaterStatus>()
            .init_resource::<DynamicClipmapWaterDraft>().init_resource::<feathrise_scene_core::residency::MapRuntimeScope>()
            .insert_resource(ActiveDynamicMap { map_id: Some(map.clone()) })
            .add_systems(Update, prepare_dynamic_water);
        let handle = app.world_mut().resource_mut::<Assets<MapManifestAsset>>().add(manifest);
        app.insert_resource(feathrise_map_content::MapContentRegistry { revision: 1,
            maps: BTreeMap::from([(map.clone(), feathrise_map_content::MapContentHandles {
                manifest: handle, portals: default(), placements: default(), characters_path: None,
                surface: None, vegetation: None, water: None, boundaries: None,
            })]), ..default() });
        app.world_mut().resource_mut::<DynamicClipmapWaterDraft>().publish_root(super::super::resources::DynamicClipmapWaterDraftSnapshot {
            source_revision: 7,
            definition: MapWaterAsset { schema_version: 1, map_id: map.clone(), layers: vec![layer], ..default() },
            sources: BTreeMap::from([("independent".into(), Arc::from([255; 4]))]), masks: default(),
        });
        app.update();
        assert_eq!(app.world().resource::<DynamicWaterStatus>().phase, DynamicWaterPhase::Building);
        let state = app.world().resource::<DynamicWaterState>();
        assert_eq!(state.terrain_generation, Some(0));
        assert!(state.mask_handles.is_empty(), "未保存独立R8不应请求不存在的磁盘文件");
        let task = app.world_mut().resource_mut::<DynamicWaterState>().task.take().unwrap();
        let (built_map, _, terrain_generation, result) = bevy::tasks::block_on(task); // 只在测试中等完整纯CPU结果。
        assert_eq!(built_map, map);
        assert_eq!(terrain_generation, 0);
        assert!(result.unwrap().iter().any(|build| !build.mesh.is_empty()));
    }

    #[test]
    fn shared_builder_keeps_a_blank_explicit_water_layer_as_a_valid_atomic_result() {
        let terrain = PaintedWaterTerrainInput::new(
            Arc::<[u16]>::from(vec![0; 16]), // 空水画布仍提供完整合法派生河床输入。
            [4, 4],                          // 派生网格顶点尺寸与测试世界对应。
            [-2.0, -2.0],                    // 世界XZ最小角。
            [2.0, 2.0],                      // 世界XZ最大角。
            0.0,                             // 零高度对应世界Y零米。
            1.0,                             // 使用一米有效高度量程。
        )
        .expect("空水测试派生高度输入应有效"); // 先建立与正式运行时相同的权威高度封装。
        let mesh = build_painted_water_mesh(
            &[0; 16],     // 作者显式新建但尚未绘制的R8静水层仍合法。
            [4, 4],       // 遮罩尺寸保持最小测试预算以上。
            [-2.0, -2.0], // 与派生高度使用相同世界范围。
            [2.0, 2.0],   // 与派生高度使用相同世界范围。
            &terrain,     // 即使画布为空也不得绕过派生高度接口。
            1.0,          // 有效独立静止水位。
        )
        .expect("空白显式静水画布应构建成功"); // 空结果是成功而不是失败托底。
        assert!(mesh.is_empty());
    }
}
