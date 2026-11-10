//! 动态水域当前输入资产的成功、失败与热更新事件仲裁。
//!
//! 【功能设计】本模块只识别当前场景的水JSON、清单和全部无Mip R8作者源。
//! 同一帧出现旧失败与新成功时，以成功事件或当前`LoadState::Loaded`为准重新构建，
//! 防止迟到失败把已经恢复的新代次覆盖成永久失败。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use bevy::asset::{Asset, AssetEvent, AssetId, AssetLoadFailedEvent, LoadState}; // 读取强类型资产事件与当前加载状态。
use bevy::prelude::*; // MessageReader和ECS资源参数。
use feathrise_scene_content::{
    ActiveDynamicScene, SceneManifestAsset, SceneWaterAsset,
    SceneWaterMaskAsset,
}; // 当前场景身份、目录句柄、水面作者输入与固定层预算。

use super::resources::{
    DynamicClipmapWaterDraft, DynamicWaterPhase, DynamicWaterRebuildRequest, DynamicWaterState, DynamicWaterStatus, TerrainWaterRebuildRequest,
}; // 资产事件和保存边界只推进唯一水事务的代次与诊断。

/// 判断一个事件是否表示指定强句柄已经产生新的可用内容。
fn event_reports_success<A: Asset>(
    event: &AssetEvent<A>, // 当前消息读取器交付的一次资产变化。
    id: AssetId<A>,        // 只接受当前事务持有的稳定资产身份。
) -> bool {
    event.is_added(id) // 首次异步加载成功需要启动首批构建。
        || event.is_modified(id) // 文件热更新成功需要作废旧批次。
        || event.is_loaded_with_dependencies(id) // 依赖完整后允许从失败或等待状态恢复。
}

/// 当前强句柄已经重新Loaded时，迟到的旧失败不得覆盖恢复结果。
fn asset_is_loaded<A: Asset>(
    asset_server: &AssetServer, // 查询同一AssetId此刻的权威加载状态。
    id: AssetId<A>,             // 失败消息声称失败的当前强句柄。
) -> bool {
    matches!(asset_server.load_state(id), LoadState::Loaded) // 成功状态优先触发重试。
}

/// 只有身份未换代、同帧无成功且当前仍未Loaded时，失败才具有发布资格。
fn should_publish_failure(
    identity_superseded: bool, // 场景、目录或定义变化说明该失败可能属于旧输入集合。
    succeeded_this_frame: bool, // 同一当前句柄的新成功事件比迟到失败更新。
    currently_loaded: bool,    // 即使成功事件已被其他读取时机消费，LoadState仍能证明恢复。
) -> bool {
    !identity_superseded && !succeeded_this_frame && !currently_loaded // 三道门共同拒绝旧失败竞态。
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TerrainWaterRebuilds<'w, 's> {
    requests: MessageReader<'w, 's, TerrainWaterRebuildRequest>,
    scope: Res<'w, feathrise_scene_core::residency::SceneRuntimeScope>,
    objects: Query<'w, 's, (&'static super::super::instances::TerrainObjectInstance, &'static super::super::instances::SubsceneInstance)>,
}
impl TerrainWaterRebuilds<'_, '_> {
    fn requested_for(&mut self, map: Option<&feathrise_scene_content::SceneId>) -> bool {
        let object = self.scope.instance.and_then(|entity| self.objects.get(entity).ok());
        // 所有读者都消费完整消息集合，父容器和同图兄弟对象不能借SceneId认领本次新代次。
        self.requests.read().fold(false, |matched, request| matched
            || map == Some(&request.scene_id)
                && object.is_some_and(|(object, instance)| object.definition.id == request.terrain_id
                    && instance.path.root_scene_id == request.scene_id && instance.path.instances.is_empty()))
    }
}

/// 只观察当前动态场景水面所依赖的资产；同帧多个变化合并为一个新代次。
#[allow(clippy::too_many_arguments)]
pub(crate) fn observe_dynamic_water_assets(
    asset_server: Res<AssetServer>, // 用当前LoadState仲裁迟到失败与新成功。
    active_scene: InstanceRes<ActiveDynamicScene>, // 切换场景立即使旧水面输入代次失效。
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>, // 取得当前清单和水JSON的唯一强句柄。
    root_draft: Res<DynamicClipmapWaterDraft>,
    frame: feathrise_scene_core::residency::InstanceFrame,
    mut rebuild_requests: MessageReader<DynamicWaterRebuildRequest>, // 保存发布输入前强制建立不可合并的新代次。
    mut terrain_rebuilds: TerrainWaterRebuilds,
    mut manifest_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneManifestAsset>, // 世界范围、高度参数或路径变化会重建全部层。
    mut water_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneWaterAsset>, // 水层结构、水位和遮罩路径变化会重建全部层。
    mut mask_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneWaterMaskAsset>, // 任一当前R8作者源变化都会作废整批候选。
    mut manifest_failures: MessageReader<AssetLoadFailedEvent<SceneManifestAsset>>, // 当前清单失败必须作废在途候选。
    mut water_failures: MessageReader<AssetLoadFailedEvent<SceneWaterAsset>>, // 当前水JSON失败保留上一有效批次。
    mut mask_failures: MessageReader<AssetLoadFailedEvent<SceneWaterMaskAsset>>, // 当前任一R8作者源失败拒绝部分水面。
    mut state: InstanceResMut<DynamicWaterState>, // 唯一single-flight状态合并全部事件。
    mut status: InstanceResMut<DynamicWaterStatus>, // 对F3公开最终失败项而不刷稳定帧日志。
) {
    let rebuild_requested = rebuild_requests.read().fold(false, |matched, request|
        matched || active_scene.scene_id.as_ref() == Some(&request.scene_id))
        | terrain_rebuilds.requested_for(active_scene.scene_id.as_ref()); // 两种消息都必须完整消费，不能短路遗留旧请求。
    let target_changed = state.desired_scene_id != active_scene.scene_id; // 同一GameScene内切换场景也必须清掉旧输入身份。
    if target_changed {
        state.desired_scene_id = active_scene.scene_id.clone(); // 先切换目标，后续只匹配新场景目录句柄。
        status.applied_generation = 0; // 切换场景立即撤销上一场景的保存交接完成证据。
        state.mask_handles.clear();
    state.mask_asset_ids.clear(); // 旧图R8失败事件从本帧起不再属于当前事务。
        state.mask_paths.clear(); // 新水JSON重新建立稳定层到作者源路径映射。
        state.terrain_generation = None; // 旧图GLB派生高度不得参与新场景深度核心。
    }
    let registry_changed = registry
        .as_deref()
        .is_some_and(|registry| registry.revision != state.catalog_revision); // 目录换代可能替换清单或水JSON句柄。
    if let Some(registry) = registry.as_deref()
        && registry_changed
    {
        state.catalog_revision = registry.revision; // 立即消费目录修订，稳定帧不重复标脏。
    }

    let current_handles = state.desired_scene_id.as_ref().and_then(|scene_id| {
        registry.as_deref()?.handles(scene_id) // 只从当前场景目录条目取得资产身份。
    });
    let manifest_id = current_handles.map(|handles| handles.manifest.id()); // 清单总是目录必需资产。
    let water_id = current_handles
        .and_then(|handles| handles.water.as_ref())
        .map(Handle::id); // 未声明水域时没有可匹配的水JSON失败。
    let manifest_succeeded = manifest_events.read().fold(false, |matched, event| {
        matched || manifest_id.is_some_and(|id| event_reports_success(event, id)) // 消费整批事件并只接受当前清单。
    });
    let water_succeeded = water_events.read().fold(false, |matched, event| {
        matched || water_id.is_some_and(|id| event_reports_success(event, id)) // 消费整批事件并只接受当前水JSON。
    });
    let mut successful_mask_ids = std::collections::HashSet::new();
    for event in mask_events.read() {
        let id = match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => *id,
            _ => continue,
        };
        if state.mask_asset_ids.contains(&id) { successful_mask_ids.insert(id); }
    }
    let definition_superseded =
        target_changed || registry_changed || manifest_succeeded || water_succeeded; // 新定义可能替换全部R8身份和高度采样规格。
    let mut recovered_current_asset = false; // LoadState已Loaded但成功消息和失败同帧乱序时仍需重试。
    let mut failure_detail = None; // 一帧只发布并记录第一个当前权威失败，避免重复刷日志。

    for failure in manifest_failures.read() {
        if manifest_id != Some(failure.id) {
            continue; // 其他场景或旧目录清单失败与当前水面无关。
        }
        if should_publish_failure(
            target_changed || registry_changed, // 新目录身份优先于旧清单失败。
            manifest_succeeded,                 // 同一清单本帧成功优先。
            asset_is_loaded(&asset_server, failure.id), // 当前已Loaded证明失败消息迟到。
        ) {
            failure_detail.get_or_insert_with(|| {
                format!(
                    "动态场景清单加载失败，无法构建水面：{} ({})",
                    failure.path, failure.error
                ) // 保留路径和加载器错误供F3定位。
            });
        } else {
            recovered_current_asset = true; // 下一批从当前成功状态重新冻结输入。
        }
    }
    for failure in water_failures.read() {
        if water_id != Some(failure.id) {
            continue; // 旧水JSON强句柄失败不得污染当前场景。
        }
        if frame.instance().is_none() && active_scene.scene_id.as_ref().is_some_and(|map| root_draft.root_for_scene(map).is_some()) {
            continue; // 当前完整独立作者网已取代正式文件；保存撤回根槽后才重新要求正式文件就绪。
        }
        if should_publish_failure(
            target_changed || registry_changed, // 目录换代后旧水句柄失败作废。
            water_succeeded,                    // 当前水JSON成功事件优先。
            asset_is_loaded(&asset_server, failure.id), // 当前加载状态作为最终恢复证据。
        ) {
            failure_detail.get_or_insert_with(|| {
                format!(
                    "动态场景水域JSON加载失败：{} ({})",
                    failure.path, failure.error
                ) // JSON失败不回退到部分层或旧输入重建。
            });
        } else {
            recovered_current_asset = true; // 成功代次需要重新提交一次完整批次。
        }
    }
    for failure in mask_failures.read() {
        if !state.mask_asset_ids.contains(&failure.id)
        {
            continue; // 不匹配当前层表的R8失败可能属于旧水层或已切换的作者源。
        }
        let same_mask_succeeded =
            successful_mask_ids.contains(&failure.id); // 精确到失败的同一R8句柄。
        if should_publish_failure(
            definition_superseded, // 新清单或水JSON可能已经移除该旧R8路径。
            same_mask_succeeded,   // 同一R8本帧成功优先。
            asset_is_loaded(&asset_server, failure.id), // 当前Loaded状态拒绝迟到失败。
        ) {
            failure_detail.get_or_insert_with(|| {
                format!(
                    "动态场景水域R8作者源加载失败：{} ({})",
                    failure.path, failure.error
                ) // 任一遮罩失败都拒绝原子批次。
            });
        } else {
            recovered_current_asset = true; // 新定义或恢复后的R8触发最新批次。
        }
    }
    if let Some(detail) = failure_detail {
        state.invalidate_failed_generation(&mut status); // 当前真实失败立即使在途旧快照失去采用资格并公开新期望代次。
        status.phase = DynamicWaterPhase::Failed; // 首载保持无水，热更继续保留上一完整批次。
        status.detail = detail; // 只发布一次当前失败诊断。
        error!("{}", status.detail); // 稳定帧不会重复进入此事件分支。
        return; // 等后续成功事件或Loaded恢复证据重新标脏。
    }
    if rebuild_requested {
        state.begin_rebuild_transaction(&mut status); // 即使旧任务在途或dirty已真，保存也取得严格更新的代次边界。
    } else if target_changed
        || registry_changed
        || manifest_succeeded
        || water_succeeded
        || !successful_mask_ids.is_empty()
        || recovered_current_asset
    {
        state.mark_dirty(&mut status); // 同帧全部成功与身份变化合并为唯一新generation并公开交接证据。
    }
}

#[cfg(test)]
mod tests {
    use super::should_publish_failure; // 纯逻辑锁定失败与恢复消息乱序语义。

    #[test]
    fn current_success_or_loaded_state_wins_over_stale_failure() {
        assert!(!should_publish_failure(false, true, false)); // 同帧成功事件不得被旧失败覆盖。
        assert!(!should_publish_failure(false, false, true)); // 当前已Loaded同样证明失败迟到。
        assert!(!should_publish_failure(true, false, false)); // 场景或定义换代后的旧失败不再权威。
        assert!(should_publish_failure(false, false, false)); // 只有当前仍失败的句柄才发布失败。
    }

    #[test]
    fn saved_terrain_rebuild_is_routed_only_to_the_exact_root_object() {
        use super::*;
        use bevy::ecs::system::RunSystemOnce;
        use feathrise_scene_content::{SceneId, SceneInstancePath, TerrainId};
        use crate::scenes::dynamic_scene::instances::{SubsceneInstance, TerrainObjectInstance};
        #[derive(Resource)]
        struct Accepted(bool);
        fn read(mut requests: TerrainWaterRebuilds, mut accepted: ResMut<Accepted>, active: Res<ActiveDynamicScene>) {
            accepted.0 = requests.requested_for(active.scene_id.as_ref());
        }
        let map = SceneId::parse("MAP-water-rebuild").unwrap();
        let other_scene = SceneId::parse("MAP-water-rebuild-other").unwrap();
        let a = TerrainId::parse("water-a").unwrap();
        let b = TerrainId::parse("water-b").unwrap();
        let mut world = World::new();
        world.init_resource::<Messages<TerrainWaterRebuildRequest>>();
        world.insert_resource(Accepted(false));
        world.insert_resource(ActiveDynamicScene { scene_id: Some(map.clone()), ..default() });
        world.init_resource::<feathrise_scene_core::residency::SceneRuntimeScope>();
        let definition = feathrise_scene_content::SceneTerrainObject { id: a.clone(), display_name: "水画布".into(), enabled: true,
            clipmap_stitches: default(), mesh_lod: None, transform: default(), world_min: [-1.; 2], world_max: [1.; 2],
            terrain: feathrise_scene_content::SceneTerrainDescriptor::empty(), voxel: None, surface_path: None,
            vegetation_path: None, water_path: None, puddles: None };
        let batch = world.spawn_empty().id();
        let entity = world.spawn((TerrainObjectInstance { definition: std::sync::Arc::new(definition.clone()) },
            SubsceneInstance { path: SceneInstancePath::root(map.clone()).terrain(a.clone()), source_scene_id: map.clone(), generation: 1, batch })).id();
        world.resource_mut::<feathrise_scene_core::residency::SceneRuntimeScope>().instance = Some(entity);
        for (requested_scene, requested_id, accepted) in [(map.clone(), b, false), (other_scene, a.clone(), false), (map.clone(), a.clone(), true)] {
            world.resource_mut::<Messages<TerrainWaterRebuildRequest>>().clear();
            world.write_message(TerrainWaterRebuildRequest { scene_id: requested_scene, terrain_id: requested_id });
            world.run_system_once(read).unwrap();
            assert_eq!(world.resource::<Accepted>().0, accepted);
        }
        world.resource_mut::<feathrise_scene_core::residency::SceneRuntimeScope>().instance = None;
        world.run_system_once(read).unwrap();
        assert!(!world.resource::<Accepted>().0, "根容器不能借同图对象的定点请求推进代次");
        world.resource_mut::<feathrise_scene_core::residency::SceneRuntimeScope>().instance = Some(entity);
        world.get_mut::<SubsceneInstance>(entity).unwrap().path.root_scene_id = SceneId::parse("MAP-elsewhere").unwrap();
        world.run_system_once(read).unwrap();
        assert!(!world.resource::<Accepted>().0, "其他根场景引用同一来源也不能消费本次根画布的保存请求");
    }
}
