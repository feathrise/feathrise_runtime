//! 动态植被目录观察、定义校验与强资产请求。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use bevy::asset::LoadState;
use bevy::prelude::*;
use feathrise_map_content::{
    ActiveDynamicMap, MapManifestAsset, MapVegetationAsset, MapVegetationKind,
    VegetationPresetRegistry,
};

use super::helpers::{
    blade_mesh_event_requests_rebuild, event_asset_id, event_requests_rebuild,
    image_event_requests_rebuild, load_pending_assets, validate_vegetation_definition,
};
use super::resources::{DynamicVegetationPhase, DynamicVegetationState, DynamicVegetationStatus};
use super::transactions::fail_current_revision;
use crate::maps::dynamic_map::terrain_derived::DynamicTerrainHeightCache; // 当前定义只采用同代次完整CPU/GPU高度派生值。

/// 目录、当前地图、JSON、密度、高度或模型变化只合并成一次重建请求。
#[allow(clippy::too_many_arguments)]
pub(in crate::maps::dynamic_map) fn observe_dynamic_vegetation_assets(
    active_map: InstanceRes<ActiveDynamicMap>,
    registry: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>,
    preset_registry: Option<Res<VegetationPresetRegistry>>, // 开发者预设成功热更要求重建当前地图引用。
    terrain_cache: InstanceRes<DynamicTerrainHeightCache>,          // 完整派生代次变化独立于磁盘Image事件。
    mut definition_events: feathrise_scene_core::residency::ResidentAssetEventReader<MapVegetationAsset>,
    mut image_events: feathrise_scene_core::residency::ResidentAssetEventReader<Image>,
    mut mesh_events: feathrise_scene_core::residency::ResidentAssetEventReader<Mesh>,
    meshes: Res<Assets<Mesh>>, // 区分内部 UV 补齐与真正破坏顶点 ABI 的网格热更。
    draft: crate::maps::dynamic_map::author_sources::TerrainAuthorRes<super::DynamicBladeDraft>, // 完整Blade/Mesh草稿边沿也必须使旧后台候选失效。
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    let failed = status.phase == DynamicVegetationPhase::Failed;
    let draft_revision = active_map
        .map_id
        .as_ref()
        .and_then(|id| draft.for_map(id))
        .map(|_| draft.layout_revision); // Blade密度由净空派生链更新，布局变更才重新加载资产和建立根。
    let draft_changed = state.draft_revision != draft_revision; // 关闭F12不改变此身份。
    state.draft_revision = draft_revision; // 常数快照比较避免稳定帧扫描层。
    let mut changed = active_map.is_changed()
        || draft_changed // 新建、刷制、删除和撤销均走正式异步建根。
        || registry
            .as_deref()
            .is_some_and(|registry| registry.revision != state.catalog_revision)
        || preset_registry
            .as_deref()
            .is_some_and(|registry| registry.revision != state.preset_catalog_revision); // 地图目录与植被预设目录拥有独立原子修订号。
    let terrain_generation = active_map
        .map_id
        .as_ref()
        .and_then(|map_id| terrain_cache.target_for(map_id))
        .map(|terrain| terrain.generation); // 稳定帧仅比较一个Option<u64>。
    if state.terrain_generation != terrain_generation {
        state.terrain_generation = terrain_generation; // 只接受完整原子采用的GLB派生代次。
        changed = true; // 新CPU高度和内存Image必须一起建立植被新修订。
    }
    if let Some(handle) = state.definition_handle.as_ref() {
        changed |= definition_events.read().any(|event| {
            event_asset_id(event) == handle.id() && event_requests_rebuild(event, failed)
        });
    } else {
        definition_events.read().for_each(drop);
    }
    if let Some(pending) = state.pending.as_ref() {
        changed |= image_events.read().any(|event| {
            let id = event_asset_id(event);
            let mesh_density_changed = pending.layers.iter().any(|layer| {
                layer.definition.kind == MapVegetationKind::Mesh && layer.density.id() == id
            }); // Mesh密度变化需要重新执行后台确定性散布。
            let blade_density_changed = pending.layers.iter().any(|layer| {
                layer.definition.kind == MapVegetationKind::Blade && layer.density.id() == id
            }); // Blade窗口继续持有同一Image句柄，Modified只需RenderAsset上传。
            let blade_base_color_changed = pending.blade_profile_banks.iter().any(|bank| {
                bank.blade_base_color
                    .as_ref()
                    .is_some_and(|texture| texture.id() == id)
            }); // 数据预设图集 Modified 由 RenderAssets 原位更新，Removed 则进入正式失败/恢复事务。
            let tracked = mesh_density_changed || blade_density_changed || blade_base_color_changed; // 内存高度以缓存generation观察，不依赖Image消息乱序。
            tracked
                && image_event_requests_rebuild(
                    event,                // 保留Removed和失败恢复语义。
                    mesh_density_changed, // 只有CPU依赖的Mesh密度Modified才重建整根。
                    failed,               // 当前失败状态允许资源修复重新尝试。
                )
        });
        changed |= mesh_events.read().any(|event| {
            let id = event_asset_id(event);
            let tracked = pending.layers.iter().any(|layer| {
                layer
                    .blade_meshes
                    .as_ref()
                    .is_some_and(|meshes| meshes.iter().any(|mesh| mesh.id() == id))
            }); // 仅当前地图实际引用的 Blade LOD 参与事件判定。
            tracked && blade_mesh_event_requests_rebuild(event, meshes.get(id), failed) // 合法 Modified 由 RenderAsset 原位更新。
        });
    } else {
        image_events.read().for_each(drop);
        mesh_events.read().for_each(drop);
    }
    if changed {
        status.input_draft_revision = draft_revision; // 将错误和等待绑定具体草稿代次。
        state.request_rebuild();
        status.phase = DynamicVegetationPhase::Waiting;
        status.detail = if state.active_root.is_some() {
            "正在准备植被热更新；当前完整修订继续显示".to_owned()
        } else {
            "正在准备动态地图植被".to_owned()
        };
    }
}

/// 目录或定义变化后完整校验并一次请求全部强资产句柄。
#[allow(clippy::too_many_arguments)]
pub(in crate::maps::dynamic_map) fn prepare_dynamic_vegetation_definition(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    active_map: InstanceRes<ActiveDynamicMap>,
    registry: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>,
    preset_registry: Option<Res<VegetationPresetRegistry>>, // 定义校验、别名迁移和运行时资产都读取同一快照。
    manifests: Res<Assets<MapManifestAsset>>,
    definitions: Res<Assets<MapVegetationAsset>>,
    draft: crate::maps::dynamic_map::author_sources::TerrainAuthorRes<super::DynamicBladeDraft>, // 未保存Blade/Mesh层直接使用内存图片，不请求未生成KTX2。
    terrain_cache: InstanceRes<DynamicTerrainHeightCache>, // 定义采用前取得同一完整CPU/GPU地形派生值。
    clipmap_pages: Option<InstanceRes<crate::maps::dynamic_map::clipmap::SparseHeightPageRuntime>>, // 旧图测试可以不安装分页域。
    clipmap_geometry: Option<InstanceRes<crate::maps::dynamic_map::sparse_runtime::SparseClipmapRuntime>>, // 唯一正式几何。
    displays: InstanceQuery<
        &Transform,
        With<crate::maps::dynamic_map::sparse_runtime::SparseClipmapDisplay>,
    >, // 真实显示中心。
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    if !state.dirty {
        return;
    }
    if state.task.is_some() {
        status.phase = DynamicVegetationPhase::Waiting;
        status.detail = "植被输入已更新；等待唯一旧后台作业结束后捕获最新修订".to_owned();
        return;
    }
    if let Some(staging) = state.mesh_chunk_staging.take() {
        commands.entity(staging.root).try_despawn();
    }
    if let Some(candidate) = state.candidate_root.take() {
        commands.entity(candidate.root).try_despawn(); // 新修订开始前回收尚未共同提交的旧候选。
    }
    state.candidate_mesh_source = None;
    state.queued_mesh_chunks.clear(); // 新定义准备期间停止为旧源继续排队空间块。
    state.desired_mesh_chunks.clear(); // 新源采用后从当前相机和画质重新计算完整期望集合。
    state.residency_focus_chunk = None; // 强制新修订首帧建立空间常驻真值。
    state.residency_range_bits = None; // 强制新修订重新读取当前场景细节档。
    let Some(map_id) = active_map.map_id.as_ref() else {
        fail_current_revision(
            &mut commands,
            &mut state,
            &mut status,
            "尚未选择动态地图，无法加载植被".to_owned(),
        );
        return;
    };
    if state.map_id.as_ref() != Some(map_id) {
        if let Some(root) = state.active_root.take() {
            commands.entity(root).try_despawn();
        }
        if let Some(candidate) = state.candidate_root.take() {
            commands.entity(candidate.root).try_despawn();
        }
        state.candidate_mesh_source = None;
        state.mesh_source = None; // 切图后旧密度与高度快照不得驱动新地图块任务。
        state.resident_mesh_chunks.clear(); // 旧块实体随父根回收，轻量身份表同步清空。
        // 已退役根自动归还本根绘制槽；其他网格/子地图的完整目录继续有效。
        state.generation = state.generation.wrapping_add(1).max(1);
        state.revision = 1;
        state.map_id = Some(map_id.clone());
        state.pending = None;
        state.definition_handle = None;
        state.attempted_revision = None;
    }
    status.map_id = Some(map_id.clone());
    let Some(registry) = registry.as_deref() else {
        status.phase = DynamicVegetationPhase::Waiting;
        status.detail = "地图目录仍在加载".to_owned();
        return;
    };
    state.catalog_revision = registry.revision;
    let Some(preset_registry) = preset_registry.as_deref() else {
        status.phase = DynamicVegetationPhase::Waiting;
        status.detail = "开发者植被预设目录仍在加载".to_owned();
        return;
    };
    if preset_registry.revision == 0 {
        status.phase = DynamicVegetationPhase::Waiting;
        status.detail = preset_registry
            .last_error
            .clone()
            .unwrap_or_else(|| "开发者植被预设目录仍在加载".to_owned());
        return; // 首份完整目录采用前不能把空快照解释为所有地图预设缺失。
    }
    state.preset_catalog_revision = preset_registry.revision; // 当前待构建修订冻结成功目录版本。
    let Some(handles) = registry.handles(map_id) else {
        status.phase = DynamicVegetationPhase::Waiting;
        status.detail = format!("地图{map_id}尚未出现在目录");
        return;
    };
    let draft_snapshot = draft.for_map(map_id); // 未登记磁盘植被的新图也能直接消费完整草稿。
    let definition_handle = handles.vegetation.as_ref(); // 正式定义可选，不是草稿采用的前置条件。
    if definition_handle.is_none() && draft_snapshot.is_none() {
        if let Some(root) = state.active_root.take() {
            commands.entity(root).try_despawn();
        }
        if let Some(candidate) = state.candidate_root.take() {
            commands.entity(candidate.root).try_despawn();
        }
        state.candidate_mesh_source = None;
        state.mesh_source = None; // 无植被地图不得保留上一地图CPU散布源。
        state.resident_mesh_chunks.clear(); // 父根回收后同步清除旧常驻块记录。
        state.queued_mesh_chunks.clear(); // 无植被地图没有任何待生成Mesh块。
        state.desired_mesh_chunks.clear(); // 无植被地图的空间期望集合为空。
        state.collision_focus_cell = None; // 下一张有植被地图必须由真实玩家位置重新建立物理近场。
        state.definition_handle = None;
        state.pending = None;
        state.dirty = false;
        status.phase = DynamicVegetationPhase::Ready;
        status.layer_count = 0;
        status.blade_layer_count = 0;
        status.blade_window_count = 0;
        status.mesh_instance_count = 0;
        status.mesh_batch_count = 0;
        status.mesh_render_node_count = 0;
        status.mesh_collision_proxy_count = 0;
        status.revision = status.revision.wrapping_add(1);
        status.detail = format!("地图{map_id}没有声明数据驱动植被");
        return;
    };
    state.definition_handle = definition_handle.cloned(); // 无磁盘定义时不伪造AssetServer句柄。
    let (Some(manifest), Some(definition)) = (
        manifests.get(&handles.manifest),
        definition_handle
            .and_then(|handle| definitions.get(handle))
            .or_else(|| {
                definition_handle
                    .is_none()
                    .then(|| draft_snapshot.map(|snapshot| &snapshot.definition))
                    .flatten()
            }), // 未声明来源只使用已完整发布的草稿。
    ) else {
        if definition_handle.is_some_and(|handle| {
            matches!(asset_server.load_state(handle.id()), LoadState::Failed(_))
        }) {
            fail_current_revision(
                &mut commands,
                &mut state,
                &mut status,
                format!("地图{map_id}植被JSON加载失败"),
            );
        } else {
            status.phase = DynamicVegetationPhase::Waiting;
            status.detail = format!("地图{map_id}植被JSON仍在加载");
        }
        return;
    };

    let effective_definition = draft_snapshot
        .map(|snapshot| snapshot.definition.clone())
        .unwrap_or_else(|| definition.clone()); // 未保存Blade与Mesh必须由同一作者修订完整替换磁盘来源。
    let definition = &effective_definition; // 以下校验与加载统一消费合成定义。
    if manifest.id != *map_id || definition.map_id != *map_id {
        fail_current_revision(
            &mut commands,
            &mut state,
            &mut status,
            format!("地图{map_id}清单或植被文件所属ID不一致"),
        );
        return;
    }
    if let Err(error) = validate_vegetation_definition(definition, manifest, preset_registry) {
        fail_current_revision(
            &mut commands,
            &mut state,
            &mut status,
            format!("地图{map_id}植被定义无效：{error}"),
        );
        return;
    }
    let (height_cells, heights, terrain_generation, clipmap_height) =
        if manifest.terrain.is_empty() && definition.layers.is_empty() {
            // 已登记的空植被文件仍是一份完整修订。空容器没有GLB高度，零层也不需要地面；
            // 继续正式后台构建和根采用流程，让旧植被在同代事务中退出，不能在此伪写Ready。
            (None, std::sync::Arc::from([]), 0, None)
        } else if manifest.terrain.kind == feathrise_map_content::MapTerrainKind::SparseClipmap {
            // 分页不派生整图缓存。
            let height = clipmap_geometry
                .as_deref()
                .and_then(|geometry| geometry.blade_geometry(map_id))
                .and_then(|(display, config)| {
                    // 真实已安装配置。
                    let transform = displays.get(display).ok()?; // Commands尚未提交则重试。
                    clipmap_pages.as_deref()?.blade_render_height(
                        map_id,
                        config,
                        Vec2::new(transform.translation.x, transform.translation.z),
                    ) // 与地表同中心。
                }); // 固定大小快照。
            let Some(height) = height else {
                // 首载等待本域资源。
                status.detail = "正在等待 Clipmap 植被共享高度页".to_owned(); // 不等待GLB。
                return; // 非阻塞重试。
            }; // Compute独立检验ready，不显示半成品。
            (None, std::sync::Arc::from([]), 0, Some(height)) // 无CPU全图高度。
        } else if manifest.terrain.kind == feathrise_map_content::MapTerrainKind::Voxel {
            // 原生Blade通过三维GPU源投射自己的XZ密度；Mesh单块读取已采用真实BVH。
            // 没有整图GLB高度，也不能构造一张隐式平地给普通窗口。
            (None, std::sync::Arc::from([]), 0, None)
        } else {
            // GLB沿用原严格校验。
            let Some(terrain_ready) = terrain_cache.target_for(map_id) else {
                status.phase = DynamicVegetationPhase::Waiting; // 首载等待GLB，热更保留上一完整植被根。
                status.detail = format!("地图{map_id}正在从GLB生成植被所需内存高度");
                return;
            };
            if terrain_ready.spec.resolution != manifest.terrain.resolution
                || terrain_ready.spec.world_min != manifest.world_min
                || terrain_ready.spec.world_max != manifest.world_max
                || terrain_ready.spec.base_height != manifest.terrain.base_height
                || terrain_ready.spec.height_scale != manifest.terrain.height_scale
            {
                fail_current_revision(
                    &mut commands,
                    &mut state,
                    &mut status,
                    format!("地图{map_id}植被取得的GLB派生高度与清单采样规格不一致"),
                );
                return;
            }
            (
                Some(terrain_ready.height_image.clone()),
                terrain_ready.heights.clone(),
                terrain_ready.generation,
                None,
            ) // 保留GLB完整同代源。
        }; // 两种高度后端到此合并。
    let mut pending = match load_pending_assets(
        &asset_server,
        definition.clone(),
        manifest.clone(),
        preset_registry,
        height_cells,
        heights,
        terrain_generation,
        state.generation,
        state.revision,
        draft_snapshot, // 强句柄和CPU密度来自同一完整草稿身份。
    ) {
        Ok(pending) => pending, // 完整目录解析和全部强句柄请求作为一个待采用输入集合。
        Err(error) => {
            fail_current_revision(
                &mut commands,
                &mut state,
                &mut status,
                format!("地图{map_id}植被预设解析失败：{error}"),
            );
            return;
        }
    };
    pending.clipmap_height = clipmap_height; // 冻结分页绑定，之后只轻量同步高度。
    state.pending = Some(pending); // 只有全部层和 Blade 批次都解析成功才发布当前修订。
    state.dirty = false;
    status.phase = DynamicVegetationPhase::Waiting;
    status.detail = format!(
        "地图{map_id}正在异步加载{}个植被层的KTX2；非零Mesh层将按需请求正式四级原型",
        definition.layers.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*; // 用正式准备系统复现无磁盘植被声明的新图。
    use feathrise_environment::vegetation::blade::GpuBladeProfileBanks;
    use feathrise_map_content::{MapContentHandles, MapId}; // 稳定注册表夹具。

    #[test]
    fn draft_without_disk_declaration_reaches_terrain_preparation() {
        let mut app = App::new(); // 无窗口验证完整定义入口。
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<MapManifestAsset>()
            .init_asset::<MapVegetationAsset>(); // 真实资产容器。
        let map = MapId::parse("MAP-unsaved-grass").unwrap(); // 用户文件之外的身份。
        let manifest: MapManifestAsset = serde_json::from_value(serde_json::json!({
            "schema_version":1, "id":map.as_str(), "display_name":"未保存草地", // 合法清单身份。
            "runtime_scene":"DynamicMapScene", "world_min":[0,0], "world_max":[10,10], // 有效世界范围。
            "default_spawn":[5,0,5], // 默认出生点。
            "terrain":{"kind":"heightfield", "glb_path":"maps/test/terrain.glb", "resolution":[2,2], "base_height":0, "height_scale":10} // 验证到地形等待门。
        }))
        .unwrap();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<MapManifestAsset>>()
            .add(manifest); // 完整清单已加载。
        app.insert_resource(ActiveDynamicMap {
            map_id: Some(map.clone()),
        }); // 当前游戏地图。
        app.insert_resource(feathrise_map_content::MapContentRegistry {
            catalog: default(), // 不需要磁盘目录句柄。
            revision: 1,        // 目录已采用。
            maps: [(
                map.clone(),
                MapContentHandles {
                    manifest: handle, // 唯一清单。
                    portals: default(),
                    placements: default(), // 本测试不请求其他地图内容。
                    vegetation: None,      // 故障关键：草稿已有定义，但目录尚未登记磁盘文件。
                    characters_path: None,
                    surface: None,
                    water: None,
                    boundaries: None, // 未启用的可选域。
                },
            )]
            .into(), // 单图注册表。
            last_error: None,   // 有效目录。
        });
        app.insert_resource(VegetationPresetRegistry {
            catalog: default(),
            revision: 1,
            resolved: default(),
            last_error: None, // 空Blade输出不需要模型预设。
        });
        let mut source = super::super::DynamicBladeDraft::default(); // 完整清空草地也是必须采用的修订。
        source.publish(super::super::DynamicBladeDraftSnapshot {
            source_revision: 7, // 最新作者修订。
            definition: MapVegetationAsset {
                schema_version: 1,
                map_id: map,
                layers: vec![],
                authoring: None,
            }, // 合法空输出。
            densities: default(),
            sources: default(),        // 无输出无需图片。
            nonzero_layers: default(), // 空输出没有Mesh原型请求。
        });
        app.insert_resource(source)
            .init_resource::<DynamicTerrainHeightCache>()
            .init_resource::<GpuBladeProfileBanks>()
            .init_resource::<DynamicVegetationStatus>(); // 故意没有完成地形。
        let mut state = DynamicVegetationState::default(); // 与真实加载一样请求一次准备。
        state.request_rebuild(); // 不能用默认dirty状态隐式假设。
        app.insert_resource(state)
            .add_systems(Update, prepare_dynamic_vegetation_definition); // 正式系统入口。
        app.update();
        let status = app.world().resource::<DynamicVegetationStatus>(); // 旧实现在缺handle时错误Ready=0。
        assert_eq!(status.phase, DynamicVegetationPhase::Waiting); // 草稿必须进入正式下游准备。
        assert!(status.detail.contains("GLB"), "{}", status.detail); // 证明已通过定义分支，等真实地形。
    }
}
