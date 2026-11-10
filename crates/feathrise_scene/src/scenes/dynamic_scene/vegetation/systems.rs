//! 动态植被事件观察、资源准备、后台构建、原子采用与轻量窗口跟随。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, futures::check_ready};
use feathrise_environment::vegetation::blade::GpuBladeProfileBanks; // 每份候选登记独立绘制槽，不替换兄弟网格目录。
use feathrise_environment::vegetation::{VegetationMeshPrototypeRegistry, VegetationPrototypeId};
use feathrise_scene_content::{SceneVegetationKind, VegetationPresetRegistry};

use super::build::{
    DynamicBladeLayerJob, DynamicMeshLayerJob, DynamicVegetationBuildJob, build_dynamic_vegetation,
};
use super::helpers::{
    copy_r8_base, pending_asset_failure, pending_assets_loaded, prepare_pending_blade_meshes,
    validate_r8_base,
};
use super::resources::{
    DynamicVegetationGroundSource, DynamicVegetationPhase, DynamicVegetationState,
    DynamicVegetationStatus,
};
use super::spawn::create_vegetation_root; // 修订根立即发布Blade窗口，Mesh随后由空间常驻系统按块加入。
use super::transactions::fail_current_revision;
use crate::scenes::dynamic_scene::mesh_profiles::{
    DynamicMeshProfileRequests, DynamicMeshProfileState, DynamicMeshProfileCache,
};
use crate::scenes::dynamic_scene::terrain_derived::{
    DynamicTerrainCommitState, DynamicTerrainHeightCache,
}; // CPU散布与GPU Blade共用同代次GLB候选，正式切换读取共同令牌。

/// 进入统一动态场景时建立独立植被代次；首载未成功前世界中没有半套实例。
pub(in crate::scenes::dynamic_scene) fn begin_dynamic_vegetation(
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    state.begin();
    *status = DynamicVegetationStatus {
        phase: DynamicVegetationPhase::Waiting,
        detail: "正在读取动态场景植被定义".to_owned(),
        ..default()
    };
}

/// 数值资产成功后只扫描一次非零Mesh层；正式原型就绪后冻结共享CPU源，不生成全图逐株数组。
pub(in crate::scenes::dynamic_scene) fn start_dynamic_vegetation_build(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    images: Res<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>, // 发布 Blade 窗口前一次性核对顶点 ABI，并补齐旧 HN-02 网格的 UV0。
    mut profile_requests: InstanceResMut<DynamicMeshProfileRequests>,
    preset_registry: Res<VegetationPresetRegistry>,
    profile_state: InstanceRes<DynamicMeshProfileState>,
    profile_cache: Res<DynamicMeshProfileCache>,
    prototypes: Res<VegetationMeshPrototypeRegistry>,
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    if state.dirty || state.task.is_some() || state.mesh_chunk_task.is_some() {
        return; // 旧空间块必须先自然完成并被身份门丢弃，禁止与新修订源并发形成无界后台工作。
    }
    let Some(pending) = state.pending.as_ref() else {
        return;
    };
    let empty_container = pending.manifest.terrain.is_empty() && pending.layers.is_empty();
    if !empty_container && pending.clipmap_height.is_none() && pending.manifest.terrain.kind != feathrise_scene_content::SceneTerrainKind::Voxel
        && state.terrain_generation != Some(pending.terrain_generation)
    {
        // 分页不重建密度；空容器的零层修订不等待GLB，其他地形仍保留原同代门。
        return; // GLB新代次已到达时旧候选不得启动后台散布。
    }
    if state.attempted_revision == Some((state.generation, state.revision)) {
        return;
    }
    if let Some(error) = pending_asset_failure(&asset_server, pending) {
        fail_current_revision(&mut commands, &mut state, &mut status, error);
        return;
    }
    if !pending_assets_loaded(&asset_server, pending, &images) {
        return;
    }
    let prepared_blade_mesh_count = match prepare_pending_blade_meshes(&mut meshes, pending) {
        Ok(Some(count)) => count, // 全部唯一 LOD 均已拥有 position、normal 与 UV0。
        Ok(None) => return,       // 主世界 Mesh 资产仍在完成最后交接。
        Err(error) => {
            fail_current_revision(&mut commands, &mut state, &mut status, error); // 可读事务失败代替 wgpu 致命验证崩溃。
            return;
        }
    };
    if prepared_blade_mesh_count > 0 {
        info!("动态植被已为{prepared_blade_mesh_count}份旧Blade网格自动补齐根尖UV"); // 首载日志提供资产兼容修复证据。
    }
    if !pending.mesh_profiles_scanned {
        let mut required_profiles = std::collections::BTreeSet::new();
        for layer in &pending.layers {
            if layer.definition.kind != SceneVegetationKind::Mesh {
                continue;
            }
            let Some(image) = images.get(&layer.density) else {
                return;
            };
            let label = format!("植被层{}密度", layer.definition.id);
            let expected = match validate_r8_base(image, layer.definition.resolution, &label) {
                Ok(expected) => expected,
                Err(error) => {
                    fail_current_revision(&mut commands, &mut state, &mut status, error);
                    return;
                }
            };
            let data = image.data.as_ref().expect("R8校验已确认CPU基础级存在");
            if data[..expected].iter().any(|value| *value != 0) {
                let Some(profile) = layer.definition.runtime_profile.as_deref() else {
                    let error = format!("植被层{}缺少正式Mesh profile", layer.definition.id);
                    fail_current_revision(&mut commands, &mut state, &mut status, error);
                    return;
                };
                required_profiles.insert(profile.to_owned());
            }
        }
        let pending = state.pending.as_mut().expect("扫描期间待采用输入仍存在");
        pending.required_mesh_profiles = required_profiles;
        pending.mesh_profiles_scanned = true;
    }
    if !state
        .pending
        .as_ref()
        .expect("扫描后待采用输入仍存在")
        .mesh_profiles_requested
    {
        let required_profiles = state
            .pending
            .as_ref()
            .expect("扫描后待采用输入仍存在")
            .required_mesh_profiles
            .iter()
            .cloned()
            .collect::<Vec<_>>(); // 草稿可在后台预先给出非零层，正式原型请求仍统一发生在主世界。
        for profile in required_profiles {
            if let Err(error) = profile_requests.request(&profile) {
                fail_current_revision(&mut commands, &mut state, &mut status, error);
                return;
            }
        }
        state
            .pending
            .as_mut()
            .expect("原型请求后待采用输入仍存在")
            .mesh_profiles_requested = true; // 稳定等待帧不得反复推进显式重试修订。
    }
    let pending = state.pending.as_ref().expect("资产就绪后待采用输入仍存在");
    if let Some((layer, error)) = pending.layers.iter().find_map(|layer| {
        if layer.definition.kind != SceneVegetationKind::Mesh { return None; }
        let profile = layer.definition.runtime_profile.as_deref()?;
        if !pending.required_mesh_profiles.contains(profile) { return None; }
        profile_state.failure_for(profile, &preset_registry, &profile_cache).map(|error| (layer, error))
    }) {
        // 首载失败归还准备席位，但保留pending及上一完整根；目录换代或兄弟重试后可从原门自动恢复。
        status.phase = DynamicVegetationPhase::Failed;
        status.detail = format!("{}的正式Mesh原型加载失败：{error}", layer.definition.label);
        return;
    }
    let missing_profile = pending.layers.iter().find(|layer| {
        if layer.definition.kind != SceneVegetationKind::Mesh {
            return false;
        }
        let Some(profile) = layer.definition.runtime_profile.as_deref() else {
            return false;
        };
        if !pending.required_mesh_profiles.contains(profile) {
            return false;
        }
        !profile_state.ready_for(profile, &preset_registry, &prototypes)
            || layer
                .definition
                .prototype_id
                .is_none_or(|id| !prototypes.contains(VegetationPrototypeId(id)))
    });
    if let Some(layer) = missing_profile {
        status.phase = DynamicVegetationPhase::Waiting;
        status.detail = format!(
            "正在等待{}的正式四级LOD与风材质原型；不会用普通GLB替代",
            layer.definition.label
        );
        return;
    }
    let expected_height_count = pending.manifest.terrain.resolution[0] as usize
        * pending.manifest.terrain.resolution[1] as usize; // 清单校验已限制维度，乘法不会溢出。
    if pending.clipmap_height.is_none() && pending.manifest.terrain.kind != feathrise_scene_content::SceneTerrainKind::Voxel && !pending.layers.is_empty() // 窗口后端不需要CPU整图。
        && (pending.height_cells.is_none() || pending.heights.len() != expected_height_count)
    {
        fail_current_revision(
            &mut commands,
            &mut state,
            &mut status,
            "植被修订缺少同代次GLB派生CPU/GPU高度".to_owned(),
        );
        return;
    }
    let mut mesh_layers = Vec::new();
    let mut volume_mesh_layers = Vec::new();
    let mut blade_layers = Vec::new(); // Blade原始基础级只为建筑净空派生冻结一次。
    for (layer_index, layer) in pending.layers.iter().enumerate() {
        let Some(density_image) = images.get(&layer.density) else {
            return;
        };
        let label = format!("植被层{}密度", layer.definition.id);
        if layer.definition.kind == SceneVegetationKind::Mesh {
            volume_mesh_layers.push(layer.definition.clone()); // 仅室外实例可因R8为空跳过，三维来源仍按需请求正式原型。
            let profile = layer
                .definition
                .runtime_profile
                .as_deref()
                .expect("Mesh定义已校验正式profile");
            if !pending.required_mesh_profiles.contains(profile) {
                continue; // 空白Mesh画布不复制CPU快照，也不启动无意义的全图候选扫描。
            }
            let source = layer.source_density.clone().map(Ok).unwrap_or_else(|| {
                copy_r8_base(density_image, layer.definition.resolution, &label)
                    .map(std::sync::Arc::from)
            }); // 未保存Mesh草稿直接浅克隆worker冻结值，不在帧线程复制整张R8。
            let density = match source {
                Ok(density) => density,
                Err(error) => {
                    fail_current_revision(&mut commands, &mut state, &mut status, error);
                    return;
                }
            };
            mesh_layers.push(DynamicMeshLayerJob {
                layer_index,
                layer: layer.definition.clone(),
                density,
            });
        } else {
            let source = layer.source_density.clone().map(Ok).unwrap_or_else(|| {
                copy_r8_base(density_image, layer.definition.resolution, &label)
                    .map(std::sync::Arc::from) // 只有磁盘初载才走既有基础级复制。
            }); // 草稿直接共享后台冻结源，连续笔划不在帧线程复制整图。
            let density = match source {
                Ok(density) => density, // 草稿只浅克隆后台共享源；磁盘初载保留原路径。
                Err(error) => {
                    fail_current_revision(&mut commands, &mut state, &mut status, error);
                    return;
                }
            };
            let runtime_layer_id = layer
                .definition
                .runtime_layer_id
                .expect("Blade定义已校验正式运行时层ID");
            blade_layers.push(DynamicBladeLayerJob {
                runtime_layer_id,                        // 四个LOD窗口共享这一正式层身份。
                resolution: layer.definition.resolution, // 内存派生图保持作者尺寸。
                density,                                 // 原始R8进入后台共享源，不改写Image。
            });
        }
    }
    let job = DynamicVegetationBuildJob {
        volume_mesh_layers,
        generation: pending.generation,
        revision: pending.revision,
        mesh_layers,
        blade_layers,
        ground: if pending.clipmap_height.is_some() || pending.manifest.terrain.kind == feathrise_scene_content::SceneTerrainKind::Voxel {
            DynamicVegetationGroundSource::Clipmap // 单块提交时再取得当前局部页与环带。
        } else {
            DynamicVegetationGroundSource::Heightfield {
                heights: pending.heights.clone(), // 所有GLB块浅克隆同一完整R16。
                resolution: pending.manifest.terrain.resolution,
                base_height: pending.manifest.terrain.base_height,
                height_scale: pending.manifest.terrain.height_scale,
            }
        },
        world_min: pending.manifest.world_min,
        world_max: pending.manifest.world_max,
    };
    state.task =
        Some(AsyncComputeTaskPool::get().spawn(async move { build_dynamic_vegetation(job) }));
    state.attempted_revision = Some((state.generation, state.revision));
    status.phase = DynamicVegetationPhase::Building;
    status.detail = "正在后台冻结Mesh植被密度与地表来源；不会裁剪作者实例".to_owned();
}

/// 非阻塞采用当前修订共享源与Blade根；Mesh由后续空间常驻系统逐块精确生成。
pub(in crate::scenes::dynamic_scene) fn install_dynamic_vegetation(
    mut commands: Commands,
    terrain_cache: InstanceRes<DynamicTerrainHeightCache>, // 新GLB候选存在时保持完整植被根隐藏。
    mut blade_profile_banks: ResMut<GpuBladeProfileBanks>, // 新候选独占绘制槽，旧完整根继续持有自己的槽。
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    let completed = state.task.as_mut().and_then(check_ready);
    if let Some(result) = completed {
        state.task = None;
        match result {
            Ok(build) if state.current(build.generation, build.revision) => {
                let created = state
                    .pending
                    .as_ref()
                    .map(|pending| create_vegetation_root(&mut commands, pending, &mut blade_profile_banks));
                match created {
                    Some(Ok(root_build)) => {
                        commands.entity(root_build.root).insert(Visibility::Hidden); // 新GLB候选的Blade窗口完整但暂不混用旧地形。
                        if let Some(old_candidate) = state.candidate_root.replace(root_build) {
                            commands.entity(old_candidate.root).try_despawn(); // 同一时间最多保留一个隐藏候选根。
                        }
                        state.candidate_mesh_source = Some(std::sync::Arc::new(build)); // 分块输入与隐藏Blade根共同等待一次提交。
                        let waits_for_terrain_commit =
                            state.pending.as_ref().is_some_and(|pending| {
                                pending.clipmap_height.is_none() && terrain_cache // 分页不参与GLB联合提交门。
                                    .pending_for(&pending.scene_id)
                                    .is_some_and(|terrain| {
                                        terrain.generation == pending.terrain_generation
                                    })
                            });
                        if waits_for_terrain_commit {
                            let scene_id =
                                &state.pending.as_ref().expect("候选根必须持有输入").scene_id;
                            status.phase = DynamicVegetationPhase::Building;
                            status.detail = format!(
                                "场景{scene_id}植被候选已完整就绪，正在等待同代地形、碰撞与水域"
                            );
                        } else {
                            commit_staged_vegetation(
                                &mut commands,
                                &mut state,
                                &mut status,
                            ); // 独立植被热更仍立即采用本根的完整修订。
                        }
                    }
                    Some(Err(error)) => {
                        fail_current_revision(&mut commands, &mut state, &mut status, error);
                        return;
                    }
                    None => return,
                }
            }
            Ok(_) => return,
            Err(error) => {
                fail_current_revision(&mut commands, &mut state, &mut status, error);
                return;
            }
        }
    }
}

/// 地形共同提交令牌存在时，把同代完整隐藏植被根切换为正式根。
pub(in crate::scenes::dynamic_scene) fn commit_dynamic_vegetation_for_terrain(
    mut commands: Commands,
    commit: InstanceRes<DynamicTerrainCommitState>,
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    let Some(pending) = state.pending.as_ref() else {
        return; // 未声明植被时共同提交门把该域视为已满足。
    };
    if !commit.matches(&pending.scene_id, pending.terrain_generation)
        || !state.candidate_ready_for_terrain(&pending.scene_id, pending.terrain_generation, true)
    {
        return; // 旧修订或尚未分帧生成完整的根不能消费令牌。
    }
    commit_staged_vegetation(
        &mut commands,
        &mut state,
        &mut status,
    );
}

/// 消费一份已经完整生成的植被隐藏根与分块源，并更新公开预算。
fn commit_staged_vegetation(
    commands: &mut Commands,
    state: &mut DynamicVegetationState,
    status: &mut DynamicVegetationStatus,
) {
    let candidate = state.candidate_root.take().expect("候选已满足完整采用门");
    let source = state
        .candidate_mesh_source
        .take()
        .expect("候选根必须拥有同修订Mesh分块源");
    commands
        .entity(candidate.root)
        .insert(Visibility::Inherited);
    // 候选在隐藏建根时已取得自己的绘制槽；提交只切换本域根，不覆盖全世界目录。
    if let Some(old_root) = state.active_root.replace(candidate.root) {
        commands.entity(old_root).try_despawn();
    }
    state.mesh_source = Some(source); // 可见根与CPU散布源在同一提交函数中换代。
    state.resident_mesh_chunks.clear(); // 旧块随旧父根回收，新源从当前视野重新常驻。
    state.queued_mesh_chunks.clear();
    state.desired_mesh_chunks.clear();
    state.residency_focus_chunk = None;
    state.residency_range_bits = None;
    state.residency_clearance_revision = None; // 新植被源必须重新读取当前建筑净空身份。
    state.residency_ground_revision = None; // Clipmap内容身份由当前已采用地表重新取得。
    state.mesh_chunk_height_waits.clear(); // 旧密度或场景的缺页等待不能阻止新源。
    state.blade_clearance_applied_revision = Some((0, None)); // 新根原始密度尚未消费后续作者发布。
    state.collision_focus_cell = None; // 新修订不能复用旧玩家网格身份而漏建首批碰撞。
    state.collision_frame = None;
    let layer_count = state
        .pending
        .as_ref()
        .map_or(0, |pending| pending.layers.len());
    state.failure_logged_revision = None;
    status.phase = DynamicVegetationPhase::Ready;
    status.applied_draft_revision = state.draft_revision; // 与可见根切换同一命令提交边界发布回执。
    status.applied_density_revision = None; // 新根尚待当前净空/密度任务真实替换全部窗口，旧根回执不可继承。
    status.revision = status.revision.wrapping_add(1);
    status.layer_count = layer_count;
    status.blade_layer_count = candidate.blade_layer_count;
    status.blade_window_count = candidate.blade_window_count;
    status.mesh_instance_count = 0;
    status.mesh_batch_count = 0;
    status.mesh_render_node_count = 0;
    status.mesh_collision_proxy_count = 0;
    status.clipmap_ground_revision = None;
    status.detail = format!(
        "动态植被修订已采用：{}层/{}窗GPU叶片；Mesh按视野分块加载，作者密度未裁剪",
        candidate.blade_layer_count, candidate.blade_window_count
    );
    info!("{}，修订{}", status.detail, status.revision);
}

/// 离场释放候选任务、活动根和全部强资产句柄；过期后台任务无法再次采用。
pub(in crate::scenes::dynamic_scene) fn clear_dynamic_vegetation(
    mut commands: Commands,
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    if let Some(staging) = state.mesh_chunk_staging.take() {
        commands.entity(staging.root).try_despawn();
    }
    if let Some(candidate) = state.candidate_root.take() {
        commands.entity(candidate.root).try_despawn();
    }
    if let Some(root) = state.active_root.take() {
        commands.entity(root).try_despawn();
    }
    let next_generation = state.generation.wrapping_add(1).max(1);
    let task = state.task.take(); // 保留无协作取消点的唯一任务，重进动态场景后只轮询并丢弃旧身份结果。
    let blade_clearance_task = state.blade_clearance_task.take(); // 遮罩任务同样自然结束并由新代次拒收。
    let blade_clearance_task_identity = state.blade_clearance_task_identity.take(); // 保留旧联合身份到完成边界。
    let mesh_chunk_task = state.mesh_chunk_task.take(); // 单块任务同样自然完成并由新代次拒收。
    let mesh_chunk_task_identity = state.mesh_chunk_task_identity.take(); // 迟到块完成前保留其完整旧身份。
    *state = DynamicVegetationState {
        generation: next_generation,
        task,
        blade_clearance_task,
        blade_clearance_task_identity,
        mesh_chunk_task,
        mesh_chunk_task_identity,
        ..default()
    };
    *status = DynamicVegetationStatus {
        detail: "已离开动态场景".to_owned(),
        ..default()
    };
}

#[cfg(test)]
#[path = "profile_bank_tests.rs"]
mod profile_bank_tests;
#[cfg(test)]
#[path = "profile_failure_tests.rs"]
mod profile_failure_tests;
#[cfg(test)]
#[path = "empty_definition_tests.rs"]
mod empty_definition_tests;
