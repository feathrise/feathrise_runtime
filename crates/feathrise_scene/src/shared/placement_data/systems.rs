//! 显式场景实例事件观察、校验、隐藏分帧生成与原子替换。

mod candidate; // 隐藏候选的分帧生成、复验、原子采用与标记收口。
mod events; // JSON与Scene0资产事件观察、重试和候选采用阻断。
mod loading; // 去重Scene依赖的O(1)事件归属与固定预算状态队列。
mod planning; // 正式placement JSON的记录/水体双预算增量规划。
mod retirement; // 活动/候选批次撤销及固定预算树回收。
mod terrain_scope; // 地图和独立地形明确划分摆放记录所有权。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use bevy::asset::{Asset, DependencyLoadState, LoadState, RecursiveDependencyLoadState}; // JSON和场景资产状态只在变化或候选构建期读取。
use bevy::prelude::*; // ECS、GLTF场景根与变换。
use feathrise_map_content::{
    ActiveDynamicMap, BuildingPresetRegistry, MapId, MapManifestAsset,
    MapPlacementAssetTarget, MapPlacementRecord, MapPlacementsAsset,
}; // 运行时与编辑器共享作者格式。
use feathrise_scene_core::state::ScenePhase; // 当前玩法地图与实体生命周期。

use crate::BuildingVegetationClearanceState; // 正式placement原子采用时同步发布建筑植被净空。

use super::components::PlacementStagingRoot; // 重建系统把候选容器Query交给原子采用子模块。
use super::resources::{
    MapPlacementRuntimePhase, MapPlacementRuntimeState, MapPlacementRuntimeStatus,
    PendingPlacementPrimitive,
};

pub(crate) use candidate::cleanup_active_placement_markers; // 插件层保持原有系统路径不变。
pub(crate) use events::{
    complete_world_asset_invalidation, observe_placement_data_events, observe_world_asset_event,
    world_asset_event_is_relevant,
}; // 上层插件与共享World生命周期域保留原有窄调用路径。
pub(crate) use retirement::cleanup_retired_placement_plan; // 大计划/ID集合独立按固定预算析构。
pub(crate) use retirement::cleanup_retired_placement_trees; // 插件层保持原有系统路径不变。

#[cfg(test)]
use super::components::{
    MapPlacementInstance, PlacementInstanceGate, PlacementPendingInstance, PlacementPendingRevision,
}; // systems/tests.rs通过父模块直接构造候选根夹具。
#[cfg(test)]
use super::resources::{PendingPlacementRevision, PlacementStaging, PlacementTreeCleanup}; // 测试直接构造候选、加载队列和退役树夹具。
#[cfg(test)]
use candidate::{PLACEMENT_READY_VERIFY_BUDGET_PER_FRAME, finish_placement_candidate}; // 现有候选状态机回归继续直接验证私有采用门。
#[cfg(test)]
use retirement::PLACEMENT_TREE_CLEANUP_BUDGET_PER_FRAME; // 现有退役树回归继续校验单帧固定预算。 // systems/tests.rs通过父模块直接构造候选与退役树夹具。

fn scene_map_id(scene: ScenePhase, active_dynamic: &ActiveDynamicMap) -> Option<MapId> {
    match scene {
        ScenePhase::Active => active_dynamic.map_id.clone(),
        ScenePhase::Idle | ScenePhase::Loading => None,
    }
}

#[cfg(test)]
use planning::validate_asset_path; // 现有路径回归继续从父模块访问同一增量校验入口。

fn handle_failed<A: Asset>(asset_server: &AssetServer, handle: &Handle<A>) -> bool {
    asset_server
        .get_load_states(handle.id())
        .is_some_and(|(direct, dependencies, recursive)| {
            matches!(direct, LoadState::Failed(_))
                || matches!(dependencies, DependencyLoadState::Failed(_))
                || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
        })
}

/// 为一次已经取得完整输入或确定失败的候选分配公开尝试修订。
fn begin_placement_revision(
    state: &mut MapPlacementRuntimeState,
    status: &mut MapPlacementRuntimeStatus,
) {
    state.revision = state.revision.wrapping_add(1).max(1); // 私有u32修订继续用于候选实体的轻量身份。
    status.attempt_revision = status.attempt_revision.wrapping_add(1).max(1); // 公开u64证据供保存门严格识别发布后的新尝试。
    status.advance_progress(); // 完整计划或确定失败开始了一次真实新尝试。
}

/// 子实例采用后的内容修订由外层完整批次接管；未Ready的初次候选仍须正常推进。
fn keep_adopted_instance_placements(instance_domain: bool, state: &MapPlacementRuntimeState) -> bool {
    instance_domain && state.active.is_some()
}

/// 场景State变化时立即撤销旧批次身份；分帧清理不能依赖玩法run condition。
pub(crate) fn observe_placement_scene_scope(
    mut commands: Commands,
    scene: InstanceRes<State<ScenePhase>>,
    mut state: InstanceResMut<MapPlacementRuntimeState>,
    mut status: InstanceResMut<MapPlacementRuntimeStatus>,
    mut building_clearances: InstanceResMut<BuildingVegetationClearanceState>,
) {
    let scene = *scene.get();
    if state.active_scene == Some(scene) {
        return; // 稳定场景不产生任何查询或命令。
    }
    retirement::retire_candidate(&mut commands, &mut state);
    retirement::retire_active_batch(&mut commands, &mut state);
    building_clearances.clear_formal(); // 离开旧场景时立即撤销正式来源，避免旧图净空残留。
    state.active_scene = Some(scene);
    state.active_map_id = None; // 同场景内MapId将在正式重建系统中再次取得并核对。
    state.terrain_patch_active = false;
    state.terrain_mesh_generation = None;
    state.terrain_holes_applied = Default::default();
    state.generation = state.generation.wrapping_add(1).max(1);
    state.revision = 0;
    state.dirty = true;
    status.phase = MapPlacementRuntimePhase::Waiting;
    status.map_id = None; // 旧地图Ready证据与活动门在同一帧失效。
    status.placement_count = 0;
    status.detail = "正在切换显式实例地图作用域".to_owned();
}

pub(super) fn fail_revision(
    state: &mut MapPlacementRuntimeState,
    status: &mut MapPlacementRuntimeStatus,
    detail: String,
) {
    let failure_changed =
        status.phase != MapPlacementRuntimePhase::Failed || status.detail != detail;
    status.phase = MapPlacementRuntimePhase::Failed;
    status.detail = detail.clone();
    if failure_changed {
        status.advance_progress(); // 同一失败稳定轮询不续命，首次终止候选则属于真实状态推进。
    }
    state.asset_retry_requested = false;
    if state.failure_logged_revision != Some(state.revision) {
        error!(
            "显式实例修订拒绝应用：generation={} revision={} detail={detail}",
            state.generation, state.revision
        );
        state.failure_logged_revision = Some(state.revision);
    }
}

/// 当前地图或JSON变化时建立强场景句柄，并在全部依赖加载后启动隐藏候选根。
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_data_driven_placements(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    scene: InstanceRes<State<ScenePhase>>,
    active_dynamic: InstanceRes<ActiveDynamicMap>,
    registry: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>,
    building_registry: Option<Res<BuildingPresetRegistry>>,
    manifests: Res<Assets<MapManifestAsset>>,
    placements: Res<Assets<MapPlacementsAsset>>,
    primitive_meshes: Res<Assets<Mesh>>, // 候选生成前只读直接primitive的顶点布局并拒绝缺Skin的蒙皮实例。
    mut state: InstanceResMut<MapPlacementRuntimeState>,
    mut status: InstanceResMut<MapPlacementRuntimeStatus>,
    mut building_clearances: InstanceResMut<BuildingVegetationClearanceState>,
    mut container_visibilities: InstanceQuery<&mut Visibility, With<PlacementStagingRoot>>, // 原子采用直接改两个容器，不让公开Ready领先于延迟命令。
    author_preview: Option<crate::maps::dynamic_map::author_sources::TerrainAuthorRes<feathrise_map_content::MapAuthorPreview>>,
    terrain_inputs: terrain_scope::TerrainDomainInputs,
    reusable_roots: candidate::PlacementReuseInputs,
) {
    let scene = *scene.get();
    if state.active_scene != Some(scene) {
        return; // 无条件作用域系统会先撤销旧批次；本系统不再以递归despawn兜底抢跑。
    }
    let Some(map_id) = scene_map_id(scene, &active_dynamic) else {
        retirement::retire_candidate(&mut commands, &mut state);
        retirement::retire_active_batch(&mut commands, &mut state);
        building_clearances.clear_formal(); // 无地图场景不保留上一张地图的正式净空。
        state.active_map_id = None;
        status.phase = MapPlacementRuntimePhase::Ready;
        status.map_id = None;
        status.placement_count = 0;
        status.detail = "当前场景没有数据驱动显式实例".to_owned();
        state.dirty = false;
        return;
    };
    if state.active_map_id.as_ref() != Some(&map_id) {
        retirement::retire_candidate(&mut commands, &mut state);
        retirement::retire_active_batch(&mut commands, &mut state);
        building_clearances.clear_formal(); // 同一动态场景切图也要先撤销旧图正式来源。
        state.active_map_id = Some(map_id.clone()); // 同一DynamicMapScene内换图也立即换代，拒绝同名placement串图。
        state.terrain_patch_active = false;
        state.terrain_mesh_generation = None;
        state.terrain_holes_applied = Default::default();
        state.generation = state.generation.wrapping_add(1).max(1);
        state.revision = 0;
        state.dirty = true;
    }
    status.map_id = Some(map_id.clone());
    if keep_adopted_instance_placements(terrain_inputs.is_instance_domain(), &state) {
        return; // 原地换代会绕过已归并SubMapBatch的物理门；epoch重建成功前保留旧根、状态和碰撞。
    }
    let Some(registry) = registry.as_deref() else {
        status.phase = MapPlacementRuntimePhase::Waiting;
        status.detail = "地图目录尚未建立".to_owned();
        return;
    };
    state.catalog_revision = registry.revision;
    let Some(building_registry) = building_registry.as_deref() else {
        status.phase = MapPlacementRuntimePhase::Waiting;
        status.detail = "建筑预设目录尚未建立".to_owned();
        return; // 未取得完整目录时保留上一正式批次和净空，不猜测建筑范围。
    };
    if state.building_preset_revision != building_registry.revision {
        state.building_preset_revision = building_registry.revision; // 冻结当前完整建筑目录身份。
        state.dirty = true; // 净空参数热更必须重新走有界规划与隐藏候选链。
    }
    let Some(handles) = registry.handles(&map_id) else {
        status.phase = MapPlacementRuntimePhase::Waiting;
        status.detail = format!("地图{map_id}尚未出现在目录");
        return; // 目录仍在重建时不消费dirty，后续帧继续取得完整句柄。
    };
    let placement_handle = author_preview
        .as_deref()
        .and_then(|preview| preview.for_map(&map_id))
        .map(|preview| &preview.placements)
        .unwrap_or(&handles.placements);

    if state.dirty {
        retirement::retire_candidate(&mut commands, &mut state);
        if state.retired_plan.is_some() || !state.retired_trees.is_empty() {
            status.phase = MapPlacementRuntimePhase::Waiting;
            status.detail =
                format!("地图{map_id}正在分帧回收旧显式实例计划与候选，完成后继续读取新修订");
            return; // 计划退役槽和树清理队列共同形成固定背压，修订风暴不能积累大容器或隐藏树。
        }
        if handle_failed(&asset_server, &handles.manifest)
            || handle_failed(&asset_server, placement_handle)
        {
            begin_placement_revision(&mut state, &mut status); // 加载失败也是本次发布之后可被交接门识别的新正式尝试。
            state.dirty = false;
            fail_revision(
                &mut state,
                &mut status,
                format!("地图{map_id}清单或摆放JSON加载失败"),
            );
            return;
        }
        let Some(manifest) = manifests.get(&handles.manifest) else {
            status.phase = MapPlacementRuntimePhase::Waiting;
            status.detail = format!("地图{map_id}清单仍在加载");
            return; // 输入未完整时不开启尝试修订。
        };
        let Some(document) = placements.get(placement_handle) else {
            status.phase = MapPlacementRuntimePhase::Waiting;
            status.detail = format!("地图{map_id}摆放文件仍在加载");
            return; // 等待Bevy资产管线完成IO，不在主线程同步读文件。
        };
        let invalid_identity = manifest.id != map_id || document.map_id != map_id; // 清单、JSON与当前地图必须属于同一身份。
        let invalid_count = document.placements.len() > planning::MAX_PLACEMENTS_PER_MAP; // Vec长度O(1)硬门早于任何单条拷贝。
        let bounds = manifest.world_bounds(); // 世界范围只在启动帧转换一次。
        if invalid_identity || invalid_count || bounds.is_none() {
            begin_placement_revision(&mut state, &mut status); // 确定格式失败也必须推进公开尝试修订。
            state.dirty = false;
            let detail = if invalid_identity {
                format!("地图{map_id}的清单或摆放文件ID不一致")
            } else if invalid_count {
                format!(
                    "地图{map_id}显式实例超过{}项；海量内容必须使用植被密度层",
                    planning::MAX_PLACEMENTS_PER_MAP,
                )
            } else {
                format!("地图{map_id}世界范围无效")
            };
            fail_revision(&mut state, &mut status, detail); // 旧活动批次在格式修复前继续保留。
            return;
        }
        begin_placement_revision(&mut state, &mut status); // 输入完整后立即冻结唯一增量规划身份。
        let generation = state.generation; // 避免构造资源时产生重叠可变借用。
        let revision = state.revision; // 本尝试修订与后续根组件完全对齐。
        state.failure_logged_revision = None;
        state.planning = Some(planning::begin_planning(
            generation,                              // 当前地图代次。
            revision,                                // 当前尝试修订。
            map_id.clone(),                          // 当前地图稳定ID。
            registry.revision,                       // 当前目录修订。
            building_registry.revision,              // 当前建筑目录修订。
            handles.manifest.id(),                   // 冻结清单句柄身份。
            placement_handle.id(),                   // 冻结正式文件或完整文档临时句柄身份。
            bounds.expect("上方已确认世界范围合法"), // 后续单条只执行Rect包含检查。
            document.placements.len(),               // 冻结O(1)文档长度，跨帧变化会取消事务。
        ));
        state.planning.as_mut().unwrap().terrain_filter = terrain_inputs.filter(manifest, &map_id);
        state.asset_retry_requested = false; // Scene失败重试门只在规划完成后有意义。
        state.dirty = false;
        status.phase = MapPlacementRuntimePhase::Waiting;
        status.detail = format!(
            "地图{map_id}正在增量规划显式实例：0/{}",
            document.placements.len(),
        );
    }

    if state.planning.is_some() {
        let planning_matches = state.planning.as_ref().is_some_and(|work| {
            planning::planning_matches_inputs(
                work,                       // 核对进行中事务。
                state.generation,           // 当前地图代次。
                &map_id,                    // 当前地图ID。
                registry.revision,          // 当前目录修订。
                building_registry.revision, // 当前建筑目录修订。
                handles.manifest.id(),      // 当前清单资产ID。
                placement_handle.id(),      // 当前摆放资产ID。
            )
        });
        if !planning_matches {
            retirement::retire_candidate(&mut commands, &mut state); // 旧规划和任何旧候选一起失效。
            state.dirty = true; // 下一帧从新清单/JSON首条重新规划。
            status.phase = MapPlacementRuntimePhase::Waiting;
            status.detail = format!("地图{map_id}显式实例输入已换代，正在取消旧规划");
            status.advance_progress(); // 旧修订取消是真实身份推进。
            return;
        }
        let Some(document) = placements.get(placement_handle) else {
            status.phase = MapPlacementRuntimePhase::Waiting;
            status.detail = format!("地图{map_id}摆放文件热更中，增量规划已停在安全游标");
            return; // 资产暂时不可用时不丢失旧活动批次，也不忙等。
        };
        let mut load_assets = |record: &MapPlacementRecord| {
            if record.asset_target.is_builtin() {
                return (Handle::default(), None); // 内置来源没有磁盘Scene0，不登记空路径或虚假加载依赖。
            }
            let scene =
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(record.asset_path.clone())); // Scene0仍作为同源GLTF解析和完整场景记录的依赖锚点。
            let primitive = match record.asset_target {
                MapPlacementAssetTarget::Scene0 => None,
                MapPlacementAssetTarget::Cuboid { .. } | MapPlacementAssetTarget::EditableCuboid { .. } | MapPlacementAssetTarget::EditableMesh { .. } => unreachable!("内置来源已提前返回"),
                MapPlacementAssetTarget::Primitive { .. } => Some(PendingPlacementPrimitive {
                    mesh: asset_server.load::<Mesh>(
                        record
                            .asset_target
                            .mesh_reference(&record.asset_path)
                            .expect("primitive目标必须生成网格引用"),
                    ),
                    material: asset_server.load::<StandardMaterial>(
                        record
                            .asset_target
                            .material_reference(&record.asset_path)
                            .expect("primitive目标必须生成材质引用"),
                    ),
                }), // 直接网格沿用F12冻结的原关联/std材质，不从Scene0猜第一个材质。
            };
            (scene, primitive) // 每条记录只在规划预算内登记一次所需句柄。
        };
        let planning_result = {
            let work = state.planning.as_mut().expect("上方已确认增量规划存在");
            planning::advance_planning_bound(work, document, building_registry, &mut load_assets,
                &mut |record| record.terrain_binding.as_ref().filter(|_|record.terrain_patch.is_none())
                    .map(|binding|terrain_inputs.binding_pose(binding,&map_id)).transpose())
        };
        let advance = match planning_result {
            Ok(advance) => advance, // 成功可能仍需后续帧继续。
            Err(detail) => {
                if detail.contains("仍在加载") {
                    status.detail=format!("地图{map_id}正在等待关联地形完整采用");
                    return; // 没有采用身份时保留当前游标和旧有效批次，不能把加载空窗固化为失败。
                }
                retirement::retire_plan_data(&mut state); // 半成品O(1)移入退役槽，不在失败帧同步析构已规划前缀。
                fail_revision(
                    &mut state,
                    &mut status,
                    format!("地图{map_id}的显式实例文档无效：{detail}"),
                );
                return;
            }
        };
        if advance.made_progress() {
            status.advance_progress(); // 一帧内无论完成多少小项都只推进一次公开heartbeat。
        }
        let (next_record, total_records) = state
            .planning
            .as_ref()
            .map(|work| (work.next_record, work.total_records))
            .expect("规划成功返回后事务仍存在"); // 廉价诊断只读取两个标量。
        status.phase = MapPlacementRuntimePhase::Waiting;
        status.detail = format!("地图{map_id}正在增量规划显式实例：{next_record}/{total_records}");
        if !advance.complete {
            return; // 规划期不创建半份隐藏根，旧活动批次继续可用。
        }
        if state
            .planning
            .as_ref()
            .is_some_and(|work| work.terrain_patch_count > 0)
            && manifests.get(&handles.manifest).is_some_and(|manifest| {
                manifest.terrain.kind != feathrise_map_content::MapTerrainKind::SparseClipmap
            })
        {
            retirement::retire_plan_data(&mut state);
            fail_revision(
                &mut state,
                &mut status,
                "三维地形洞口只允许用于 Clipmap 地图".into(),
            );
            return;
        }
        let work = state.planning.take().expect("本帧已完成整份增量规划");
        let (pending, tracker) = planning::finish_planning(work); // Vec、HashSet与VecDeque所有权O(1)移入下一阶段。
        state.pending = Some(pending); // 完整计划和加载跟踪在同帧安装。
        state.scene_load = Some(tracker); // 首轮去重Scene检查队列已在规划期增量构建。
        status.advance_progress(); // 从记录规划切换到依赖检查是一次真实阶段推进。
        status.detail = format!("地图{map_id}正在加载显式实例Scene依赖");
    }

    if state.pending.is_none() {
        return;
    }
    if status.phase == MapPlacementRuntimePhase::Failed && !state.asset_retry_requested {
        return;
    }
    if status.phase == MapPlacementRuntimePhase::Failed {
        state.asset_retry_requested = false; // 消费一次匹配事件唤醒，再次失败后继续等新事件。
        status.phase = MapPlacementRuntimePhase::Waiting;
        status.detail = format!("地图{map_id}显式实例Scene资产已变化，正在有界复查");
    }
    let tracker_matches = state
        .pending
        .as_ref()
        .zip(state.scene_load.as_ref())
        .is_some_and(|(pending, tracker)| {
            pending.generation == tracker.generation && pending.revision == tracker.revision
        }); // 计划与Scene跟踪器必须来自同一原子收尾。
    if !tracker_matches {
        fail_revision(
            &mut state,
            &mut status,
            format!("地图{map_id}的显式实例计划与Scene加载身份不一致"),
        );
        return;
    }
    let mut classify = |id| loading::scene_dependency_state(&asset_server, id); // 每个队列ID执行一次非阻塞AssetServer查询。
    let load_advance = loading::poll_scene_loads(
        state
            .scene_load
            .as_mut()
            .expect("上方已确认Scene加载跟踪器身份"),
        &mut classify,
    ); // 队列再长也不超过每帧固定查询额度。
    if load_advance.newly_ready > 0 {
        status.advance_progress(); // 稳定Waiting轮询不续命，只有新Scene Ready推进heartbeat。
    }
    if load_advance.failed {
        fail_revision(
            &mut state,
            &mut status,
            format!("地图{map_id}至少一个显式实例Scene加载失败；保留上一完整修订"),
        );
        return;
    }
    if !load_advance.all_ready {
        let (ready, total) = state
            .scene_load
            .as_ref()
            .map(|tracker| (tracker.ready_scene_ids.len(), tracker.scene_ids.len()))
            .expect("加载轮询后跟踪器仍存在");
        status.phase = MapPlacementRuntimePhase::Waiting;
        status.detail = format!("地图{map_id}正在加载显式实例Scene依赖：{ready}/{total}");
        return;
    }
    candidate::advance_placement_candidate(
        &mut commands,
        &asset_server,
        &primitive_meshes, // Loaded到主世界Assets后执行常数成本蒙皮属性安全门。
        &map_id,
        &mut state,
        &mut status,
        &mut building_clearances,
        &mut container_visibilities,
        &reusable_roots,
    );
}

#[cfg(test)]
mod tests; // 候选生命周期回归独立放置，保持主系统模块体量有界。
