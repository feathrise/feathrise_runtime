//! 动态地图水面候选资产分帧登记、RenderApp回执核验与整批原子采用系统。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use crate::shared::large_map::{
    DynamicWaterMaterial, WaterGpuUploadQueue, WaterGpuUploadReceipts, spawn_staged_water_warmup,
}; // 运行时采用复用共享GPU水材质、上传预算与真实管线准备回执。
use bevy::prelude::*; // ECS、Assets、Time以及日志。
use bevy::tasks::futures::check_ready; // Update只非阻塞探测CPU任务完成。
use feathrise_map_content::ActiveDynamicMap; // 当前MapId用于拒收迟到CPU与GPU结果。
use feathrise_scene_core::state::ScenePhase; // 候选管线承接实体随动态地图退出兜底回收。

use crate::maps::dynamic_map::terrain_derived::{
    DynamicTerrainCommitState, DynamicTerrainHeightCache,
}; // 新GLB候选存在时水域只准备隐藏批次，由共同令牌提交。

use super::super::resources::{
    DynamicWaterPhase, DynamicWaterState, DynamicWaterStatus, PendingDynamicWaterBatch,
}; // 候选批、代次身份与公开诊断状态。
use super::super::runtime_batch::{spawn_dynamic_water_batch, stage_dynamic_water_surface}; // 完整后台值分帧登记并在采用门后生成真实实体。

pub(super) fn remove_installed_water(
    commands: &mut Commands,
    _meshes: &mut Assets<Mesh>,
    _materials: &mut Assets<DynamicWaterMaterial>,
    state: &mut DynamicWaterState,
) {
    for installed in state.installed.drain(..) {
        installed.spawn.retire(commands);
    }
    state.installed_map_id = None;
}

fn discard_pending_water(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<DynamicWaterMaterial>,
    uploads: &mut WaterGpuUploadQueue,
    state: &mut DynamicWaterState,
) {
    let Some(mut pending) = state.pending.take() else {
        return;
    };
    if let Some(ticket) = pending.ticket.take() {
        uploads.retire(ticket); // 先撤销RenderApp目标，迟到回执不能进入后续地图或代次。
    }
    for entity in pending.warmup_entities.drain(..) {
        commands.entity(entity).try_despawn();
    }
    for staged in pending.staged.drain(..) {
        staged.discard(commands, meshes, materials);
    }
    crate::retire_water_cpu(commands, pending);
}

/// 完整任务只有地图和代次仍匹配时才生成新资产，并在全部成功后替换上一有效水面。
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_dynamic_water(
    mut commands: Commands,
    time: Res<Time>,
    active_map: InstanceRes<ActiveDynamicMap>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<DynamicWaterMaterial>>,
    mut uploads: ResMut<WaterGpuUploadQueue>,
    receipts: Res<WaterGpuUploadReceipts>,
    terrain_cache: InstanceRes<DynamicTerrainHeightCache>, // 判断本批是否属于尚未提交的地形候选。
    mut state: InstanceResMut<DynamicWaterState>,
    mut status: InstanceResMut<DynamicWaterStatus>,
) {
    let pending_is_stale = state.pending.as_ref().is_some_and(|pending| {
        active_map.map_id.as_ref() != Some(&pending.map_id)
            || state.desired_map_id.as_ref() != Some(&pending.map_id)
            || state.generation != pending.generation
            || state.terrain_generation != Some(pending.terrain_generation)
    });
    if pending_is_stale {
        discard_pending_water(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut uploads,
            &mut state,
        );
    }

    if state.pending.is_none() {
        let Some(task) = state.task.as_mut() else {
            return;
        };
        let Some((map_id, generation, terrain_generation, result)) = check_ready(task) else {
            return;
        };
        state.task = None;
        if active_map.map_id.as_ref() != Some(&map_id)
            || state.desired_map_id.as_ref() != Some(&map_id)
            || state.generation != generation
            || state.terrain_generation != Some(terrain_generation)
        {
            crate::retire_water_cpu(&mut commands, result);
            return; // 过期CPU完成值不登记任何主世界资产。
        }
        let builds = match result {
            Ok(builds) => builds,
            Err(error) => {
                status.phase = DynamicWaterPhase::Failed;
                status.detail = format!("地图{map_id}水面后台构建失败：{error}");
                error!("{}", status.detail);
                return;
            }
        };
        state.pending = Some(PendingDynamicWaterBatch {
            map_id,
            generation,
            terrain_generation,
            remaining: builds.into(),
            staged: Vec::new(),
            warmup_entities: Vec::new(),
            triangle_count: 0,
            ticket: None,
            ready_to_commit: false,
        });
    }

    // 合法空静水不消耗GPU额度；本帧至多登记第一张非空完成Mesh。
    let mut skipped = 0;
    while skipped < 32
        && state
            .pending
            .as_ref()
            .and_then(|pending| pending.remaining.front())
            .is_some_and(|build| build.mesh.is_empty())
    {
        state
            .pending
            .as_mut()
            .expect("上方已确认候选批存在")
            .remaining
            .pop_front();
        skipped += 1;
    }
    if state
        .pending
        .as_ref()
        .and_then(|pending| pending.remaining.front())
        .is_some_and(|build| build.mesh.is_empty())
    {
        return;
    } // 空层也只分批跳过，32是每帧节奏。
    let next_bytes = state
        .pending
        .as_ref()
        .and_then(|pending| pending.remaining.front())
        .map(|build| build.mesh.estimated_gpu_bytes());
    if let Some(next_bytes) = next_bytes {
        match uploads.try_reserve_registration(next_bytes) {
            Ok(false) => return, // editor或另一批已使用本帧全局唯一登记额度。
            Err(error) => {
                let map_id = state
                    .pending
                    .as_ref()
                    .map(|pending| pending.map_id.clone())
                    .expect("存在待登记Mesh时必有候选批");
                discard_pending_water(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut uploads,
                    &mut state,
                );
                status.phase = DynamicWaterPhase::Failed;
                status.detail = format!("地图{map_id}水面GPU分帧预算失败：{error}");
                error!("{}", status.detail);
                return;
            }
            Ok(true) => {
                let build = state
                    .pending
                    .as_mut()
                    .expect("存在待登记Mesh时必有候选批")
                    .remaining
                    .pop_front()
                    .expect("上方已读取队首完成值");
                let pending = state.pending.as_mut().expect("候选批在登记期间保持存在");
                pending.triangle_count += stage_dynamic_water_surface(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut pending.staged,
                    build,
                    time.elapsed_secs(),
                );
            }
        }
    }

    let Some(pending) = state.pending.as_mut() else {
        return;
    };
    if !pending.remaining.is_empty() {
        status.phase = DynamicWaterPhase::Building;
        status.detail = format!(
            "地图{}水面正在按每帧一张Mesh登记GPU资产：已登记{}张，剩余{}张",
            pending.map_id,
            pending.staged.len(),
            pending.remaining.len()
        );
        return;
    }
    if pending.staged.is_empty() {
        // 空水网没有GPU资产，CPU整批成功即可直接进入同一原子采用路径。
    } else if pending.ticket.is_none() {
        for staged in &pending.staged {
            if let Err(error) = staged.rebase_material(&mut materials, time.elapsed_secs()) {
                let map_id = pending.map_id.clone();
                discard_pending_water(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut uploads,
                    &mut state,
                );
                status.phase = DynamicWaterPhase::Failed;
                status.detail = format!("地图{map_id}水面候选材质重置失败：{error}");
                error!("{}", status.detail);
                return;
            }
        }
        pending.warmup_entities = pending
            .staged
            .iter()
            .map(|staged| {
                let entity = spawn_staged_water_warmup(&mut commands, staged);
                commands
                    .entity(entity)
                    .insert(DespawnOnExit(ScenePhase::Active));
                entity
            })
            .collect();
        pending.ticket = Some(uploads.submit(&pending.staged, &pending.warmup_entities)); // 修改与真实管线承接实体齐备后才提交不可追加ticket。
    }
    if pending
        .ticket
        .is_some_and(|ticket| receipts.has_failed(ticket))
    {
        let map_id = pending.map_id.clone();
        discard_pending_water(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut uploads,
            &mut state,
        );
        status.phase = DynamicWaterPhase::Failed;
        status.detail = format!("地图{map_id}水面透明PBR管线编译失败；保留上一完整水面");
        error!("{}", status.detail);
        return;
    }
    if pending
        .ticket
        .is_some_and(|ticket| !receipts.is_ready(ticket))
    {
        status.phase = DynamicWaterPhase::Building;
        status.detail = format!(
            "地图{}的{}张候选水面正在等待RenderApp连续GPU资产准备回执",
            pending.map_id,
            pending.staged.len()
        );
        return;
    }

    let pending = state.pending.as_mut().expect("候选批刚通过完整采用门");
    pending.ready_to_commit = true; // 从此不再登记资产，只等待独立或地形共同提交。
    if terrain_cache
        .pending_for(&pending.map_id)
        .is_some_and(|terrain| terrain.generation == pending.terrain_generation)
    {
        status.phase = DynamicWaterPhase::Building;
        status.detail = format!(
            "地图{}水域候选已完整就绪，正在等待同代地形与植被",
            pending.map_id
        );
        return; // 旧正式水面继续显示，不能提前混用新高度版本。
    }
    commit_pending_water_batch(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut uploads,
        &mut state,
        &mut status,
    ); // 非地形引起的水域独立热更保持原有立即原子采用语义。
}

/// 地形共同提交令牌存在时，把已就绪隐藏水域批次切为正式批次。
pub(crate) fn commit_dynamic_water_for_terrain(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<DynamicWaterMaterial>>,
    mut uploads: ResMut<WaterGpuUploadQueue>,
    commit: InstanceRes<DynamicTerrainCommitState>,
    mut state: InstanceResMut<DynamicWaterState>,
    mut status: InstanceResMut<DynamicWaterStatus>,
) {
    let Some(pending) = state.pending.as_ref() else {
        return; // 未声明水域时共同提交门把该域视为已满足。
    };
    if !pending.ready_to_commit || !commit.matches(&pending.map_id, pending.terrain_generation) {
        return; // 独立水域修订或旧地形候选不得消费令牌。
    }
    commit_pending_water_batch(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut uploads,
        &mut state,
        &mut status,
    );
}

/// 消费一份已经完成CPU、GPU和管线预热的水域候选。
fn commit_pending_water_batch(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<DynamicWaterMaterial>,
    uploads: &mut WaterGpuUploadQueue,
    state: &mut DynamicWaterState,
    status: &mut DynamicWaterStatus,
) {
    let mut pending = state.pending.take().expect("提交前必须存在完整水域候选");
    debug_assert!(pending.ready_to_commit); // 不能绕过GPU与管线回执门。
    if let Some(ticket) = pending.ticket.take() {
        uploads.retire(ticket);
    }
    for entity in pending.warmup_entities.drain(..) {
        commands.entity(entity).try_despawn();
    }
    let map_id = pending.map_id;
    let generation = pending.generation;
    let next = spawn_dynamic_water_batch(commands, &map_id, pending.staged);
    remove_installed_water(commands, meshes, materials, state);
    let triangle_count = pending.triangle_count;
    let layer_count = next.len();
    state.installed = next;
    state.installed_map_id = Some(map_id.clone());
    status.phase = DynamicWaterPhase::Ready;
    status.applied_generation = generation; // 只有整批实体成功安装后才发布当前完成证据。
    status.revision = status.revision.wrapping_add(1);
    status.layer_count = layer_count;
    status.triangle_count = triangle_count;
    status.detail = format!(
        "地图{map_id}水域已就绪：{layer_count}个非空水体，{triangle_count}个三角形，湖面流向、三维河流/急流、瀑布幕布与河流实际高度踏水已启用"
    );
    info!("{}", status.detail);
}

/// 离开动态地图时回收运行时水面与孤儿资产；F12预览由编辑器自己的会话清理。
pub(crate) fn clear_dynamic_water(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<DynamicWaterMaterial>>,
    mut uploads: ResMut<WaterGpuUploadQueue>,
    mut state: InstanceResMut<DynamicWaterState>,
    mut status: InstanceResMut<DynamicWaterStatus>,
) {
    discard_pending_water(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut uploads,
        &mut state,
    );
    remove_installed_water(&mut commands, &mut meshes, &mut materials, &mut state);
    state.mask_handles.clear();
    state.mask_asset_ids.clear();
    state.mask_paths.clear();
    state.terrain_generation = None; // 离图后不再接受旧GLB派生高度代次。
    state.desired_map_id = None;
    state.dirty = false;
    state.generation = state.generation.wrapping_add(1).max(1); // 零只用于公开Disabled状态。
    // 不丢弃仍在运行的CPU任务句柄；下次进入后先轮询并按上方新代次拒收，保持全局single-flight。
    status.phase = DynamicWaterPhase::Disabled;
    status.map_id = None;
    status.desired_generation = 0; // 离图后旧输入代次不再属于任何当前地图。
    status.applied_generation = 0; // Disabled明确清空上一地图原子采用证据。
    status.layer_count = 0;
    status.triangle_count = 0;
    status.detail = "已离开动态地图".to_owned();
}
