//! 多层碰撞请求身份与单飞阶段驱动；IO和原子发布各由独立模块负责。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use super::super::resources::{DynamicSceneRuntimeState, DynamicSceneRuntimeStatus};
use super::{
    build,
    lifecycle::{fail, reject_input, reset},
    publication::adopt,
    resources::*,
}; // 分离阶段职责。
use bevy::{
    asset::{AssetEvent, LoadState},
    ecs::system::SystemParam,
    prelude::*,
    tasks::{AsyncComputeTaskPool, futures::check_ready},
}; // 独立资产游标与非阻塞阶段。
use crate::SceneRuntimeObserver;
use feathrise_scene_core::residency::TerrainCollisionRegion; // 玩家中心活动区。
use feathrise_scene_content::sparse_height::{SparseHeightLiveOverride, SparseHeightSessionCrater}; // 当前不可变覆盖身份。
use feathrise_scene_content::{
    ActiveDynamicScene, SceneCollisionLodStatus, SceneContentRegistry, SceneManifestAsset, SceneTerrainKind,
    SparseHeightManifestAsset, SparseHeightTileAsset, validate_collision_lods,
}; // 受检清单与状态。
use std::{sync::Arc, time::Instant}; // 小型请求快照共享，真实延迟不参与请求身份。
#[derive(SystemParam)] // 聚合只读资产输入，避免超出Bevy系统参数数量。
pub(crate) struct LayeredAssets<'w, 's> {
    // 独立消息游标，不抢占显示或旧模式事件。
    pub(super) server: Res<'w, AssetServer>, // IO仍走正式资产源。
    pub(super) active: InstanceRes<'w, ActiveDynamicScene>, // 当前场景选择。
    pub(super) registry: Option<feathrise_scene_core::residency::InstanceRes<'w, SceneContentRegistry>>, // 独立测试允许目录尚未就绪。
    pub(super) scenes: Res<'w, Assets<SceneManifestAsset>>, // 正式已保存配置。
    pub(super) manifests: Res<'w, Assets<SparseHeightManifestAsset>>, // 受检高度索引。
    pub(super) tiles: Res<'w, Assets<SparseHeightTileAsset>>, // 正式块强引用。
    pub(super) live: crate::scenes::dynamic_scene::author_sources::TerrainAuthorRes<'w, 's, SparseHeightLiveOverride>, // 作者临时覆盖。
    pub(super) craters: InstanceRes<'w, SparseHeightSessionCrater>, // 会话破坏。
    pub(super) events: MessageReader<'w, 's, AssetEvent<SparseHeightManifestAsset>>, // 正式高度修订。
} // 不深拷贝清单。

pub(crate) fn drive_layered_collision(
    frame: feathrise_scene_core::residency::InstanceFrame,
    mut commands: Commands,
    mut assets: LayeredAssets,
    players: Query<&Transform, With<SceneRuntimeObserver>>,
    mut runtime: InstanceResMut<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
    mut state: InstanceResMut<LayeredCollisionRuntime>,
    mut region: InstanceResMut<TerrainCollisionRegion>,
    mut feedback: InstanceResMut<SceneCollisionLodStatus>,
    mut topology: Option<InstanceResMut<super::super::terrain_topology::TerrainTopologyRuntime>>,
) {
    // Clipmap唯一碰撞写侧。
    super::retirement::poll(&mut state); // 包括输入未就绪的帧也推进后台释放，不阻塞玩家需求更新。
    let Some((scene_id, root, path)) = runtime
        .active_scene_id
        .as_ref()
        .zip(runtime.root)
        .zip(runtime.sparse_height_path.as_ref())
        .map(|((id, root), path)| (id.clone(), root, path.clone()))
    else {
        if runtime.sparse_height_path.is_none() && (state.scene_id.is_some() || region.enabled) {
            super::lifecycle::deactivate(&mut commands, &mut state, &mut region, &mut feedback);
        } // 同一动态场景内改用GLB也必须解除旧活动门，不能等待不存在的OnExit。
        return;
    }; // 地形根必须先就绪。
    if assets.active.scene_id.as_ref() != Some(&scene_id) {
        return;
    } // 拒绝切换场景间隙。
    let Some(map) = assets
        .registry
        .as_deref()
        .and_then(|registry| registry.handles(&scene_id))
        .and_then(|handles| assets.scenes.get(&handles.manifest))
    else {
        return;
    }; // 只消费当前正式配置。
    if map.terrain.kind != SceneTerrainKind::SparseClipmap {
        return;
    } // 原始GLB地形不消费多层碰撞设置。
    if state.scene_id.as_ref() != Some(&scene_id)
        || state.root != Some(root)
        || state.path.as_ref() != Some(&path)
    {
        // 场景完整身份换代。
        reset(&mut commands, &mut state); // 不丢弃尚未结束的旧任务。
        state.scene_id = Some(scene_id.clone()); // 新图身份。
        state.root = Some(root); // 新根身份。
        state.path = Some(path.clone()); // 新高度源。
        state.manifest = Some(assets.server.load(path)); // IO异步读取。
        region.enabled = true; // 清单加载期间也阻止无承托下落。
        region.supported_bounds.clear(); // 未完成候选不提供伪承托。
        region.supported_polygon.clear(); // 切换场景时不沿用旧圆形承托边界。
        region.supported_rows = None; // 旧图完整格行索引同时清理。
        region.revision = region.revision.wrapping_add(1); // 活动缓存失效。
        runtime.collision_ready = false; // 首载不能提前放行。
        *feedback = SceneCollisionLodStatus {
            scene_id: Some(scene_id.clone()),
            detail: "正在准备多层碰撞".into(),
            ..default()
        }; // 显示真实阶段。
    } // 同图保存仅换配置，不清空已采用块。
    let handle = state.manifest.as_ref().expect("新图已建立清单句柄").clone(); // 小句柄克隆。
    if assets.events.read().any(|event| matches!(event, AssetEvent::Added { id } | AssetEvent::Modified { id } | AssetEvent::Removed { id } | AssetEvent::LoadedWithDependencies { id } if *id == handle.id())) { state.manifest_revision = state.manifest_revision.wrapping_add(1); } // 独立正式修订。
    let center = players
        .iter()
        .next()
        .map(|player| frame.to_local(player.translation).xz())
        .unwrap_or(Vec2::new(map.default_spawn[0], map.default_spawn[2])); // 首载用出生点，不跟相机或F12。
    if region.center != center {
        region.center = center;
        region.revision = region.revision.wrapping_add(1);
    } // 活动资格始终围绕真实当前玩家。
    feedback.adopted_center_lag_meters = state
        .applied
        .as_ref()
        .map_or(0.0, |old| center.distance(old.anchor)); // 实时显示已采用中心落后距离，不只在发布帧取样。
    if feedback.saved_lods != map.terrain.collision_lods {
        feedback.saved_lods = map.terrain.collision_lods.clone();
    } // 保存与采用分开回显。
    region.max_distance_meters = state
        .applied
        .as_ref()
        .and_then(|request| request.lods.last())
        .or_else(|| map.terrain.collision_lods.last())
        .map_or(0.0, |lod| lod.max_distance_meters); // 配置替换完成前沿用旧活动半径。
    if matches!(assets.server.load_state(handle.id()), LoadState::Failed(_)) {
        reject_input(
            &mut state,
            &mut feedback,
            &mut status,
            "稀疏高度清单加载失败".into(),
        );
        return;
    } // 缺源不冒充平地。
    let Some(manifest) = assets.manifests.get(&handle).map(|asset| &asset.manifest) else {
        return;
    }; // 等待正式资产。
    if manifest.scene_id != scene_id {
        reject_input(
            &mut state,
            &mut feedback,
            &mut status,
            "高度清单场景身份不一致".into(),
        );
        return;
    } // 内容必须属于当前场景。
    let spec = &manifest.spec; // 独立物理不能依赖显示系统代验场景规格。
    if spec.world_min != map.world_min
        || spec.world_max != map.world_max
        || spec.base_height != map.terrain.base_height
        || spec.height_scale != map.terrain.height_scale
        || spec.cells[0] + 1 != u64::from(map.terrain.resolution[0])
        || spec.cells[1] + 1 != u64::from(map.terrain.resolution[1])
    {
        reject_input(
            &mut state,
            &mut feedback,
            &mut status,
            "高度清单与场景地形规格不一致".into(),
        );
        return;
    } // 禁止采用另一个坐标或编码空间的地面。
    if let Err(error) = validate_collision_lods(&map.terrain.collision_lods, spec) {
        reject_input(&mut state, &mut feedback, &mut status, error);
        return;
    } // 在量化前拒绝零倍率、非有限距离等坏输入。
    if state
        .applied
        .as_ref()
        .is_some_and(|old| old.spec != manifest.spec)
    {
        reject_input(
            &mut state,
            &mut feedback,
            &mut status,
            "地形规格改变需要重新进入场景".into(),
        );
        return;
    } // 不跨规格复用旧块。
    let live = (assets.live.scene_id() == Some(&scene_id)).then_some(&*assets.live); // 不带入其他场景草稿。
    let craters = (assets.craters.scene_id() == Some(&scene_id)).then_some(&*assets.craters); // 会话身份独立。
    let stitch_profiles=map.terrain.resolved_stitches.clone();
    let stitches=state.desired.as_ref().filter(|r|Arc::ptr_eq(&r.stitch_profiles,&stitch_profiles)).map(|r|r.stitches.clone())
        .unwrap_or_else(||Arc::new(super::super::clipmap::stitches::collision_constraints(&stitch_profiles)));
    let mut request = Request {
        stitch_profiles,
        stitches,
        holes: topology
            .as_deref()
            .filter(|value| value.scene_id.as_ref() == Some(&scene_id) && value.root == Some(root))
            .map_or_else(Arc::default, |value| value.desired.clone()),
        scene_id: scene_id.clone(),
        root,
        spec: manifest.spec.clone(),
        manifest_revision: state.manifest_revision,
        live_revision: live.map(SparseHeightLiveOverride::revision),
        crater_revision: craters.map(SparseHeightSessionCrater::revision),
        lods: map.terrain.collision_lods.clone(),
        anchor: center, // 先冻结完整内容身份，再按候选阶段选择稳定世界锚点。
    }; // 小型完整需求快照。
    let preparing = state.failed_ticket != Some(state.ticket)
        && (state.task.is_some() || state.layout.is_some()); // 只有有效在途候选才允许短暂合并移动。
    request.anchor = if !preparing && state.last_player_center == Some(center) {
        center
    } else {
        build::select_request_anchor(
            &request,
            state.desired.as_deref(),
            center,
            preparing,                // 失败后遗留布局不能抑制正常刷新。
            state.last_player_center, // 只识别单帧跨覆盖跳转；普通移动合并到这个最新位置，下一轮追赶。
        )
    }; // 停步后精确回到玩家圆心；移动时有界合并，首帧/传送及时重定向。
    state.last_player_center = Some(center); // 小型标量状态，不扫描网格。
    if state.desired.as_deref() != Some(&request) {
        // 只有真实输入变化才推进票据。
        state.ticket = state.ticket.wrapping_add(1); // 在途结果不再拥有发布权。
        state.desired = Some(Arc::new(request)); // 只保留最新需求。
        state.request_started = Some(Instant::now()); // 当前票据从冻结到完整采用，包含排队、IO和两阶段后台。
        let old_layout = state.layout.take(); // 需求换代只移动所有权，不在主帧析构规划树。
        super::retirement::candidate(&mut state, old_layout); // 旧候选后台退役，不丢CPU槽。
        state.failed_ticket = None; // 新输入允许一次重新尝试。
        feedback.detail = "已保存，碰撞后台准备中；暂用上一有效版本".into(); // 不误报即时生效。
    } // 稳定帧不创建新任务。
    super::retirement::poll(&mut state); // 新增退役负载先派发；当前需求与票据已完成更新。
    if super::retirement::busy(&state) {
        return;
    } // 退役期间不接收第二完成值，唯一旧CPU任务继续占槽。
    if let Some(result) = state.task.as_mut().and_then(check_ready) {
        // 从不阻塞主线程。
        if let Completion::Cooked(Ok(candidate)) = &result.completion {
            feedback.peak_shape_count = feedback
                .peak_shape_count
                .max(state.resident.len() + candidate.shapes.len());
        } // 迟到候选也曾占用形状内存，不因拒收而漏记重叠数量。
        state.task = None; // 实际完成后才归还并发槽。
        if result.ticket == state.ticket
            && state.desired.as_deref() == Some(result.request.as_ref())
        {
            match result.completion {
                // 所有阶段必须完全匹配最新票据。
                Completion::Layout(Ok(layout)) => {
                    // 转入有界资产准备。
                    let previous = state.layout.replace(layout); // 正常为空，异常旧候选也不能在主帧析构。
                    super::retirement::candidate(&mut state, previous); // 固定槽后台归还。
                } // 当前完整布局继续等待源页。
                Completion::Cooked(Ok(cooked)) => {
                    if !super::publication::preserves_player_ground(&region, &cooked) {
                        // 掉头后候选不承托当前玩家时，不撤掉仍有效的旧地面。
                        state.desired = None; // 下一帧以最新位置重新冻结请求，失败票据不锁住重试。
                        let old_layout = state.layout.take(); // 只撤回失去发布权的候选。
                        super::retirement::candidate(&mut state, old_layout); // 不在主帧析构大型布局。
                        super::retirement::completion(&mut state, Completion::Cooked(Ok(cooked))); // 完成但不承托玩家的真实网格后台释放。
                        feedback.detail = "碰撞候选位置落后，保留已有地面并追赶当前位置".into(); // 不冒充后台错误或已经采用。
                        return; // CPU槽已经归还，下一帧允许最新请求启动。
                    } // 内容身份校验仍在上层严格执行。
                    if let Err(error) = adopt(
                        &mut commands,
                        &mut state,
                        &mut region,
                        &mut feedback,
                        &mut runtime,
                        cooked,
                    ) {
                        fail(&mut state, &mut feedback, &mut status, error);
                    } else if let Some(topology) = topology.as_deref_mut() {
                        topology.adopt(
                            state
                                .applied
                                .as_ref()
                                .expect("已完整采用碰撞")
                                .holes
                                .clone(),
                        );
                    }
                } // 同步提交仅完整集合。
                Completion::Layout(Err(error)) | Completion::Cooked(Err(error)) => {
                    fail(&mut state, &mut feedback, &mut status, error)
                } // 保留旧集合。
            }
        } else {
            super::retirement::completion(&mut state, result.completion);
        } // 迟到结果只移交后台释放，不产生世界副作用。
    } // 接收后允许下一阶段启动。
    super::retirement::poll(&mut state); // 成功替换或拒收可能新增释放工作。
    if super::retirement::busy(&state) {
        return;
    } // 旧版实际释放后才允许新一批候选启动。
    if state.task.is_some() || state.failed_ticket == Some(state.ticket) {
        return;
    } // 背压与失败去重。
    let desired = Arc::clone(state.desired.as_ref().expect("当前需求已建立")); // 异步共享小身份。
    if state.applied.as_deref() == Some(desired.as_ref()) {
        runtime.collision_ready = true;
        if !feedback.detail.starts_with("已采用") {
            feedback.detail = "已采用：多层共享碰撞（实验）".into();
        } // 外部坏配置撤回原值时恢复真实反馈，不留下错误锁定。
        return;
    } // 稳定帧没有页面或块遍历。
    if let Err(error) = validate_collision_lods(&desired.lods, &desired.spec) {
        fail(&mut state, &mut feedback, &mut status, error);
        return;
    } // 外部坏配置同样拒绝。
    let ticket = state.ticket; // 每个阶段冻结同一票据。
    if state.layout.is_none() {
        // 先后台规划，不在主帧做512平方邻接检查。
        let previous = state
            .applied
            .as_ref()
            .zip(state.applied_layout.as_ref())
            .map(|(request, layout)| (Arc::clone(request), Arc::clone(layout))); // 主线程只克隆两个Arc，不复制网格或依赖集合。
        state.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            TaskResult {
                ticket,
                request: Arc::clone(&desired), // 完成值保留完整冻结身份。
                completion: Completion::Layout(
                    build::prepare_layout_parallel(
                        Arc::clone(&desired),
                        previous,
                        AsyncComputeTaskPool::get(),
                        super::parallel::MAX_WORKERS,
                    )
                    .await,
                ), // 仅后台协调器等待有界worker；主世界仍非阻塞轮询整个候选。
            }
        })); // 唯一CPU任务。
        return; // 等待下一帧非阻塞接收。
    } // 布局完整后才请求真实需要的基础源页。
    super::loading::load_and_cook(
        &assets,
        &mut state,
        desired,
        manifest,
        live,
        craters,
        &mut feedback,
        &mut status,
    ); // 完整布局进入有界源页准备。
} // 驱动只负责请求身份和异步阶段切换。
