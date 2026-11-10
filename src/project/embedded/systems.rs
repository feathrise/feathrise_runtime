//! 常驻游戏的控制状态机；原子来源发布与真正可试玩分开确认，按钮交接不销毁任何 World。

use super::{
    EmbeddedProjectState,
    asset_overlay::{AssetOverlay, Publication},
    resources::{ApplyingSnapshot, PendingMode, Runtime, Viewport},
    transport::Inbound,
};
use bevy::prelude::*;
use feathrise_editor_protocol::{Message, Mode};
use feathrise_scene_core::input::EditorInputState;
use feathrise_scene_content::{ActiveDynamicScene, SceneId, SparseHeightLiveOverride, TerrainId};
use feathrise_scene::{
    DynamicSceneBoundaryPhase, DynamicSceneBoundaryStatus, DynamicSceneRuntimePhase,
    DynamicSceneRuntimeStatus, DynamicVegetationPhase, DynamicVegetationStatus, DynamicWaterPhase,
    DynamicWaterStatus, ScenePlacementRuntimePhase, ScenePlacementRuntimeStatus, SubsceneInstance,
};
use feathrise_scene_core::residency::InstanceResourceTable;
use feathrise_scene_core::state::{LoadingProgress, PendingSceneTransition, ScenePhase, SceneStartupGate};
use std::{
    collections::BTreeMap,
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Resource, Default)]
pub(super) struct HeightSources {
    scene_id: Option<SceneId>,
    // 私有表不跨任务共享；单画布更新只改一个键，避免帧内Arc写时复制完整地形集合。
    sources: BTreeMap<TerrainId, SparseHeightLiveOverride>,
    cursor: Option<Entity>,
    seen: BTreeMap<TerrainId, u64>,
    scan: u64,
    seen_count: usize,
    pub ready: bool,
}

/// 完整快照序号与作者雕刻修订不是同一时钟；共用来源票据，避免内容不同却撞上同一缓存身份。
pub(super) fn next_height_source_revision() -> Result<u64, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |value|value.checked_add(1))
        .map_err(|_| "实时高度来源票据已耗尽".into())
}

pub(super) fn drive(world: &mut World) {
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else {
        return;
    };
    if runtime.failed || runtime.stopping {
        world.insert_non_send(runtime);
        return;
    }
    runtime.frames += 1;
    let result = drive_inner(world, &mut runtime);
    if let Err(error) = result {
        eprintln!("工程控制连接失败：{error}");
        let _ = runtime.transport.send(Message::Failure {
            request_id: 0,
            code: "guest-control".into(),
            message: error,
        });
        runtime.failed = true;
        #[cfg(feature = "function_hotpatch")]
        super::hotpatch::stop(world);
        world.write_message(AppExit::error());
    }
    world.insert_non_send(runtime);
}

fn drive_inner(world: &mut World, runtime: &mut Runtime) -> Result<(), String> {
    // 生产事件保留期内无法完整消费时，不能继续凭缺失证据放行或让修订永远悬挂。
    // 断开本次会话并由宿主保留作者现场；下一进程从完整最新快照恢复。
    if runtime.source_evidence_lost {
        return Err("生产资产事件超过本次采用预算，来源完成证据丢失，请重连工程视口".into());
    }
    if let Some(value) = runtime.retired.take() {
        if let Err(error) = runtime.transport.retire.try_send(value) {
            runtime.retired = Some(match error {
                std::sync::mpsc::TrySendError::Full(value)
                | std::sync::mpsc::TrySendError::Disconnected(value) => value,
            });
        }
    }
    // 大来源尚未交给后台释放时暂停消费；管道队列和退役队列各自有界，禁止帧内堆积快照。
    #[cfg(feature = "function_hotpatch")]
    let patch_busy = super::hotpatch::busy(world);
    #[cfg(not(feature = "function_hotpatch"))]
    let patch_busy = false;
    // 补丁装载至主App确认之间延后消费场景／模式命令，避免核包后又换代；画面、输入和模拟继续。
    if runtime.retired.is_none() && !patch_busy {
        for _ in 0..8 {
            let input = match runtime.transport.incoming.try_recv() {
                Ok(value) => value,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("控制读取线程已经结束".into());
                }
            };
            match input {
                Inbound::Disconnected(reason) => return Err(reason),
                Inbound::Rejected {
                    transaction,
                    generation,
                    reason,
                } => {
                    runtime.transport.identity.generation = generation;
                    runtime.transport.send(Message::Failure {
                        request_id: transaction,
                        code: "snapshot-rejected".into(),
                        message: reason,
                    })?;
                }
                Inbound::Snapshot(value) => {
                    if runtime.pending.is_some() {
                        return Err("前一草稿尚未确认，宿主又发送了下一份快照".into());
                    }
                    if world.resource::<EmbeddedProjectState>().mode == Mode::Playing {
                        runtime.retired = Some(Box::new(value));
                        runtime.transport.send(Message::Failure {
                            request_id: 0,
                            code: "playing-snapshot".into(),
                            message: "预览期间不能覆盖作者来源，请先返回编辑".into(),
                        })?;
                    } else {
                        super::preview::snapshot_started(world, runtime)?;
                        runtime.pending = Some(ApplyingSnapshot {
                            value,
                            cursor: 0,
                            published: false,
                            quiescent_frame: None,
                        });
                    }
                    break;
                }
                Inbound::Preview(value) => super::preview::receive(world, runtime, value)?,
                Inbound::Control(message) => {
                    let terminal = matches!(message, Message::ApplyCodePatch { .. } | Message::Stop);
                    receive_control(world, runtime, message)?;
                    if runtime.stopping { return Ok(()); }
                    // 同一帧入槽后也立即结束消费，不能让后续模式／场景消息越过刚开启的补丁事务。
                    if terminal { break; }
                }
            }
        }
    }
    publish_snapshot(world, runtime)?;
    super::preview::advance(world, runtime)?;
    poll_source_readiness(world, runtime);
    apply_mode(world, runtime)?;
    let editing = world.resource::<EmbeddedProjectState>().mode == Mode::Editing;
    let transitioning = world.resource::<EmbeddedProjectState>().transitioning;
    world.resource_mut::<EditorInputState>().scene_editor_open = editing
        || transitioning || runtime.native_handoff.is_some() || runtime.native_result.is_some();
    if !editing
        && !runtime.requested_return
        && (runtime.window_return_requested || super::preview_toolbar::take_return_request(world))
    {
        runtime.transport.send(Message::RequestMode {
            mode: Mode::Editing,
        })?;
        runtime.requested_return = true;
        runtime.window_return_requested = false;
        super::window::suspend_input(world, runtime);
    }
    if runtime.ready && runtime.last_status.elapsed() >= Duration::from_millis(250) {
        let state = *world.resource::<EmbeddedProjectState>();
        runtime.transport.send(Message::Status {
            world_id: state.world_id,
            revision: state.revision,
            frame: runtime.frames,
            ticks: runtime.ticks,
            mode: state.mode,
        })?;
        runtime.last_status = Instant::now();
    }
    Ok(())
}

fn receive_control(
    world: &mut World,
    runtime: &mut Runtime,
    message: Message,
) -> Result<(), String> {
    match message {
        Message::Hello { .. } if !runtime.hello => runtime.hello = true,
        #[cfg(feature = "function_hotpatch")]
        Message::ApplyCodePatch { patch_id, source_generation, base_build, artifact, sha256 } => {
            super::hotpatch::receive(world, runtime, patch_id, source_generation, base_build, artifact, sha256);
        }
        Message::SetCamera { camera } => runtime.camera = Some(camera),
        Message::SetAuthorView { view } => runtime.author_view = Some(view),
        Message::SetOverlay { revision, lines } => runtime.overlay = Some((revision, lines)),
        Message::SetAuthorTools { revision, preview } => runtime.author_tools = Some((revision, preview)),
        Message::SetViewport {
            x,
            y,
            width,
            height,
            visible,
            focused,
            excluded_rects,
            excluded_shapes,
        } => {
            runtime.viewport = Viewport {
                x,
                y,
                width,
                height,
                visible,
                focused,
                excluded_rects,
                excluded_shapes,
            };
        }
        Message::SetMode {
            request_id,
            mode,
            required_revision,
        } => {
            if runtime.pending_mode.is_some() {
                return Err("前一模式请求尚未完成".into());
            }
            runtime.pending_mode = Some(PendingMode {
                request_id,
                mode,
                revision: required_revision,
                started: Instant::now(),
                native_started: false,
            });
        }
        Message::Stop => {
            #[cfg(feature = "function_hotpatch")]
            super::hotpatch::stop(world);
            runtime.stopping = true;
            runtime.pending_mode = None;
            runtime.native_handoff = None;
            runtime.native_result = None;
            runtime.window_return_requested = false;
            super::window::suspend_input(world, runtime);
            runtime.transport.send(Message::Stopped)?;
            world.write_message(AppExit::Success);
        }
        _ => return Err("工程端收到不属于宿主命令方向的消息".into()),
    }
    Ok(())
}

fn publish_snapshot(world: &mut World, runtime: &mut Runtime) -> Result<(), String> {
    let Some(pending) = runtime.pending.as_mut() else {
        return Ok(());
    };
    if !pending.published {
        // 同一来源版本的重载必须先采用或失败，再发布下一个版本。
        // 发布前还要排空首载任务及其 PostUpdate 事件，避免旧事件冒充本次修改采用。
        if !runtime.sources_settled {
            return Ok(());
        }
        if let Some(placements) = pending.value.placements.as_mut() {
            match placements.verify(world, &pending.value.scene_id) {
                Ok(false) => return Ok(()),
                Ok(true) => {
                    // 根源已完整验证；正式对象文档由增量入口接回，不再发资产Modified事件。
                    pending.value.paths.retain(|path| path != &placements.path);
                    pending.value.reload_paths.retain(|path| path != &placements.path);
                    pending.value.reads = Arc::default();
                }
                Err(()) => { pending.value.placements = None; } // 不完整活动根回到原生产loader/候选门。
            }
        }
        if pending.quiescent_frame.is_none() {
            let server = world.resource::<AssetServer>();
            let end = (pending.cursor + 128).min(pending.value.reload_paths.len());
            for path in &pending.value.reload_paths[pending.cursor..end] {
                if super::source_readiness::loading(server, path) {
                    return Ok(());
                }
            }
            pending.cursor = end;
            if end != pending.value.reload_paths.len() {
                return Ok(());
            }
            pending.quiescent_frame = Some(runtime.frames);
            pending.cursor = 0;
            return Ok(());
        }
        if pending
            .quiescent_frame
            .is_some_and(|frame| runtime.frames <= frame)
        {
            return Ok(());
        }
        if pending.value.clear_water {
            // 正式水来源即将发布才归还旧局部草稿；最多四个目标/帧，旧像素在独立回收槽释放。
            if !runtime.previews.clear_water(world)? { return Ok(()); }
            pending.value.clear_water = false;
        }
        if pending.value.clear_vegetation {
            if !runtime.previews.clear_vegetation(world)? { return Ok(()); }
            pending.value.clear_vegetation = false;
        }
        let overlay = world.resource::<AssetOverlay>();
        let Ok(old) = overlay.publish(Publication {
            files: pending.value.files.clone(),
            reads: pending.value.reads.clone(),
        }) else {
            return Ok(());
        };
        let old_paths =
            std::mem::replace(&mut runtime.paths, std::mem::take(&mut pending.value.paths));
        let old_reads = std::mem::replace(&mut runtime.source_reads, pending.value.reads.clone());
        let old_handles = std::mem::take(&mut runtime.source_handles);
        runtime.retired = Some(Box::new((old, old_paths, old_reads, old_handles)));
        runtime.sources_settled = false;
        runtime.sources_ready = false;
        runtime.source_failed = false;
        runtime.transport.identity.generation = pending.value.generation;
        pending.published = true;
    }
    let server = world.resource::<AssetServer>();
    let end = (pending.cursor + 128).min(pending.value.reload_paths.len());
    for path in &pending.value.reload_paths[pending.cursor..end] {
        // 生产场景常只持有 GLB 的 Scene0 子资产；根句柄可已释放，不能因此跳过新版本就绪。
        // 本次来源暂持根 Gltf，确保所有子资产经过真实 loader，并以其资产事件作为完成依据。
        if path.ends_with(".glb") && pending.value.files.contains_key(path) {
            let existing = server.get_handle_untyped(path.clone()).is_some();
            runtime
                .source_handles
                .push(server.load::<Gltf>(path.clone()).untyped());
            if existing {
                server.reload(path.clone());
            }
        } else {
            server.reload(path);
        }
    }
    pending.cursor = end;
    if end != pending.value.reload_paths.len() {
        return Ok(());
    }
    if let Some(placements) = pending.value.placements.as_mut() {
        let Some(old) = placements.apply(world, &mut runtime.previews)? else { return Ok(()); };
        runtime.retired = Some(Box::new((runtime.retired.take(), old)));
        pending.value.placements = None;
    }
    let pending = runtime.pending.take().unwrap();
    let value = pending.value;
    let scene_changed = world.resource::<ActiveDynamicScene>().scene_id.as_ref() != Some(&value.scene_id);
    // 根兼容高度直接替换；所有独立画布沿稳定对象路径由固定预算系统同步到实例资源。
    let old_height = world.remove_resource::<SparseHeightLiveOverride>();
    let old_sources = world.remove_resource::<HeightSources>();
    let old_height_bindings = world.remove_resource::<super::tool_brush::HeightSourceBindings>();
    world.insert_resource(value.heights);
    world.insert_resource(value.height_bindings);
    world.insert_resource(HeightSources {
        scene_id: Some(value.scene_id.clone()),
        ready: value.height_overrides.is_empty(),
        sources: value.height_overrides,
        ..default()
    });
    if scene_changed {
        world.resource_mut::<crate::project::ProjectViewOrigin>().0 = Vec3::from_array(value.spawn);
        world
            .resource_mut::<ActiveDynamicScene>()
            .select(value.scene_id.clone());
        let mut destination = world.resource_mut::<feathrise_portal::PortalDestination>();
        destination.dynamic_scene_id = Some(value.scene_id.0.clone());
        destination.spawn_pos = Some(Vec3::from_array(value.spawn));
        world.insert_resource(
            PendingSceneTransition::named(ScenePhase::Active, &value.name).with_fresh_world(),
        );
        world
            .resource_mut::<NextState<ScenePhase>>()
            .set(ScenePhase::Loading);
    }
    world.resource_mut::<EmbeddedProjectState>().revision = value.revision;
    runtime.retired = Some(Box::new((
        runtime.retired.take(),
        old_height,
        old_sources,
        old_height_bindings,
        value.reload_paths,
    )));
    runtime.applied_frame = runtime.frames;
    runtime.readiness_cursor = 0;
    runtime.sources_ready = runtime.paths.is_empty();
    runtime.sources_settled = runtime.paths.is_empty();
    runtime.source_ready_frame = runtime.frames;
    runtime.transport.send(Message::SnapshotApplied {
        transaction: value.transaction,
        revision: value.revision,
    })?;
    Ok(())
}

fn poll_source_readiness(world: &World, runtime: &mut Runtime) {
    if runtime.sources_settled
        || runtime.frames <= runtime.applied_frame
        || runtime
            .pending
            .as_ref()
            .is_some_and(|pending| pending.published)
    {
        return;
    }
    let server = world.resource::<AssetServer>();
    for _ in 0..128 {
        let Some(path) = runtime.paths.get(runtime.readiness_cursor) else {
            runtime.sources_settled = true;
            runtime.sources_ready = !runtime.source_failed;
            runtime.source_ready_frame = runtime.frames;
            break;
        };
        if let Some(handle) = server.get_handle_untyped(path) {
            if let Some(source) = runtime.source_reads.get(path) {
                if source.failed.load(std::sync::atomic::Ordering::Acquire) {
                    runtime.source_failed = true;
                    runtime.readiness_cursor += 1;
                    continue;
                }
                if !super::source_readiness::adopted(source) {
                    return;
                }
            }
            if !server.is_loaded_with_dependencies(handle.id()) {
                return;
            }
        }
        runtime.readiness_cursor += 1;
    }
}

pub(super) fn scene_ready(world: &World) -> bool {
    let map = &world.resource::<ActiveDynamicScene>().scene_id;
    // 空工程也是明确可试玩的常驻 World；仅初始零修订且完全没有场景/加载意图时放行。
    // 已加载场景变成缺失、加载失败或要求尚未采用的修订，都不能伪装为空工程绕过就绪门。
    if map.is_none() {
        return world.resource::<EmbeddedProjectState>().revision == 0
            && world.resource::<State<ScenePhase>>().get() == &ScenePhase::Idle
            && !world.contains_resource::<LoadingProgress>()
            && !world.contains_resource::<PendingSceneTransition>()
            && !world.contains_resource::<SceneStartupGate>();
    }
    if world.resource::<State<ScenePhase>>().get() != &ScenePhase::Active
        || world.contains_resource::<LoadingProgress>()
        || world
            .get_resource::<SceneStartupGate>()
            .is_some_and(|gate| !gate.ready)
        || !world.resource::<HeightSources>().ready
    {
        return false;
    }
    if world
        .get_resource::<DynamicSceneRuntimeStatus>()
        .is_none_or(|status| status.scene_id != *map || status.phase != DynamicSceneRuntimePhase::Ready)
    {
        return false;
    }
    if world
        .get_resource::<ScenePlacementRuntimeStatus>()
        .is_none_or(|status| {
            status.scene_id != *map || status.phase != ScenePlacementRuntimePhase::Ready
        })
    {
        return false;
    }
    if world
        .get_resource::<DynamicVegetationStatus>()
        .is_some_and(|status| {
            status.scene_id == *map && status.phase != DynamicVegetationPhase::Ready
        })
    {
        return false;
    }
    if world
        .get_resource::<DynamicWaterStatus>()
        .is_some_and(|status| {
            status.scene_id == *map
                && (!matches!(
                    status.phase,
                    DynamicWaterPhase::Ready | DynamicWaterPhase::Disabled
                ) || status.phase == DynamicWaterPhase::Ready
                    && status.applied_generation != status.desired_generation)
        })
    {
        return false;
    }
    if world
        .get_resource::<DynamicSceneBoundaryStatus>()
        .is_some_and(|status| {
            status.scene_id == *map
                && !matches!(
                    status.phase,
                    DynamicSceneBoundaryPhase::Ready | DynamicSceneBoundaryPhase::Disabled
                )
        })
    {
        return false;
    }
    true
}

fn apply_mode(world: &mut World, runtime: &mut Runtime) -> Result<(), String> {
    let Some(request) = runtime.pending_mode.as_ref() else {
        return Ok(());
    };
    if !request.native_started && request.started.elapsed() > Duration::from_secs(60) {
        let request = runtime.pending_mode.take().unwrap();
        runtime.transport.send(Message::Failure {
            request_id: request.request_id,
            code: "mode-not-ready".into(),
            message: "本次作者来源尚未通过场景、资源和碰撞就绪门，保留编辑状态".into(),
        })?;
        return Ok(());
    }
    let revision = world.resource::<EmbeddedProjectState>().revision;
    if !request.native_started && request.mode == Mode::Playing && runtime.sources_settled && !runtime.sources_ready {
        let request = runtime.pending_mode.take().unwrap();
        runtime.transport.send(Message::Failure {
            request_id: request.request_id,
            code: "source-load-failed".into(),
            message: "本次作者来源的生产资产加载失败；保留编辑状态，修复草稿后可再次同步".into(),
        })?;
        return Ok(());
    }
    if !request.native_started && request.mode == Mode::Playing
        && (revision < request.revision
            || runtime.pending.is_some()
            || !runtime.sources_ready
            || runtime.frames <= runtime.source_ready_frame + 3
            || !scene_ready(world))
    {
        return Ok(());
    }
    if !request.native_started {
        let (request_id, mode) = (request.request_id, request.mode);
        if let Err(message) = super::window::begin_handoff(world, runtime, request_id, mode) {
            runtime.pending_mode = None;
            runtime.transport.send(Message::Failure { request_id, code: "native-window".into(), message })?;
        } else {
            runtime.pending_mode.as_mut().unwrap().native_started = true;
        }
        return Ok(());
    }
    let Some((request_id, outcome)) = runtime.native_result.take() else { return Ok(()); };
    if request_id != request.request_id {
        return Err("原生窗口结果不属于当前模式请求".into());
    }
    if let Err(message) = outcome {
        runtime.pending_mode = None;
        runtime.requested_return = false;
        runtime.window_return_requested = false;
        // 只有旧承载已实际恢复才收到失败结果；保留旧模式与原World，交还其输入策略。
        let mode = world.resource::<EmbeddedProjectState>().mode;
        world.resource_mut::<EmbeddedProjectState>().transitioning = false;
        reset_mode_input(world, mode);
        super::window::restore_mode_cursor(world, runtime, mode);
        runtime.transport.send(Message::Failure { request_id, code: "native-window".into(), message })?;
        return Ok(());
    }
    let request = runtime.pending_mode.take().unwrap();
    if request.mode == Mode::Playing && runtime.camera_mode == Mode::Editing {
        // 先恢复真实游戏相机，再开放同一Update中的ProjectGameplay；否则玩法会读到作者姿态，
        // 而PostUpdate的迟到恢复还会覆盖控制器本帧刚计算的结果。
        restore_game_camera(world, runtime.game_camera.as_ref());
        super::author_view::release(world, runtime);
        runtime.camera_mode = Mode::Playing;
    }
    // 预览只临时拥有两个显示开关；进入前保存实际状态，返回后交回原作者显示规则。
    super::preview_toolbar::mode_changed(world, request.mode);
    let world_id = {
        let mut state = world.resource_mut::<EmbeddedProjectState>();
        state.mode = request.mode;
        state.transitioning = false;
        state.world_id
    };
    runtime.requested_return = false;
    if request.mode == Mode::Editing {
        // 同一次返回期间的重复X合并到已完成事务；进入Playing时的新关闭请求仍须保留。
        runtime.window_return_requested = false;
    }
    reset_mode_input(world, request.mode);
    super::window::restore_mode_cursor(world, runtime, request.mode);
    runtime.transport.send(Message::ModeApplied {
        request_id: request.request_id,
        mode: request.mode,
        revision,
        world_id,
    })
}

#[cfg(test)]
mod native_result_tests {
    use super::*;

    #[test]
    fn editing_completion_settles_repeated_close_but_playing_keeps_a_new_close() {
        for mode in [Mode::Editing, Mode::Playing] {
            let (mut runtime, _, output) = super::super::transport::test_runtime();
            runtime.pending_mode = Some(super::super::resources::PendingMode {
                request_id: 7, mode, revision: 1, started: std::time::Instant::now(), native_started: true,
            });
            runtime.native_result = Some((7, Ok(())));
            runtime.requested_return = true;
            runtime.window_return_requested = true;
            runtime.camera_mode = Mode::Playing;
            let mut world = World::new();
            world.insert_resource(EmbeddedProjectState {
                mode: Mode::Playing, world_id: 1, revision: 1, transitioning: true,
            });
            apply_mode(&mut world, &mut runtime).unwrap();
            assert!(!runtime.requested_return);
            assert_eq!(runtime.window_return_requested, mode == Mode::Playing);
            assert!(runtime.pending_mode.is_none());
            assert!(matches!(output.try_recv().unwrap().message,
                Message::ModeApplied { request_id: 7, mode: applied, .. } if applied == mode));
            apply_mode(&mut world, &mut runtime).unwrap();
            assert!(output.try_recv().is_err(), "已消费的原生结果不能重复应答");
        }
    }
}

pub(super) fn reset_mode_input(world: &mut World, mode: Mode) {
    super::input_handoff::clear(world);
    if mode == Mode::Editing {
        use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
        // 角色控制器可能锁住真实游戏子窗口；返回作者前释放锁定，不能只清按键状态。
        for mut cursor in world
            .query_filtered::<&mut CursorOptions, With<PrimaryWindow>>()
            .iter_mut(world)
        {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
    }
}

pub(super) fn restore_game_camera(
    world: &mut World,
    saved: Option<&(Entity, Transform, Projection)>,
) {
    let Some((entity, transform, projection)) = saved else {
        return;
    };
    let mut cameras = world.query_filtered::<
        (&mut Transform, &mut Projection),
        With<feathrise_scene_core::view::PrimarySceneView>,
    >();
    if let Ok((mut current_transform, mut current_projection)) = cameras.get_mut(world, *entity) {
        *current_transform = *transform;
        *current_projection = projection.clone();
    } // 切换场景后旧相机已退出时不把它的姿态套给新相机。
}

pub(super) fn camera(
    mut runtime: NonSendMut<Runtime>,
    state: Res<EmbeddedProjectState>,
    mut cameras: Query<
        (Entity, &mut Transform, &mut Projection),
        With<feathrise_scene_core::view::PrimarySceneView>,
    >,
) {
    if runtime.stopping { return; }
    if state.mode == Mode::Playing {
        return; // 玩法已在开门前取得相机；其后每帧的姿态和投影完全由工程系统控制。
    }
    let Ok((entity, mut transform, mut projection)) = cameras.single_mut() else {
        return;
    };
    if state.mode == Mode::Editing {
        if runtime.camera_mode == Mode::Playing
            || runtime
                .game_camera
                .as_ref()
                .is_none_or(|(old, _, _)| *old != entity)
        {
            runtime.game_camera = Some((entity, *transform, projection.clone()));
        }
        if let Some(camera) = runtime.camera {
            *transform = Transform::from_translation(Vec3::from_array(camera.position))
                .with_rotation(Quat::from_array(camera.rotation).normalize());
            if let Some(height) = camera.orthographic_height {
                *projection = Projection::Orthographic(OrthographicProjection {
                    scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                        viewport_height: height,
                    },
                    ..OrthographicProjection::default_3d()
                });
            } else {
                *projection = Projection::Perspective(PerspectiveProjection {
                    fov: camera.vertical_fov_radians,
                    ..default()
                });
            }
        }
    }
    runtime.camera_mode = state.mode;
}

/// 该计数器确实只在项目玩法集合运行；不会用每帧 Update 数冒充暂停/恢复证明。
pub(super) fn gameplay_tick(mut runtime: NonSendMut<Runtime>) {
    runtime.ticks = runtime.ticks.saturating_add(1);
}

pub(super) fn heights(
    mut sources: ResMut<HeightSources>,
    instances: Query<&SubsceneInstance>,
    mut runtime: NonSendMut<Runtime>,
    mut table: ResMut<InstanceResourceTable<SparseHeightLiveOverride>>,
    mut root: ResMut<SparseHeightLiveOverride>,
    target: Option<Res<feathrise_scene::TerrainObjectAuthoringTarget>>,
) {
    if runtime.stopping { return; }
    if runtime.retired.is_some() {
        return;
    } // 旧高度最后一个持有者必须在有界后台释放，不能落入帧析构。
    if sources.sources.is_empty() {
        sources.ready = true;
        return;
    }
    let keys: Vec<_> = match sources.cursor {
        Some(cursor) => table
            .values
            .range((Excluded(cursor), Unbounded))
            .take(128)
            .map(|(entity, _)| *entity)
            .collect(),
        None => table.values.keys().take(128).copied().collect(),
    };
    if keys.is_empty() {
        sources.ready = sources.seen_count == sources.sources.len();
        sources.cursor = None;
        // 以扫描代号替代每轮清空/遍历整棵地形集合，收尾保持固定工作量。
        sources.scan = sources.scan.wrapping_add(1);
        sources.seen_count = 0;
        return;
    }
    let mut retired = Vec::new();
    for entity in keys {
        sources.cursor = Some(entity);
        let Ok(instance) = instances.get(entity) else {
            continue;
        };
        if instance.path.root_scene_id != *sources.scene_id.as_ref().unwrap()
            || !instance.path.instances.is_empty()
        {
            continue;
        }
        let Some(id) = &instance.path.terrain_id else {
            continue;
        };
        let Some(source) = sources.sources.get(id) else {
            continue;
        };
        // 原作者系统把当前对象放在root alias；写回实例表会更新停放槽而使真正GPU仍读取旧高度。
        let current = if target.as_ref().is_some_and(|target| target.instance() == Some(entity)
            && target.applied_to(sources.scene_id.as_ref().unwrap(), id)) { &mut *root }
            else { table.values.get_mut(&entity).unwrap() };
        if current.scene_id() != source.scene_id() || current.revision() != source.revision() {
            retired.push(std::mem::replace(current, source.clone()));
        }
        let scan = sources.scan;
        if sources.seen.insert(id.clone(), scan) != Some(scan) {
            sources.seen_count += 1;
        }
    }
    if !retired.is_empty() {
        runtime.retired = Some(Box::new(retired));
    }
}

/// 实时雕刻只复制持久树根，叶模块在后台应用页差量；完整快照和实时页共享同一实例来源表。
pub(super) fn height_preview_source(
    world: &World,
    map: &SceneId,
    terrain: Option<&TerrainId>,
) -> Option<SparseHeightLiveOverride> {
    if world.get_resource::<ActiveDynamicScene>()?.scene_id.as_ref() != Some(map) { return None; }
    match terrain {
        Some(id) => {
            let sources = world.get_resource::<HeightSources>()?;
            (sources.scene_id.as_ref() == Some(map)).then(|| sources.sources.get(id).cloned()).flatten()
        }
        None => world.get_resource::<SparseHeightLiveOverride>().cloned(),
    }
}

/// 仅发布已在后台完成的持久根；返回旧根交给叶模块回收槽，禁止在帧内析构整片高度页。
pub(super) fn publish_height_preview(
    world: &mut World,
    map: &SceneId,
    terrain: Option<TerrainId>,
    source: SparseHeightLiveOverride,
) -> Result<Option<SparseHeightLiveOverride>, String> {
    if world.get_resource::<ActiveDynamicScene>().and_then(|active|active.scene_id.as_ref()) != Some(map)
        || source.scene_id().is_some_and(|owner|owner != map) {
        return Err("实时高度发布的场景身份已改变".into());
    }
    if let Some(id) = terrain {
        let mut sources = world.get_resource_mut::<HeightSources>().ok_or("实时高度缺少实例来源表")?;
        if sources.scene_id.as_ref() != Some(map) { return Err("实时高度实例来源尚未绑定当前场景".into()); }
        if sources.sources.get(&id).is_some_and(|old|source.revision() <= old.revision()) {
            return Err("实时高度实例发布序号已经过期".into());
        }
        let previous = sources.sources.insert(id, source);
        sources.cursor = None;
        sources.ready = false;
        sources.scan = sources.scan.wrapping_add(1);
        sources.seen_count = 0;
        Ok(previous)
    } else {
        if world.get_resource::<SparseHeightLiveOverride>().is_some_and(|old|source.revision() <= old.revision()) {
            return Err("实时高度根发布序号已经过期".into());
        }
        let previous = world.remove_resource::<SparseHeightLiveOverride>();
        world.insert_resource(source);
        Ok(previous)
    }
}
