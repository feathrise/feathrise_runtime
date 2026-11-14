//! 传送图事件观察、跨文件解析与原子实体替换。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use bevy::asset::{Asset, DependencyLoadState, LoadState, RecursiveDependencyLoadState}; // 只对当前传送闭包检查直接、依赖和递归失败。
use bevy::prelude::*; // ECS查询、资产集合、颜色与日志。
use feathrise_scene_content::{
    ActiveDynamicScene, SceneBoundariesAsset, SceneContentRegistry, SceneId, SceneManifestAsset,
    ScenePortalRecord, ScenePortalsAsset,
}; // 运行时只读取编辑器共享作者数据。
use feathrise_scene_core::state::ScenePhase; // 静态场景清单绑定到当前正式场景枚举。

use super::resources::{ScenePortalRuntimePhase, ScenePortalRuntimeState, ScenePortalRuntimeStatus};

/// 一次完整解析后可直接生成的传送门计划。
struct ResolvedPortal {
    position: Vec3,
    scale:Vec3,
    target_scene: ScenePhase,
    target_scene_id: String,
    target_display_name: String,
    target_spawn: Vec3,
    label: String,
    color: Color,
    arrow_degrees: f32,
}

#[derive(Component)]
pub(crate) struct LocalPortalArrival(Vec3); // 自引用目标保留源局部坐标，跟随当前实例整体变换。

pub(crate) fn follow_local_portal_arrivals(frame: feathrise_scene_core::residency::InstanceFrame,
    mut portals: feathrise_scene_core::residency::InstanceQuery<(&LocalPortalArrival, &mut feathrise_portal::Portal)>) {
    for (local, mut portal) in &mut portals {
        let target = frame.to_world(local.0);
        if portal.spawn_position != target { portal.spawn_position = target; }
    }
}

/// 把当前静态玩法场景映射到作者文件使用的稳定场景ID。
fn active_scene_id(scene: ScenePhase, active_dynamic_scene: &ActiveDynamicScene) -> Option<SceneId> {
    match scene {
        ScenePhase::Active => active_dynamic_scene.scene_id.clone(),
        ScenePhase::Idle | ScenePhase::Loading => None,
    }
}

/// 把场景清单中的运行时绑定转换成现有正式场景。
fn runtime_scene(value: &str) -> Option<ScenePhase> {
    feathrise_scene_content::SceneRuntimeKind::parse(value).map(|kind| match kind {
        feathrise_scene_content::SceneRuntimeKind::DataDriven => ScenePhase::Active,
    })
}

/// 校验有限三维坐标并转换为Bevy向量。
fn finite_vec3(value: [f32; 3], context: &str) -> Result<Vec3, String> {
    let value = Vec3::from_array(value);
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| format!("{context}包含非有限坐标"))
}

/// 场景容器不限制传送位置；仍拒绝非有限坐标进入正式传送图。
fn validate_position_in_scene(
    manifest: &SceneManifestAsset,
    position: Vec3,
    context: &str,
) -> Result<(), String> {
    position
        .is_finite()
        .then_some(())
        .ok_or_else(|| format!("{context}在场景{}中的位置无效", manifest.id))
}

/// 以当前主区域边界为正式方向权威；只有旧场景没有可用边界时才采用序列化兼容快照。
fn resolved_portal_arrow_degrees(
    position: Vec3,
    stored_degrees: f32,
    boundaries: Option<&SceneBoundariesAsset>,
) -> f32 {
    boundaries
        .and_then(|boundaries| boundaries.nearest_main_boundary_direction_degrees(position.xz()))
        .unwrap_or(stored_degrees)
}

/// 查找目标场景传送点并返回其安全到达坐标。
#[cfg(test)]
fn resolve_target_spawn(
    record: &ScenePortalRecord,
    registry: &SceneContentRegistry,
    manifests: &Assets<SceneManifestAsset>,
    portal_assets: &Assets<ScenePortalsAsset>,
    author_preview: Option<&feathrise_scene_content::SceneAuthorPreview>,
) -> Result<(ScenePhase, String, Vec3), String> {
    resolve_target_spawn_bound(record,registry,manifests,portal_assets,author_preview,&mut |_,_|Err("关联地形索引仍在加载".into()))
}
fn resolve_target_spawn_bound(
    record:&ScenePortalRecord,registry:&SceneContentRegistry,manifests:&Assets<SceneManifestAsset>,portal_assets:&Assets<ScenePortalsAsset>,
    author_preview:Option<&feathrise_scene_content::SceneAuthorPreview>,
    resolve_binding:&mut impl FnMut(&SceneManifestAsset,&feathrise_scene_content::SceneTerrainBinding)->Result<feathrise_scene_content::TerrainObjectTransform,String>,
) -> Result<(ScenePhase,String,Vec3),String> {
    let target_handles = registry
        .handles(&record.target.scene_id)
        .ok_or_else(|| format!("目标场景{}不在场景目录", record.target.scene_id))?;
    let target_manifest = manifests
        .get(&target_handles.manifest)
        .ok_or_else(|| format!("目标场景{}清单仍在加载", record.target.scene_id))?;
    if target_manifest.id != record.target.scene_id {
        return Err(format!(
            "目标场景清单声明{}，但传送引用{}",
            target_manifest.id, record.target.scene_id
        )); // 目的地名称和坐标只能从同一SceneId清单原子采用。
    }
    let target_scene = runtime_scene(&target_manifest.runtime_scene).ok_or_else(|| {
        format!(
            "目标场景{}运行时场景{}尚未发布",
            record.target.scene_id, target_manifest.runtime_scene
        )
    })?;
    let target_display_name = target_manifest.display_name.trim(); // 目标清单是目的地名称的唯一权威来源。
    if target_display_name.is_empty() {
        return Err(format!("目标场景{}显示名称为空", record.target.scene_id)); // 空名称不能进入加载页形成空白标题。
    }
    let arrival = if let Some(portal_id) = record.target.portal_id.as_deref() {
        if portal_id.trim().is_empty() {
            return Err(format!("目标场景{}包含空传送点ID", record.target.scene_id)); // 空字符串不是默认出生点，避免损坏旧文件被静默改义。
        }
        let target_handle = author_preview
            .and_then(|preview| preview.for_scene(&record.target.scene_id))
            .map(|preview| &preview.portals)
            .unwrap_or(&target_handles.portals);
        let target_portals = portal_assets
            .get(target_handle)
            .ok_or_else(|| format!("目标场景{}传送图仍在加载", record.target.scene_id))?;
        if target_portals.scene_id != record.target.scene_id {
            return Err(format!(
                "目标传送图声明场景{}，但目录请求{}",
                target_portals.scene_id, record.target.scene_id
            ));
        }
        let target = target_portals
            .portals
            .iter()
            .find(|candidate| candidate.id == portal_id)
            .ok_or_else(|| format!("目标场景{}不存在传送点{}", record.target.scene_id, portal_id))?;
        let arrival=target.arrival_terrain_binding.as_ref().map(|binding|resolve_binding(target_manifest,binding)).transpose()?.map_or(target.arrival,|pose|pose.translation);
        finite_vec3(arrival, &format!("目标传送点{}到达位置", target.id))?
    } else {
        finite_vec3(
            target_manifest.default_spawn,
            &format!("目标场景{}默认起始位置", record.target.scene_id),
        )? // 默认出生点是场景清单的显式安全落点，不使用几何中心猜测高度。
    };
    validate_position_in_scene(target_manifest, arrival, "目标到达位置")?;
    Ok((target_scene, target_display_name.to_owned(), arrival)) // 名称与出生点在同一次完整解析中采用。
}

/// 完整解析当前场景传送图；任一目标未就绪或损坏时不返回部分计划。
fn resolve_current_scene_bound(
    scene_id:&SceneId,registry:&SceneContentRegistry,manifests:&Assets<SceneManifestAsset>,portal_assets:&Assets<ScenePortalsAsset>,boundary_assets:&Assets<SceneBoundariesAsset>,
    author_preview:Option<&feathrise_scene_content::SceneAuthorPreview>,
    resolve_binding:&mut impl FnMut(&SceneManifestAsset,&feathrise_scene_content::SceneTerrainBinding)->Result<feathrise_scene_content::TerrainObjectTransform,String>,
) -> Result<Vec<ResolvedPortal>,String> {
    let handles = registry
        .handles(scene_id)
        .ok_or_else(|| format!("场景{scene_id}尚未出现在场景目录"))?;
    let manifest = manifests
        .get(&handles.manifest)
        .ok_or_else(|| format!("场景{scene_id}清单仍在加载"))?;
    if manifest.id != *scene_id {
        return Err(format!("场景清单声明{}，但目录请求{scene_id}", manifest.id));
    }
    let preview = author_preview.and_then(|preview| preview.for_scene(scene_id));
    let portal_handle = preview
        .map(|preview| &preview.portals)
        .unwrap_or(&handles.portals);
    let portals = portal_assets
        .get(portal_handle)
        .ok_or_else(|| format!("场景{scene_id}传送图仍在加载"))?;
    if portals.scene_id != *scene_id {
        return Err(format!("传送图声明{}，但目录请求{scene_id}", portals.scene_id));
    }
    let boundary_handle = preview
        .map(|preview| preview.boundaries.as_ref())
        .unwrap_or(handles.boundaries.as_ref());
    let boundaries = if let Some(boundary_handle) = boundary_handle {
        let boundaries = boundary_assets
            .get(boundary_handle)
            .ok_or_else(|| format!("场景{scene_id}区域边界仍在加载"))?;
        if boundaries.scene_id != *scene_id {
            return Err(format!(
                "区域边界声明{}，但传送方向请求{scene_id}",
                boundaries.scene_id
            )); // 箭头不能从另一个场景的几何推导方向。
        }
        Some(boundaries)
    } else {
        None // 没有边界文件的旧静态场景继续使用已保存作者角度。
    };
    let mut ids = std::collections::BTreeSet::new();
    let mut resolved = Vec::with_capacity(portals.portals.len());
    for record in &portals.portals {
        if record.id.trim().is_empty() || !ids.insert(record.id.as_str()) {
            return Err(format!("场景{scene_id}包含空传送点ID或重复ID：{}", record.id));
        }
        let pose=record.terrain_binding.as_ref().map(|binding|resolve_binding(manifest,binding)).transpose()?;
        let position = finite_vec3(pose.map_or(record.position,|pose|pose.translation), &format!("传送点{}位置", record.id))?;
        validate_position_in_scene(manifest, position, &format!("传送点{}位置", record.id))?;
        let (target_scene, target_display_name, target_spawn) =
            resolve_target_spawn_bound(record, registry, manifests, portal_assets, author_preview,resolve_binding)?;
        let label = feathrise_portal::portal_display_label(
            &record.label,           // 作者名称允许表达王府、河道或乡村等目标区块。
            record.label_customized, // 未自定义的新点仍随目标场景显示名生成默认名称。
            &target_display_name,    // 自动默认名读取本次原子解析的目标清单名称。
        );
        if (record.label_customized && record.label.trim().is_empty())
            || !record.arrow_degrees.is_finite()
            || record
                .color_srgba
                .iter()
                .any(|channel| !channel.is_finite())
        {
            return Err(format!("传送点{}名称、颜色或方向无效", record.id));
        }
        resolved.push(ResolvedPortal {
            position,
            scale:pose.map_or(Vec3::ONE,|pose|Vec3::from_array(pose.scale)),
            target_scene,
            target_scene_id: record.target.scene_id.as_str().to_owned(),
            target_display_name, // 保存已经受检的目标清单名称供传送组件使用。
            target_spawn,
            label, // 正式Billboard、实体诊断和碰撞日志统一显示“前往{目标场景名}”。
            color: Color::srgba(
                record.color_srgba[0],
                record.color_srgba[1],
                record.color_srgba[2],
                record.color_srgba[3],
            ),
            arrow_degrees: resolved_portal_arrow_degrees(
                position,
                pose.map_or(record.arrow_degrees,|pose|pose.yaw_degrees),
                boundaries,
            ), // 主边界是新方向权威；缺失或不完整时才兼容旧静态作者角度。
        });
    }
    Ok(resolved)
}

fn handle_failed<A: Asset>(asset_server: &AssetServer, handle: &Handle<A>) -> bool {
    asset_server
        .get_load_states(handle.id())
        .is_some_and(|(direct, dependencies, recursive)| {
            matches!(direct, LoadState::Failed(_))
                || matches!(dependencies, DependencyLoadState::Failed(_))
                || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
        })
}

/// 只检查当前场景及其显式目标的清单、传送图和方向边界，其他目录场景损坏不得污染当前状态。
fn current_portal_dependency_failure(
    scene_id: &SceneId,
    registry: &SceneContentRegistry,
    portal_assets: &Assets<ScenePortalsAsset>,
    asset_server: &AssetServer,
    author_preview: Option<&feathrise_scene_content::SceneAuthorPreview>,
) -> Option<String> {
    let handles = registry.handles(scene_id)?;
    let preview = author_preview.and_then(|preview| preview.for_scene(scene_id));
    let portal_handle = preview
        .map(|preview| &preview.portals)
        .unwrap_or(&handles.portals);
    if handle_failed(asset_server, &handles.manifest) || handle_failed(asset_server, portal_handle)
    {
        return Some(format!("场景{scene_id}清单或传送图加载失败"));
    }
    if preview
        .map(|preview| preview.boundaries.as_ref())
        .unwrap_or(handles.boundaries.as_ref())
        .is_some_and(|handle| handle_failed(asset_server, handle))
    {
        return Some(format!("场景{scene_id}区域边界加载失败，无法更新传送方向")); // 热更失败保留上一完整传送实体。
    }
    let portals = portal_assets.get(portal_handle)?;
    for record in &portals.portals {
        let Some(target) = registry.handles(&record.target.scene_id) else {
            continue;
        };
        let target_portals_failed = record.target.portal_id.is_some()
            && handle_failed(
                asset_server,
                author_preview
                    .and_then(|preview| preview.for_scene(&record.target.scene_id))
                    .map(|preview| &preview.portals)
                    .unwrap_or(&target.portals),
            ); // 自引用时也采用同一完整临时传送图。
        if handle_failed(asset_server, &target.manifest) || target_portals_failed {
            return Some(format!(
                "目标场景{}清单或传送图加载失败",
                record.target.scene_id
            ));
        }
    }
    None
}

/// 资产事件仅设置脏标记；稳定帧不遍历世界或重复解析JSON。
pub(crate) fn observe_portal_data_events(
    registry: Option<Res<SceneContentRegistry>>, // 跨场景关系订阅完整目录，不采用只含当前容器的地形投影视图。
    mut manifest_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneManifestAsset>,
    mut portal_events: feathrise_scene_core::residency::ResidentAssetEventReader<ScenePortalsAsset>,
    mut boundary_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneBoundariesAsset>,
    mut state: InstanceResMut<ScenePortalRuntimeState>,
    author_preview: Option<crate::scenes::dynamic_scene::author_sources::TerrainAuthorRes<feathrise_scene_content::SceneAuthorPreview>>,
    terrain:(Option<Res<feathrise_scene_content::SceneInstanceGraph>>,Option<Res<crate::SubsceneRuntimeStatus>>),
) {
    let manifest_changed = manifest_events.read().next().is_some(); // 各自消费事件，避免短路后把其他类型积压到下一帧。
    let portals_changed = portal_events.read().next().is_some();
    let boundaries_changed = boundary_events.read().next().is_some();
    let data_changed = manifest_changed || portals_changed || boundaries_changed; // 区域边界换代时同批重新推导全部传送箭头。
    let registry_changed = registry
        .as_deref()
        .is_some_and(|registry| registry.revision != state.catalog_revision);
    let preview = author_preview
        .as_ref()
        .and_then(|preview| preview.active.as_ref())
        .map(|preview| {
            (
                preview.scene_id.clone(),
                preview.portals.id(),
                preview.boundaries.as_ref().map(Handle::id),
            )
        });
    let preview_changed = state.observed_preview != preview;
    state.observed_preview = preview;
    let adopted=terrain.0.as_deref().zip(terrain.1.as_deref()).filter(|(graph,status)|status.ready&&status.applied_revision==graph.revision)
        .and_then(|(graph,status)|graph.root_scene_id.clone().map(|map|(map,status.applied_revision)));
    let terrain_changed=adopted.is_some()&&state.observed_terrain_revision!=adopted;
    if adopted.is_some() {state.observed_terrain_revision=adopted;}
    if data_changed || registry_changed || preview_changed || terrain_changed {
        state.dirty = true;
    }
}

/// 玩法场景离开后撤销上一场景会话身份；作用域实体由`DespawnOnExit`统一回收。
pub(crate) fn invalidate_portal_runtime_on_scene_exit(
    mut state: InstanceResMut<ScenePortalRuntimeState>,
    mut status: InstanceResMut<ScenePortalRuntimeStatus>,
) {
    state.dirty = true; // 即使随后重新进入同一个动态SceneId，也必须重新生成已经离场销毁的传送实体。
    state.active_scene = None;
    state.active_scene_id = None;
    state.spawned_entities.clear(); // 这里只清除已由场景作用域回收的陈旧Entity身份，不逐项重复销毁。
    status.phase = ScenePortalRuntimePhase::Waiting;
    status.scene_id = None;
    status.portal_count = 0;
    status.detail = "场景切换中，等待目标场景传送图".to_owned();
}

/// 当前场景身份、传送资产或主边界改变时完整校验，并在成功后一次替换关系根与文字锚点集合。
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_data_driven_portals(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    current_scene: (InstanceRes<State<ScenePhase>>, feathrise_scene_core::residency::InstanceFrame,
        Option<Res<State<ScenePhase>>>, Option<Res<feathrise_scene_content::SceneInstanceRootRequest>>,
        Option<Res<feathrise_scene_content::SceneInstanceGraph>>,Option<Res<crate::SubsceneRuntimeStatus>>),
    registry: Option<Res<SceneContentRegistry>>, // 传送目标属于全局场景目录；源点姿态仍由当前实例frame解释。
    manifests: Res<Assets<SceneManifestAsset>>,
    portal_assets: Res<Assets<ScenePortalsAsset>>,
    boundary_assets: Res<Assets<SceneBoundariesAsset>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    visual_meshes: Option<Res<feathrise_portal::PortalVisualMeshes>>, // PortalPlugin启动后提供全场景共享固定网格。
    active_dynamic_scene: InstanceRes<ActiveDynamicScene>,
    mut state: InstanceResMut<ScenePortalRuntimeState>,
    mut status: InstanceResMut<ScenePortalRuntimeStatus>,
    author_preview: Option<crate::scenes::dynamic_scene::author_sources::TerrainAuthorRes<feathrise_scene_content::SceneAuthorPreview>>,
    mut bindings:ResMut<super::bindings::PortalBindingResolver>,
    queries:crate::TerrainObjectQueries,
) {
    let (current_scene, frame, root_scene, root_request,graph,terrain_status) = current_scene;
    bindings.poll();
    let scene = *current_scene.get();
    let scene_id = active_scene_id(scene, &active_dynamic_scene); // 动态场景的真实身份来自ActiveDynamicScene，不能压缩成同一个枚举值。
    if state.active_scene != Some(scene) || state.active_scene_id.as_ref() != scene_id.as_ref() {
        state.active_scene = Some(scene);
        state.active_scene_id = scene_id.clone();
        state.dirty = true;
    }
    if !state.dirty {
        return;
    }
    let Some(scene_id) = scene_id else {
        for entity in state.spawned_entities.drain(..) {
            commands.entity(entity).try_despawn();
        }
        status.phase = ScenePortalRuntimePhase::Ready;
        status.scene_id = None;
        status.portal_count = 0;
        status.detail = "当前场景没有数据驱动传送点".to_owned();
        state.dirty = false;
        return;
    };
    status.scene_id = Some(scene_id.clone());
    if graph.as_deref().zip(terrain_status.as_deref()).is_some_and(|(graph,status)|
        graph.root_scene_id.is_some()&&(!status.ready||status.applied_revision!=graph.revision)) {
        status.phase=ScenePortalRuntimePhase::Waiting;status.detail="正在等待完整关联地形姿态采用后恢复传送图".into();return;
    }
    let Some(registry) = registry.as_deref() else {
        status.phase = ScenePortalRuntimePhase::Waiting;
        status.detail = "场景目录尚未建立".to_owned();
        return;
    };
    let Some(visual_meshes) = visual_meshes.as_deref() else {
        status.phase = ScenePortalRuntimePhase::Waiting; // 启动首帧不发布缺少视觉几何的半套传送实体。
        status.detail = "传送点共享视觉网格尚未建立".to_owned(); // 明确等待PortalPlugin启动资源。
        return; // 保留上一完整传送修订，下一帧资源就绪后继续原子采用。
    };
    state.catalog_revision = registry.revision;
    if let Some(detail) = current_portal_dependency_failure(
        &scene_id,
        registry,
        &portal_assets,
        &asset_server,
        author_preview.as_deref(),
    ) {
        status.phase = ScenePortalRuntimePhase::Failed;
        if status.detail != detail {
            error!("传送图拒绝应用：{detail}");
        }
        status.detail = detail;
        state.dirty = false;
        return;
    }
    let plans = match resolve_current_scene_bound(
        &scene_id,
        registry,
        &manifests,
        &portal_assets,
        &boundary_assets,
        author_preview.as_deref(),
        &mut |manifest,binding|queries.binding_pose(&manifest.id,(manifest.id==scene_id).then(||frame.instance()).flatten(),binding)
            .or_else(|_|bindings.resolve(manifest,binding)),
    ) {
        Ok(plans) => plans,
        Err(detail) if detail.contains("仍在加载") || detail.contains("尚未出现") => {
            status.phase = ScenePortalRuntimePhase::Waiting;
            status.detail = detail;
            return;
        }
        Err(detail) => {
            status.phase = ScenePortalRuntimePhase::Failed;
            if status.detail != detail {
                error!("传送图拒绝应用：{detail}");
            }
            status.detail = detail;
            state.dirty = false;
            return;
        }
    };
    for entity in state.spawned_entities.drain(..) {
        commands.entity(entity).try_despawn();
    }
    for plan in &plans {
        let local_arrival = frame.instance().is_some() && plan.target_scene_id == scene_id.as_str();
        let target_scene = if local_arrival { root_scene.as_ref().map_or(plan.target_scene, |scene| *scene.get()) } else { plan.target_scene };
        let target_scene_id = if local_arrival { root_request.as_ref().and_then(|root| root.scene_id.as_ref()).map(SceneId::as_str).unwrap_or(&plan.target_scene_id) }
            else { &plan.target_scene_id };
        let spawned = feathrise_portal::spawn_portal_entity(
            &mut commands,
            visual_meshes,
            &mut materials,
            feathrise_portal::PortalSpawnSpec {
                position: plan.position,
                target_scene,
                target_scene_id: (target_scene == ScenePhase::Active).then_some(target_scene_id),
                target_display_name: &plan.target_display_name, // LoadingScene直接采用目的地实际名称。
                spawn_position: if local_arrival { frame.to_world(plan.target_spawn) } else { plan.target_spawn },
                label: &plan.label,
                color: plan.color,
                owner_scene: scene,
                direction_degrees: plan.arrow_degrees,
            },
        );
        commands.entity(spawned.root).insert(Transform::from_translation(plan.position).with_scale(plan.scale)); // 整体地形尺寸同步到传送可视/触发根。
        if local_arrival { commands.entity(spawned.root).insert(LocalPortalArrival(plan.target_spawn)); }
        state.spawned_entities.extend(spawned.into_array());
    }
    status.phase = ScenePortalRuntimePhase::Ready;
    status.revision = status.revision.wrapping_add(1);
    status.portal_count = plans.len();
    status.detail = format!("场景{scene_id}传送图已应用");
    state.dirty = false;
    info!(
        "{}：{}个传送点，修订{}",
        status.detail, status.portal_count, status.revision
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap; // 构造只包含目标场景的确定性注册表。

    use feathrise_scene_content::{
        SceneCatalogAsset, SceneContentHandles, SceneEnvironmentDescriptor, ScenePlacementsAsset,
        ScenePortalTarget, SceneSurfaceAsset, SceneTerrainDescriptor, SceneTerrainKind, SceneVegetationAsset,
        SceneWaterAsset,
    }; // 使用正式公开资产类型验证默认出生点完整解析路径。

    use super::*; // 测试私有resolve_target_spawn，确保运行时语义而不只验证Serde。

    #[test]
    fn cross_scene_catalog_updates_invalidate_portals_inside_scoped_scene_views() {
        use feathrise_scene_core::residency::{
            InstanceResourceTable, SceneInstanceContextAppExt, SceneRuntimeScope, SceneSourceFrame,
        };

        let mut app = App::new();
        app.init_instance_resource::<SceneContentRegistry>()
            .init_instance_resource::<ScenePortalRuntimeState>()
            .init_resource::<ActiveDynamicScene>()
            .add_message::<AssetEvent<SceneManifestAsset>>()
            .add_message::<AssetEvent<ScenePortalsAsset>>()
            .add_message::<AssetEvent<SceneBoundariesAsset>>()
            .add_systems(Update, observe_portal_data_events);
        let instance = app.world_mut().spawn_empty().id();
        app.insert_resource(SceneRuntimeScope {
            instance: Some(instance), frame: SceneSourceFrame::default(),
        });
        app.world_mut().resource_mut::<SceneContentRegistry>().revision = 7;
        app.world_mut().resource_mut::<InstanceResourceTable<SceneContentRegistry>>()
            .values.insert(instance, SceneContentRegistry { revision: 3, ..default() });
        app.world_mut().resource_mut::<InstanceResourceTable<ScenePortalRuntimeState>>()
            .values.insert(instance, ScenePortalRuntimeState {
                dirty: false, catalog_revision: 3, ..default()
            });

        // 当前实例只持有自己的投影，目标场景的新目录仍必须唤醒这份传送图。
        app.update();
        let state = &mut app.world_mut()
            .resource_mut::<InstanceResourceTable<ScenePortalRuntimeState>>().values;
        assert!(state[&instance].dirty);
        let state = state.get_mut(&instance).unwrap();
        state.dirty = false;
        state.catalog_revision = 7;
        app.update();
        assert!(!app.world().resource::<InstanceResourceTable<ScenePortalRuntimeState>>()
            .values[&instance].dirty); // 目录稳定时不反复解析。
        app.world_mut().resource_mut::<SceneContentRegistry>().revision = 8;
        app.update();
        assert!(app.world().resource::<InstanceResourceTable<ScenePortalRuntimeState>>()
            .values[&instance].dirty);
    }

    #[test]
    fn local_arrivals_follow_only_the_selected_instance_frame() {
        use feathrise_scene_core::residency::{SceneInstanceContextAppExt, SceneInstanceOwner, SceneRuntimeScope, SceneSourceFrame};
        let mut app = App::new();
        app.init_instance_resource::<ScenePortalRuntimeState>()
            .add_systems(Update, follow_local_portal_arrivals);
        let a = app.world_mut().spawn_empty().id();
        let b = app.world_mut().spawn_empty().id();
        let portal = feathrise_portal::Portal { target_scene: ScenePhase::Active,
            target_scene_id: Some("MAP-parent".into()), target_display_name: "父场景".into(),
            spawn_position: Vec3::ZERO, label: "内部传送".into() };
        let local = Vec3::new(2.0, 3.0, 4.0);
        let first = app.world_mut().spawn((portal.clone(), LocalPortalArrival(local), SceneInstanceOwner(a))).id();
        let second = app.world_mut().spawn((portal, LocalPortalArrival(local), SceneInstanceOwner(b))).id();
        app.world_mut().insert_resource(SceneRuntimeScope { instance: Some(a), frame: SceneSourceFrame::from_transform(
            Transform::from_xyz(100.0, 12.0, 20.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))) });
        app.update();
        assert!(app.world().get::<feathrise_portal::Portal>(first).unwrap().spawn_position
            .abs_diff_eq(Vec3::new(104.0, 15.0, 18.0), 0.0001));
        assert_eq!(app.world().get::<feathrise_portal::Portal>(second).unwrap().spawn_position, Vec3::ZERO);
        app.world_mut().insert_resource(SceneRuntimeScope { instance: Some(b), frame: SceneSourceFrame::from_transform(
            Transform::from_xyz(-100.0, 0.0, 0.0)) });
        app.update();
        assert_eq!(app.world().get::<feathrise_portal::Portal>(second).unwrap().spawn_position, Vec3::new(-98.0, 3.0, 4.0));
        assert!(app.world().get::<feathrise_portal::Portal>(first).unwrap().spawn_position
            .abs_diff_eq(Vec3::new(104.0, 15.0, 18.0), 0.0001));
    }

    #[test]
    fn scene_exit_invalidates_same_dynamic_scene_session() {
        let scene_id = SceneId::parse("MAP-returning").expect("测试场景ID有效");
        let mut app = App::new();
        app.init_resource::<ScenePortalRuntimeStatus>()
            .init_resource::<ScenePortalRuntimeState>()
            .add_systems(Update, invalidate_portal_runtime_on_scene_exit);
        {
            let mut state = app.world_mut().resource_mut::<ScenePortalRuntimeState>();
            state.dirty = false;
            state.active_scene = Some(ScenePhase::Active);
            state.active_scene_id = Some(scene_id);
            state.spawned_entities.push(Entity::from_bits(42));
        }
        {
            let mut status = app.world_mut().resource_mut::<ScenePortalRuntimeStatus>();
            status.phase = ScenePortalRuntimePhase::Ready;
            status.portal_count = 3;
            status.revision = 7;
        }

        app.update();

        let state = app.world().resource::<ScenePortalRuntimeState>();
        assert!(state.dirty); // 返回同一DynamicSceneRoot时仍会重新建图。
        assert_eq!(state.active_scene, None);
        assert_eq!(state.active_scene_id, None);
        assert!(state.spawned_entities.is_empty());
        let status = app.world().resource::<ScenePortalRuntimeStatus>();
        assert_eq!(status.phase, ScenePortalRuntimePhase::Waiting);
        assert_eq!(status.portal_count, 0);
        assert_eq!(status.revision, 7); // 离场不伪造一次成功采用修订。
    }

    #[test]
    fn target_without_portal_id_uses_manifest_default_spawn_without_portal_asset() {
        let target_scene_id = SceneId::parse("MAP-default-arrival").expect("测试场景ID有效");
        let mut manifests = Assets::<SceneManifestAsset>::default(); // 只登记目标清单，不登记目标传送图。
        let manifest_handle = manifests.add(SceneManifestAsset {
            vegetation_paint_groups: Default::default(),
            terrains: None,
            subscenes: Default::default(),
            puddles: None,                               // 旧传送图夹具不启用新增独立积水作者域。
            schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,                           // 本测试只覆盖运行时目标解析。
            id: target_scene_id.clone(),                   // 清单身份必须与引用一致。
            display_name: "默认落点场景".to_owned(),     // 返回计划应采用清单名称。
            notes: String::new(),                        // 传送目标不读取作者备注。
            runtime_scene: "data_driven".to_owned(), // 使用已发布动态场景绑定。
            world_min: [-64.0, -64.0],                   // 默认落点位于受检范围内。
            world_max: [64.0, 64.0],                     // 提供合法二维世界边界。
            default_spawn: [7.0, 3.5, -9.0],             // 期望玩家实际生成的安全坐标。
            environment: SceneEnvironmentDescriptor::default(), // 目标解析不依赖环境资源。
            terrain: SceneTerrainDescriptor {resolved_stitches: Default::default(),
                kind: SceneTerrainKind::Gltf, // 合法清单仍需提供完整地形描述。
                collision_mode: Default::default(), // 传送目标解析不读取Clipmap碰撞选择。
                collision_lods: Vec::new(),        // GLB传送测试不消费稀疏碰撞配置。
                display_lods: None,                // GLB传送测试不消费SparseClipmap显示配置。
                player_collision_preload_radius_meters:
                    feathrise_scene_content::DEFAULT_PLAYER_COLLISION_PRELOAD_RADIUS_METERS, // 目标出生不依赖此值。
                role_collision_preload_radius_meters: None, // 传送测试GLB地形不使用粗级预载。
                player_collision_chunk_cells: None,
                role_collision_chunk_cells: None,
                player_collision_span_meters: None, // 传送测试场景不使用分块联动设置。
                role_collision_span_meters: None,   // 传送测试场景不使用分块联动设置。
                glb_path: "scenes/MAP-default-arrival/terrain.glb".to_owned(), // 路径不在本测试加载。
                height_path: None,                  // 目标传送解析不读取高度作者源。
                height_texture_path: None,          // 目标传送解析不读取GPU高度图。
                resolution: [2, 2],                 // 最小合法描述即可满足结构完整性。
                base_height: 0.0,                   // 不参与出生点解析。
                height_scale: 1.0,                  // 不参与出生点解析。
            },
            asset_roots: Vec::new(), // 目标解析不依赖资产浏览目录。
        });
        let mut scenes = BTreeMap::new();
        scenes.insert(
            target_scene_id.clone(),
            SceneContentHandles {
                manifest: manifest_handle.clone(),             // 唯一需要Ready的目标资产。
                portals: Handle::<ScenePortalsAsset>::default(), // 故意不给该句柄登记资产，证明默认出生点不依赖传送图。
                placements: Handle::<ScenePlacementsAsset>::default(), // 其余作者文档不参与传送目标解析。
                surface: None::<Handle<SceneSurfaceAsset>>,            // 不需要地表权重。
                vegetation: None::<Handle<SceneVegetationAsset>>,      // 不需要植被定义。
                water: None::<Handle<SceneWaterAsset>>,                // 不需要水域定义。
                boundaries: None,                                    // 不需要区域边界定义。
                characters_path: None, // 旧传送目标不要求追加场景角色文档。
            },
        );
        let registry = SceneContentRegistry {
            catalog: Handle::<SceneCatalogAsset>::default(), // 测试直接构造场景句柄表。
            revision: 1,                                   // 表示目录已经完成一次正式采用。
            scenes,                                          // 只公开目标场景。
            last_error: None,                              // 目录没有失败状态。
        };
        let record = ScenePortalRecord {
            terrain_binding: None,
            arrival_terrain_binding: None,
            id: "source".to_owned(),   // 源点其余字段不影响目标解析。
            label: "源点".to_owned(),  // 提供完整作者记录。
            label_customized: true,    // 测试名称是显式作者名而非自动默认快照。
            position: [0.0, 0.0, 0.0], // 源触发位置不在本函数检查范围。
            arrival: [1.0, 0.0, 0.0],  // 源arrival只供其他场景传入。
            target: ScenePortalTarget::default_spawn(target_scene_id.clone()), // 明确选择清单默认出生点。
            color_srgba: [1.0, 1.0, 1.0, 1.0],                             // 提供完整记录颜色。
            arrow_degrees: 0.0,                                            // 提供完整记录方向。
        };
        let portal_assets = Assets::<ScenePortalsAsset>::default(); // 保持完全空集合。

        let (scene, display_name, spawn) =
            resolve_target_spawn(&record, &registry, &manifests, &portal_assets, None)
                .expect("缺少具体portal_id时应只依赖目标清单");

        assert_eq!(scene, ScenePhase::Active); // 场景仍从目标清单解析。
        assert_eq!(display_name, "默认落点场景"); // 加载页名称来自同一清单快照。
        assert_eq!(spawn, Vec3::new(7.0, 3.5, -9.0)); // 玩家最终生成在default_spawn而不是几何中心。
        // 同一目标以新公共名称保存时，传送结果不变；未知类型仍明确失败。
        manifests.get_mut(&manifest_handle).unwrap().runtime_scene = "data_driven".into();
        let public = resolve_target_spawn(&record, &registry, &manifests, &portal_assets, None).unwrap();
        assert_eq!(public, (scene, display_name, spawn));
        manifests.get_mut(&manifest_handle).unwrap().runtime_scene = "FutureRuntime".into();
        assert!(resolve_target_spawn(&record, &registry, &manifests, &portal_assets, None).is_err());
    }

    #[test] // 正式实体方向必须采用当前边界派生值；序列化角度只服务无边界旧场景。
    fn runtime_portal_direction_prefers_main_boundary_and_keeps_static_fallback() {
        let scene_id = SceneId::parse("MAP-runtime-portal-direction").expect("测试场景ID有效");
        let boundaries = SceneBoundariesAsset::rectangular_default(
            &scene_id,
            [-50.0, -30.0],
            [50.0, 30.0],
            -80.0,
            160.0,
        )
        .expect("测试主边界应构造成功");
        let position = Vec3::new(40.0, 2.0, 0.0); // 该点明显最靠近矩形东边界。
        assert_eq!(
            resolved_portal_arrow_degrees(position, 13.0, Some(&boundaries)),
            90.0
        ); // 有边界时旧13°不能污染自动东向结果。
        assert_eq!(resolved_portal_arrow_degrees(position, 13.0, None), 13.0); // HN旧静态图没有边界文件时保持既有视觉。
    }
}
