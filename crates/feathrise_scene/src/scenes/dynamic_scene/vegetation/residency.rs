//! 动态Mesh植被按相机范围进行空间常驻，不改变作者密度或候选间距。
//!
//! 【功能设计】密度图是全图逻辑真值；本模块只决定哪些一百二十八米块此刻拥有ECS根。
//! 块任务single-flight、主线程分帧排入和LOD最远范围共同控制瞬时工作量，但不存在全图实例上限。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use std::collections::BTreeSet; // 期望块集合保持稳定有序，便于确定性差分与测试。
use std::sync::Arc; // 常驻块共享完整作者变换给近场碰撞，不复制后台结果。

use bevy::prelude::*; // ECS、空间向量、可见性和异步任务调度。
use bevy::tasks::{AsyncComputeTaskPool, futures::check_ready}; // 单块CPU散布离开主线程，主线程只非阻塞轮询。
use crate::SceneRuntimeObserver; // 真实玩家而非编辑观察相机决定物理代理常驻。
use feathrise_environment::vegetation::VegetationMeshPrototypeRegistry; // 原型最远LOD范围决定常驻半径。
use feathrise_environment::vegetation::blade::GpuBladeView; // F12会把同一正式观察身份移交给编辑相机。
use feathrise_scene_content::{SceneVegetationInstanceTransform, VegetationPresetRegistry}; // 目录物理规格与后台作者变换共同驱动近场代理。
use feathrise_settings::RenderSettings; // 玩家场景细节档只改变当前常驻范围，不删作者内容。

use super::build::{
    DYNAMIC_MESH_CHUNK_SIZE_METERS, DynamicMeshChunkGround, DynamicMeshChunkJob,
    build_dynamic_mesh_chunk,
}; // 每次后台任务只生成一个固定空间块。
use super::collision::update_dynamic_mesh_collision_focus; // 远景渲染块只在玩家近场派生低成本物理代理。
use super::resources::{
    DynamicMeshChunkKey, DynamicMeshChunkOutcome, DynamicMeshResidentChunk, DynamicVegetationBuild,
    DynamicVegetationGroundSource, DynamicVegetationPhase, DynamicVegetationState,
    DynamicVegetationStatus,
}; // 状态显式区分作者源、期望块、在途块和已常驻块。
use super::spawn::{create_mesh_chunk_staging, spawn_mesh_chunk_batch}; // 当前块完整排入前保持父根隐藏。
use crate::BuildingVegetationClearanceState; // 正式placement与F12草稿通过同一修订快照驱动块重建。
use crate::scenes::dynamic_scene::clipmap::{
    ClipmapGroundContentRevision, DynamicClipmapGroundSurface,
}; // Clipmap块只读取本帧已经发布的同代地表快照。

/// 相机在一个块内移动时为最远LOD额外保留对角安全带，避免稳定帧逐米重算期望集合。
const RESIDENCY_CHUNK_SAFETY_METERS: f32 = DYNAMIC_MESH_CHUNK_SIZE_METERS * 1.5;

/// 读取一个正式原型最远可见边界；空原型仍至少常驻相机所在块供诊断发现。
fn prototype_residency_radius(
    prototype: &feathrise_environment::vegetation::VegetationMeshPrototype,
    range_multiplier: f32,
) -> f32 {
    let authored_radius = prototype
        .lods
        .iter() // 原型档数和部件数固定且很小，不访问任何实例或场景像素。
        .map(|lod| lod.visibility_range.end_margin.end) // 最远档淡出终点才是安全回收边界。
        .filter(|distance| distance.is_finite() && *distance >= 0.0) // 坏原型范围不能污染块坐标。
        .fold(0.0_f32, f32::max); // 任意档可见时都必须保留其实例根。
    authored_radius * range_multiplier.max(0.0) + RESIDENCY_CHUNK_SAFETY_METERS // 画质只缩放作者可见范围，安全带不参与画面语义。
}

/// 返回场景在一个轴上的固定块数量；合法场景至少拥有一个块。
fn chunk_axis_count(world_min: f32, world_max: f32) -> i32 {
    ((world_max - world_min) / DYNAMIC_MESH_CHUNK_SIZE_METERS)
        .ceil()
        .max(1.0) as i32 // 项目场景尺寸受清单校验约束，转换不会溢出i32。
}

/// 返回一个相对场景最小角对齐块的世界半开边界。
pub(super) fn chunk_bounds(
    source: &DynamicVegetationBuild,
    key: DynamicMeshChunkKey,
) -> ([f32; 2], [f32; 2]) {
    let minimum = [
        source.world_min[0] + key.x as f32 * DYNAMIC_MESH_CHUNK_SIZE_METERS, // 列坐标只影响当前请求范围。
        source.world_min[1] + key.z as f32 * DYNAMIC_MESH_CHUNK_SIZE_METERS, // 行坐标只影响当前请求范围。
    ];
    let maximum = [
        (minimum[0] + DYNAMIC_MESH_CHUNK_SIZE_METERS).min(source.world_max[0]), // 最东块钳到场景闭边界。
        (minimum[1] + DYNAMIC_MESH_CHUNK_SIZE_METERS).min(source.world_max[1]), // 最南块钳到场景闭边界。
    ];
    (minimum, maximum) // 核心散布入口负责相邻半开块无重复拼接。
}

/// 计算一个层当前视野真正需要的全部空间块；数量由场景和LOD范围决定，不设常量封顶。
fn desired_layer_chunks(
    source: &DynamicVegetationBuild,
    layer_index: usize,
    focus: Vec2,
    radius: f32,
) -> Vec<DynamicMeshChunkKey> {
    let columns = chunk_axis_count(source.world_min[0], source.world_max[0]); // 全图固定列数。
    let rows = chunk_axis_count(source.world_min[1], source.world_max[1]); // 全图固定行数。
    let first_x = (((focus.x - radius - source.world_min[0]) / DYNAMIC_MESH_CHUNK_SIZE_METERS)
        .floor() as i32)
        .clamp(0, columns - 1); // 只访问与圆形常驻域可能相交的最西列。
    let last_x = (((focus.x + radius - source.world_min[0]) / DYNAMIC_MESH_CHUNK_SIZE_METERS)
        .floor() as i32)
        .clamp(0, columns - 1); // 只访问与圆形常驻域可能相交的最东列。
    let first_z = (((focus.y - radius - source.world_min[1]) / DYNAMIC_MESH_CHUNK_SIZE_METERS)
        .floor() as i32)
        .clamp(0, rows - 1); // 只访问与圆形常驻域可能相交的最北行。
    let last_z = (((focus.y + radius - source.world_min[1]) / DYNAMIC_MESH_CHUNK_SIZE_METERS)
        .floor() as i32)
        .clamp(0, rows - 1); // 只访问与圆形常驻域可能相交的最南行。
    let mut keys = Vec::new(); // 结果容量由真实相交块决定，不使用作者数量预算。
    for z in first_z..=last_z {
        for x in first_x..=last_x {
            let key = DynamicMeshChunkKey { layer_index, x, z }; // 同坐标不同层保持独立常驻身份。
            let (minimum, maximum) = chunk_bounds(source, key); // 取得当前块真实世界AABB。
            let nearest = focus.clamp(Vec2::from_array(minimum), Vec2::from_array(maximum)); // 圆到AABB最近点决定真实相交。
            if nearest.distance_squared(focus) <= radius * radius {
                keys.push(key); // 所有与作者可见范围相交的块完整进入期望集合。
            }
        }
    }
    keys // 调用方再按与相机距离排序生成，集合本身不抽样。
}

/// 块中心到观察点的平方距离只用于加载顺序，不影响块是否存在。
fn chunk_distance_squared(
    source: &DynamicVegetationBuild,
    key: DynamicMeshChunkKey,
    focus: Vec2,
) -> f32 {
    let (minimum, maximum) = chunk_bounds(source, key); // 当前块边界已钳到场景范围。
    let center = (Vec2::from_array(minimum) + Vec2::from_array(maximum)) * 0.5; // 最外窄块使用真实中心。
    center.distance_squared(focus) // 最近块优先可见，但远块不会被删除或跳过。
}

/// 当前源、焦点或场景细节范围变化时重建期望集合并回收真正离开范围的块。
fn rebuild_desired_chunks(
    commands: &mut Commands,
    state: &mut DynamicVegetationState,
    registry: &VegetationMeshPrototypeRegistry,
    focus: Vec2,
    range_multiplier: f32,
    clearance_revision: u64,
    ground_revision: Option<ClipmapGroundContentRevision>,
) -> Result<bool, String> {
    let source = state
        .mesh_source
        .as_ref()
        .expect("调用方已确认当前Mesh散布源存在"); // 本函数不处理资产等待阶段。
    let mut desired = BTreeSet::new(); // 多层相交块统一形成当前常驻真值集合。
    for layer in &source.mesh_layers {
        let prototype_id = layer
            .layer
            .prototype_id
            .ok_or_else(|| format!("Mesh植被层{}缺少正式原型ID", layer.layer.id))?; // 定义校验正常已阻止该路径。
        let prototype = registry
            .get(feathrise_environment::vegetation::VegetationPrototypeId(
                prototype_id,
            ))
            .ok_or_else(|| format!("Mesh植被层{}正式原型{}已丢失", layer.layer.id, prototype_id))?; // 热移除原型时发布明确失败。
        let radius = prototype_residency_radius(prototype, range_multiplier); // 当前玩家档位只决定此刻常驻半径。
        desired.extend(desired_layer_chunks(
            source,
            layer.layer_index,
            focus,
            radius,
        )); // 层内全部相交块进入集合，没有数量截断。
    }
    let retired = state
        .resident_mesh_chunks
        .keys()
        .filter(|key| !desired.contains(key))
        .copied()
        .collect::<Vec<_>>(); // 先冻结小型块身份列表，避免可变遍历冲突。
    let retired_any = !retired.is_empty();
    for key in retired {
        if let Some(chunk) = state.resident_mesh_chunks.remove(&key) {
            commands.entity(chunk.root).try_despawn(); // 离开所有可见LOD范围后按块回收ECS根，作者密度仍保留在源图。
        }
    }
    if state.mesh_chunk_staging.as_ref().is_some_and(|staging| {
        !desired.contains(&staging.build.key)
            || staging.build.clearance_revision != clearance_revision
            || staging.build.ground_revision != ground_revision
    }) {
        if let Some(staging) = state.mesh_chunk_staging.take() {
            commands.entity(staging.root).try_despawn(); // 相机已离开时停止继续排入不可见半块。
        }
    }
    let current_identity = (
        source.generation,
        source.revision,
        clearance_revision,
        ground_revision,
    ); // 植被、净空与高度内容都相同的在途块才能参与去重。
    let inflight = state
        .mesh_chunk_task_identity
        .filter(|(generation, revision, clearance, ground, _)| {
            (*generation, *revision, *clearance, *ground) == current_identity
        })
        .map(|(_, _, _, _, key)| key); // 旧净空或高度任务继续占槽但不能阻止新修订同坐标排队。
    let staging_key = state
        .mesh_chunk_staging
        .as_ref()
        .filter(|staging| {
            staging.build.clearance_revision == clearance_revision
                && staging.build.ground_revision == ground_revision
        })
        .map(|staging| staging.build.key); // 当前隐藏安装块不能重复生成。
    let mut queued = desired
        .iter()
        .filter(|key| {
            state.resident_mesh_chunks.get(key).is_none_or(|chunk| {
                chunk.clearance_revision != clearance_revision
                    || chunk.ground_revision != ground_revision
            })
        }) // 旧净空或旧高度完整块继续显示，但必须排队生成同坐标替代块。
        .filter(|key| Some(**key) != inflight) // 当前修订在途块已经拥有后台任务。
        .filter(|key| Some(**key) != staging_key) // 当前修订隐藏块已经进入主线程安装。
        .copied()
        .collect::<Vec<_>>(); // 所有其余缺失块都进入队列，不设最大长度。
    queued.sort_by(|left, right| {
        chunk_distance_squared(source, *left, focus)
            .total_cmp(&chunk_distance_squared(source, *right, focus))
            .then_with(|| left.cmp(right)) // 距离相同时使用稳定层和块身份决胜。
    });
    state.queued_mesh_chunks = queued.into(); // 替换为最近优先的完整缺失块队列。
    state
        .mesh_chunk_height_waits
        .retain(|key, _| desired.contains(key)); // 离开范围的缺页等待不再占状态。
    state.desired_mesh_chunks = desired; // 发布当前画质与焦点的空间真值。
    Ok(retired_any)
}

/// 更新公开状态中的真实常驻数量；只在块集合变化时调用。
fn refresh_residency_status(state: &DynamicVegetationState, status: &mut DynamicVegetationStatus) {
    status.mesh_instance_count = state
        .resident_mesh_chunks
        .values()
        .map(|chunk| chunk.instance_count)
        .sum(); // 汇总完整块缓存，不遍历任何植被ECS实体。
    status.mesh_batch_count = state
        .resident_mesh_chunks
        .values()
        .filter(|chunk| chunk.instance_count > 0)
        .count();
    status.mesh_render_node_count = state
        .resident_mesh_chunks
        .values()
        .map(|chunk| chunk.render_node_count)
        .sum();
    status.mesh_collision_proxy_count = state
        .resident_mesh_chunks
        .values()
        .map(|chunk| chunk.collision_proxies.len())
        .sum();
    status.detail = format!(
        "动态植被已采用：{}个作者Mesh实例 / {}个共享批次 / {}个ECS渲染节点 / {}个近场碰撞，常驻{}块，等待{}块；作者密度未裁剪",
        status.mesh_instance_count,
        status.mesh_batch_count,
        status.mesh_render_node_count,
        status.mesh_collision_proxy_count,
        state.resident_mesh_chunks.len(), // 当前真正可见或可由LOD选择的完整块数。
        state.queued_mesh_chunks.len() + usize::from(state.mesh_chunk_task.is_some()), // 待生成数包含唯一在途任务。
    );
}

/// 相机跨块、画质变化或块任务完成时推进空间常驻；稳定帧不扫描密度图或全世界实体。
#[allow(clippy::too_many_arguments)]
pub(in crate::scenes::dynamic_scene) fn drive_dynamic_mesh_residency(
    frame: feathrise_scene_core::residency::InstanceFrame,
    mut commands: Commands,
    cameras: Query<&Transform, With<GpuBladeView>>,
    players: Query<&Transform, With<SceneRuntimeObserver>>,
    settings: Res<RenderSettings>,
    registry: Res<VegetationMeshPrototypeRegistry>,
    presets: Res<VegetationPresetRegistry>,
    clearances: InstanceRes<BuildingVegetationClearanceState>,
    clipmap_ground: InstanceRes<DynamicClipmapGroundSurface>, // 发布顺序保证只看本帧正式地表快照。
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    let clearance_snapshot = state
        .scene_id
        .as_ref()
        .and_then(|scene_id| clearances.snapshot_for(scene_id)); // 跨场景正式或作者快照不会进入当前动态场景。
    let clearance_revision = clearance_snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.revision); // 零表示尚未有任何建筑净空来源。
    let completed = state.mesh_chunk_task.as_mut().and_then(check_ready); // 主线程只执行一次非阻塞轮询。
    if completed.is_some() {
        state.mesh_chunk_task = None; // 完成任务立即释放single-flight槽。
        state.mesh_chunk_task_identity = None; // 完成身份不再参与队列去重。
    }
    let completed = match completed {
        Some(Ok(outcome)) => Some(outcome),
        Some(Err(error)) => {
            state.queued_mesh_chunks.clear(); // 明确失败后停止同修订继续生成，等待资产事件或作者修复重试。
            status.phase = DynamicVegetationPhase::Failed; // F12和F3都能看到失败而不是静默缺树。
            status.detail = format!("动态Mesh植被空间块生成失败：{error}");
            if state.failure_logged_revision != Some(state.revision) {
                error!("{}", status.detail); // 每个修订最多记录一次完整错误。
                state.failure_logged_revision = Some(state.revision); // 稳定帧不重复刷日志。
            }
            return;
        }
        None => None,
    };

    if status.phase == DynamicVegetationPhase::Failed {
        return; // 同一修订一旦明确失败就保持静止；只有资产事件创建新修订后才允许重新提交空间块。
    }

    let Some(source) = state.mesh_source.clone() else {
        return; // 定义、纹理或正式原型尚未形成当前修订源。
    };
    if !state.current(source.generation, source.revision) || state.dirty {
        return; // 热更准备期间保留上一完整常驻集合，但不再为旧源提交新块。
    }
    if source.mesh_layers.is_empty() {
        return; // 纯Blade场景不需要取得CPU地表快照或维护空Mesh队列。
    }
    let clipmap_snapshot = match &source.ground {
        DynamicVegetationGroundSource::Heightfield { .. } => None,
        DynamicVegetationGroundSource::Clipmap => {
            let Some(snapshot) = clipmap_ground.snapshot.as_ref().filter(|snapshot| {
                state.scene_id.as_ref() == Some(&snapshot.scene_id) && snapshot.ready
            }) else {
                status.detail = "动态Mesh植被正在等待同图已采用Clipmap高度页".to_owned();
                return; // 非空分页地形绝不按Y=0生成实例。
            };
            Some(snapshot.clone()) // 单块worker浅克隆局部页Arc，主线程不复制高度字节。
        }
    };
    let ground_revision = clipmap_snapshot
        .as_ref()
        .map(|snapshot| snapshot.content_revision());
    let ground_query_revision = clipmap_snapshot
        .as_ref()
        .map(|snapshot| snapshot.query_revision());
    status.clipmap_ground_revision =
        ground_revision.map(ClipmapGroundContentRevision::status_words);
    if let Some(query_revision) = ground_query_revision {
        state
            .mesh_chunk_height_waits
            .retain(|_, waiting| *waiting == query_revision); // 页或环带变化后允许原等待块重试。
    } else {
        state.mesh_chunk_height_waits.clear(); // GLB没有局部缺页等待身份。
    }
    if let Some(outcome) = completed {
        match outcome {
            DynamicMeshChunkOutcome::Ready(build)
                if state.current(build.generation, build.revision)
                    && build.clearance_revision == clearance_revision
                    && build.ground_revision == ground_revision
                    && state.desired_mesh_chunks.contains(&build.key) =>
            {
                if let Some(active_root) = state.active_root {
                    state.mesh_chunk_height_waits.remove(&build.key);
                    state.mesh_chunk_staging =
                        Some(create_mesh_chunk_staging(&mut commands, active_root, build)); // 当前仍需要的块进入隐藏分帧安装。
                }
            }
            DynamicMeshChunkOutcome::WaitingForGround(wait)
                if state.current(wait.generation, wait.revision)
                    && wait.clearance_revision == clearance_revision
                    && Some(wait.ground_revision) == ground_revision
                    && state.desired_mesh_chunks.contains(&wait.key) =>
            {
                state
                    .mesh_chunk_height_waits
                    .insert(wait.key, wait.query_revision); // 同一局部页身份不忙重试。
                if !state.queued_mesh_chunks.contains(&wait.key) {
                    state.queued_mesh_chunks.push_back(wait.key); // 其他已就绪块仍可先生成。
                }
                status.detail = "动态Mesh植被正在等待当前空间块的Clipmap高度页".to_owned();
            }
            _ => {} // 切换场景、热更、雕刻或相机移出范围后的迟到结果按身份直接丢弃。
        }
    }
    let mut resident_changed = false; // 只在块增删时额外触发一次近场物理核对。
    let fallback = state
        .pending
        .as_ref()
        .map(|pending| {
            Vec2::new(
                pending.manifest.default_spawn[0],
                pending.manifest.default_spawn[2],
            )
        })
        .unwrap_or(Vec2::ZERO); // 相机尚未出现的首帧从正式出生点准备最近块。
    let focus = cameras
        .single()
        .map(|camera| frame.to_local(camera.translation).xz())
        .unwrap_or(fallback); // F12和玩法共用唯一正式观察身份。
    let focus_chunk = [
        ((focus.x - source.world_min[0]) / DYNAMIC_MESH_CHUNK_SIZE_METERS).floor() as i32, // 相机跨列时才重建集合。
        ((focus.y - source.world_min[1]) / DYNAMIC_MESH_CHUNK_SIZE_METERS).floor() as i32, // 相机跨行时才重建集合。
    ];
    let (_, range_multiplier) = settings.graphics.scene_detail_quality.runtime_multipliers(); // 低中档缩短当前常驻范围，高极高保留作者范围。
    let range_bits = range_multiplier.to_bits(); // 精确档位身份避免浮点近似比较造成稳定帧重算。
    if state.residency_focus_chunk != Some(focus_chunk)
        || state.residency_range_bits != Some(range_bits)
        || state.residency_clearance_revision != Some(clearance_revision)
        || state.residency_ground_revision != ground_revision
    {
        match rebuild_desired_chunks(
            &mut commands,
            &mut state,
            &registry,
            focus,
            range_multiplier,
            clearance_revision,
            ground_revision,
        ) {
            Ok(retired) => resident_changed |= retired,
            Err(error) => {
                status.phase = DynamicVegetationPhase::Failed; // 原型身份异常必须明确发布。
                status.detail = error; // 保留具体层和原型诊断。
                return;
            }
        }
        state.residency_focus_chunk = Some(focus_chunk); // 当前块身份完成采用。
        state.residency_range_bits = Some(range_bits); // 当前画质距离倍率完成采用。
        state.residency_clearance_revision = Some(clearance_revision); // 当前所需块已按这一完整净空修订排队。
        state.residency_ground_revision = ground_revision; // 仅作者/玩法高度内容换代，不记录视角换页。
        refresh_residency_status(&state, &mut status); // 回收或新增队列后同步真实常驻证据。
    }

    if let Some(staging) = state.mesh_chunk_staging.take() {
        let Some(layer) = source
            .mesh_layers
            .iter()
            .find(|layer| layer.layer_index == staging.build.key.layer_index)
        else {
            commands.entity(staging.root).try_despawn(); // 坏迟到层身份不得留下隐藏孤儿根。
            status.phase = DynamicVegetationPhase::Failed;
            status.detail = "动态Mesh植被空间块引用了不存在的正式层".to_owned();
            return;
        };
        match spawn_mesh_chunk_batch(&mut commands, layer, &staging, &registry, &presets) {
            Ok(false) => {
                state.mesh_chunk_staging = Some(staging); // 为未来需要异步GPU上传门的后端保留显式未完成语义。
                return;
            }
            Ok(true) => {
                let key = staging.build.key; // 完整块身份在移动完成值前冻结。
                let transforms: Arc<[SceneVegetationInstanceTransform]> =
                    Arc::from(staging.build.transforms); // 后台Vec零拷贝转成只读共享切片供近场物理复用。
                let instance_count = transforms.len(); // 当前块真实作者实例数。
                let render_node_count = if instance_count == 0 {
                    0
                } else {
                    let prototype_id = layer
                        .layer
                        .prototype_id
                        .expect("非空Mesh层已在批次安装前验证正式原型ID");
                    registry
                        .get(feathrise_environment::vegetation::VegetationPrototypeId(
                            prototype_id,
                        ))
                        .expect("批次安装已取得同一正式原型")
                        .lods
                        .iter()
                        .map(|lod| lod.parts.len())
                        .sum()
                };
                commands.entity(staging.root).insert(Visibility::Inherited); // 一次父根切换显示完整块。
                if let Some(previous) = state.resident_mesh_chunks.insert(
                    key,
                    DynamicMeshResidentChunk {
                        root: staging.root, // 后续离开范围只回收这一父根。
                        clearance_revision: staging.build.clearance_revision, // 新块完整显示后才替换旧净空块。
                        ground_revision: staging.build.ground_revision, // 新块完整显示后才替换旧高度块。
                        instance_count,                                 // 状态汇总不访问子实体。
                        render_node_count, // F3/F4直接区分作者数量与有限ECS承接规模。
                        transforms,        // 保留当前完整块作者姿态，不建立远场逐株ECS。
                        source_root_bounds: staging.build.source_root_bounds, // 后台真实三维根范围与同一完整块原子采用。
                        collision_proxies: default(), // 玩家近场核对后再按稳定ID填充。
                    },
                ) {
                    commands.entity(previous.root).try_despawn(); // 防御重复身份时原子替换旧完整块。
                }
                resident_changed = true;
                refresh_residency_status(&state, &mut status); // 块完成后更新真实常驻数量。
            }
            Err(error) => {
                commands.entity(staging.root).try_despawn(); // 半块失败绝不进入可见世界。
                state.queued_mesh_chunks.clear(); // 当前修订停止继续生成，等待明确重建。
                status.phase = DynamicVegetationPhase::Failed;
                status.detail = format!("动态Mesh植被空间块安装失败：{error}");
                return;
            }
        }
    }

    if let Ok(player) = players.single() {
        match update_dynamic_mesh_collision_focus(
            &mut commands,
            player.translation,
            frame.frame(),
            &source,
            &presets,
            &mut state,
            resident_changed,
        ) {
            Ok(changed) => {
                if changed || resident_changed {
                    refresh_residency_status(&state, &mut status);
                }
            }
            Err(error) => {
                status.phase = DynamicVegetationPhase::Failed;
                status.detail = format!("动态Mesh植被近场碰撞同步失败：{error}");
                return;
            }
        }
    }

    if state.mesh_chunk_task.is_some() || state.mesh_chunk_staging.is_some() {
        return; // single-flight任务或隐藏安装任一占用时不再提交新块。
    }
    let queued_attempts = state.queued_mesh_chunks.len(); // 全部缺页块被阻塞时本帧最多各检查一次。
    for _ in 0..queued_attempts {
        let Some(key) = state.queued_mesh_chunks.pop_front() else {
            break;
        };
        if !state.desired_mesh_chunks.contains(&key)
            || state.resident_mesh_chunks.get(&key).is_some_and(|chunk| {
                chunk.clearance_revision == clearance_revision
                    && chunk.ground_revision == ground_revision
            })
        {
            continue; // 焦点变化留下的过期或已完成队列项不产生后台工作。
        }
        if ground_query_revision.is_some_and(|query| {
            state
                .mesh_chunk_height_waits
                .get(&key)
                .is_some_and(|waiting| *waiting == query)
        }) {
            state.queued_mesh_chunks.push_back(key);
            continue; // 同一缺页窗口等待地形流送，不能每帧重复扫描候选。
        }
        state.mesh_chunk_height_waits.remove(&key); // 页窗口已经变化，本次允许重新查询。
        let Some(layer) = source
            .mesh_layers
            .iter()
            .find(|layer| layer.layer_index == key.layer_index)
        else {
            status.phase = DynamicVegetationPhase::Failed;
            status.detail = "动态Mesh植被队列引用了不存在的正式层".to_owned();
            return;
        };
        let (region_min, region_max) = chunk_bounds(&source, key); // 当前任务只读取一个固定半开块。
        if let Some(snapshot) = clipmap_snapshot.as_ref() {
            match snapshot.request_authoritative_rect(
                Vec2::from_array(region_min),
                Vec2::from_array(region_max),
            ) {
                Ok(true) => {} // 完整块所需Mip 0所有者页均已采用，可以安全提交worker。
                Ok(false) => {
                    state.mesh_chunk_height_waits.insert(
                        key,
                        ground_query_revision.expect("Clipmap快照必须有查询修订"),
                    );
                    state.queued_mesh_chunks.push_back(key);
                    status.detail = "动态Mesh植被正在等待当前空间块的权威Mip 0高度页".to_owned();
                    continue; // 不先扫描全部候选再发现远处粗LOD或缺页。
                }
                Err(error) => {
                    state.queued_mesh_chunks.clear();
                    status.phase = DynamicVegetationPhase::Failed;
                    status.detail = format!("动态Mesh植被请求权威高度页失败：{error}");
                    return;
                }
            }
        }
        let ground = match &source.ground {
            DynamicVegetationGroundSource::Heightfield {
                heights,
                resolution,
                base_height,
                height_scale,
            } => DynamicMeshChunkGround::Heightfield {
                heights: heights.clone(), // Arc浅克隆，不复制整张R16。
                resolution: *resolution,
                base_height: *base_height,
                height_scale: *height_scale,
            },
            DynamicVegetationGroundSource::Clipmap => DynamicMeshChunkGround::Clipmap {
                snapshot: clipmap_snapshot
                    .as_ref()
                    .expect("Clipmap任务已通过同图就绪门")
                    .clone(),
                content_revision: ground_revision.expect("Clipmap快照必须有内容修订"),
                query_revision: ground_query_revision.expect("Clipmap快照必须有查询修订"),
            },
        };
        let job = DynamicMeshChunkJob {
            generation: source.generation, // 完成值绑定当前场景代次。
            revision: source.revision,     // 完成值绑定当前密度和高度修订。
            clearance_revision,            // 完成值绑定当前建筑净空来源。
            clearances: clearance_snapshot.as_ref().map_or_else(
                BuildingVegetationClearanceState::empty_regions,
                |snapshot| snapshot.regions.clone(),
            ), // 只在提交块任务时浅克隆完整四边形数组。
            key,                           // 完成值绑定当前层和块坐标。
            layer: layer.layer.clone(),    // 小型正式描述按任务拥有。
            density: layer.density.clone(), // Arc浅克隆，不复制整张R8。
            ground,                        // 高度后端与当前内容身份同批冻结。
            world_min: source.world_min,   // 全图候选稳定原点。
            world_max: source.world_max,   // 全图合法边界。
            region_min,                    // 当前半开块最小角。
            region_max,                    // 当前半开块最大角。
        };
        state.mesh_chunk_task_identity = Some((
            source.generation,
            source.revision,
            clearance_revision,
            ground_revision,
            key,
        )); // 在途身份先登记再提交，建筑或高度换代后同坐标可以重新排队。
        state.mesh_chunk_task =
            Some(AsyncComputeTaskPool::get().spawn(async move { build_dynamic_mesh_chunk(job) })); // 唯一后台槽完整扫描当前块，绝不按候选或实例数量抽样。
        refresh_residency_status(&state, &mut status); // 状态显示真实在途块而非固定预算。
        break; // 每帧最多提交一个任务且进程内始终single-flight。
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc; // 构造最小不可变源快照。

    use feathrise_scene_content::{SceneVegetationKind, SceneVegetationLayer}; // 测试只需要正式层纯值结构。

    use super::*; // 直接锁定空间块集合不受实例预算影响。

    fn source(world_max: [f32; 2]) -> DynamicVegetationBuild {
        DynamicVegetationBuild {
            volume_mesh_layers: Arc::default(),
            generation: 1, // 固定测试代次。
            revision: 1,   // 固定测试修订。
            mesh_layers: vec![super::super::resources::DynamicMeshLayerBuild {
                layer_index: 3, // 非零层索引证明身份不是数组游标。
                layer: SceneVegetationLayer {
                    id: "tree".to_owned(),                         // 测试层稳定身份。
                    label: "树木".to_owned(),                      // 显示名不参与块计算。
                    kind: SceneVegetationKind::Mesh,                 // 使用Mesh空间常驻路径。
                    runtime_profile: Some("hn01_tree".to_owned()), // 保持完整正式描述。
                    runtime_layer_id: Some(1),                     // 保持完整正式层身份。
                    prototype_id: Some(2),                         // 保持完整正式原型身份。
                    asset_path: "tree.glb".to_owned(),             // 块计算不读取资产路径。
                    density_source_path: "tree.r8".to_owned(),     // 块计算不读取作者源路径。
                    density_texture_path: "tree.ktx2".to_owned(),  // 块计算不读取派生纹理路径。
                    resolution: [2, 2],                            // 最小密度快照尺寸。
                    spacing_meters: 4.0,                           // 作者四米树木间距保持在源中。
                    scale_range: [1.0, 1.0],                       // 姿态不参与块计算。
                    max_instances: 1, // 故意极小，空间常驻绝不能读取该旧兼容字段。
                    seed: 7,          // 稳定种子不参与块集合。
                },
                density: Arc::from(vec![u8::MAX; 4]), // 最小满密度源。
            }],
            blade_layers: Vec::new(), // 块集合测试不需要GPU叶片源。
            ground: DynamicVegetationGroundSource::Heightfield {
                heights: Arc::from(vec![0_u16; 4]), // 最小二乘二高度场。
                resolution: [2, 2],                 // 高度场尺寸与快照一致。
                base_height: 0.0,                   // 高度不参与块集合。
                height_scale: 1.0,                  // 高度不参与块集合。
            },
            world_min: [0.0, 0.0], // 场景从世界原点开始。
            world_max,             // 调用方控制场景块数。
        }
    }

    #[test] // 大场景的期望块数必须随真实LOD范围增长，不存在8192之类全图容量门。
    fn desired_chunks_are_bounded_only_by_scene_and_view_range() {
        let source = source([1_024.0, 1_024.0]); // 八乘八个一百二十八米块。
        let all = desired_layer_chunks(&source, 3, Vec2::splat(512.0), 2_000.0); // 半径覆盖整图。
        assert_eq!(all.len(), 64); // 全部六十四块必须进入常驻集合。
        assert!(all.iter().all(|key| key.layer_index == 3)); // 每个块保留正式层身份。
    }

    #[test] // 分块边界必须完整覆盖非整倍数场景终点。
    fn final_chunk_is_clamped_to_the_scene_boundary() {
        let source = source([300.0, 260.0]); // 每轴都不是一百二十八米整倍数。
        let key = DynamicMeshChunkKey {
            layer_index: 3, // 使用测试正式层身份。
            x: 2,           // 第三列是窄尾块。
            z: 2,           // 第三行是窄尾块。
        };
        assert_eq!(chunk_bounds(&source, key), ([256.0, 256.0], [300.0, 260.0])); // 最终边界不得越出场景。
    }
}
