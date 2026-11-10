//! 采用事务的主App门：只做有界通道轮询和真实结果通知，文件读取与装载由唯一后台槽承担。

use super::{package::RootLease, resources::{FunctionHotpatchState, active::Runtime},
    types::{Outcome, Request}, worker};
use crate::project::embedded::{EmbeddedProjectState, resources::Runtime as EmbeddedRuntime, transport::Configuration};
use bevy::{ecs::{HotPatchChanges, HotPatched}, prelude::*};
use feathrise_editor_protocol::Message;
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}, mpsc}};

pub(in crate::project::embedded) fn prepare(app: &mut App, config: &Configuration, base_build: &str) -> Result<bool, String> {
    let Some(directory) = std::env::var_os("FEATHRISE_CODE_PATCH_ROOT").map(PathBuf::from) else {
        return Ok(false);
    };
    // 初始化发生在App运行之前；普通帧绝不打开目录、散列DLL或等待工作线程。
    let root = RootLease::open(&directory)?;
    let aslr_reference = subsecond::aslr_reference() as u64;
    if aslr_reference == 0 { return Err("当前EXE未导出热补丁main入口，不能协商函数体热更新".into()); }
    let (input_tx, input_rx) = mpsc::sync_channel(1);
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let generation = Arc::new(AtomicU64::new(config.identity.generation));
    let worker_stop = stop.clone(); let worker_generation = generation.clone();
    let worker = std::thread::Builder::new().name("feathrise-function-hotpatch".into())
        .spawn(move || worker::run(root, input_rx, output_tx, worker_stop, worker_generation))
        .map_err(|error| format!("代码补丁后台槽启动失败：{error}"))?;
    app.init_resource::<FunctionHotpatchState>().insert_non_send(Runtime {
        sender: input_tx, receiver: output_rx, stop, scene_generation: generation,
        worker: Some(worker), in_flight: None, reply: None, ready_sent: false,
        last_patch_id: 0, last_source_generation: 0, epoch: 0, base_build: base_build.into(),
        aslr_reference,
    });
    Ok(true)
}

pub(in crate::project::embedded) fn busy(world: &World) -> bool {
    world.get_non_send::<Runtime>().is_some_and(|runtime| runtime.in_flight.is_some() || runtime.reply.is_some())
}

pub(in crate::project::embedded) fn receive(world: &mut World, embedded: &mut EmbeddedRuntime, patch_id: u64,
    source_generation: u64, base_build: String, artifact: String, sha256: String) {
    let state = *world.resource::<EmbeddedProjectState>();
    let request = Request { patch_id, source_generation, base_build, artifact, sha256,
        project_id: embedded.config.identity.project_id.clone(), session: embedded.config.identity.session,
        scene_generation: embedded.transport.identity.generation, pid: std::process::id(), world_id: state.world_id,
        aslr_reference: subsecond::aslr_reference() as u64 };
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else {
        let outcome = Outcome { request, result: Err("当前运行实例未协商函数体热更新能力".into()) };
        let _ = embedded.transport.send(outcome.message(0));
        return;
    };
    let occupied = runtime.in_flight.is_some() || runtime.reply.is_some();
    let rejection = if !embedded.ready || !embedded.hello || embedded.failed || runtime.stop.load(Ordering::Acquire) {
        Some("工程控制会话尚未就绪")
    } else if occupied {
        Some("前一个函数体补丁事务尚未结束")
    } else if embedded.pending.is_some() || embedded.pending_mode.is_some() || embedded.retired.is_some() || !embedded.sources_settled {
        Some("场景或模式正在交接，请在交接完成后应用")
    } else if request.base_build != runtime.base_build {
        Some("函数体补丁不属于当前冻结基线")
    } else if request.patch_id <= runtime.last_patch_id || request.source_generation < runtime.last_source_generation {
        Some("函数体补丁事务重复或源码代次已经过期")
    } else if runtime.epoch == u64::MAX {
        Some("函数体补丁采用代次已耗尽，请重新启动预览")
    } else { None };
    if let Some(reason) = rejection {
        let outcome = Outcome { request, result: Err(reason.into()) };
        if !occupied {
            // 拒绝同样是事务结果；管道暂满时由First可靠重试，不能让宿主永远停在应用中。
            runtime.reply = Some((outcome, runtime.epoch));
        } else {
            // 正常drive的busy门会延后这类请求；额外请求不能覆盖原事务唯一的在途／待回执槽。
            let _ = embedded.transport.send(outcome.message(runtime.epoch));
        }
    } else {
        runtime.scene_generation.store(request.scene_generation, Ordering::Release);
        match runtime.sender.try_send(request.clone()) {
            Ok(()) => {
                runtime.last_patch_id = request.patch_id;
                runtime.last_source_generation = request.source_generation;
                runtime.in_flight = Some(request);
            }
            Err(error) => {
                let outcome = Outcome { request, result: Err(format!("函数体补丁后台槽不可用：{error}")) };
                runtime.reply = Some((outcome, runtime.epoch));
            }
        }
    }
    world.insert_non_send(runtime);
}

pub(super) fn advance(world: &mut World) {
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else { return; };
    let Some(mut embedded) = world.remove_non_send::<EmbeddedRuntime>() else {
        world.insert_non_send(runtime); return;
    };
    runtime.scene_generation.store(embedded.transport.identity.generation, Ordering::Release);
    if embedded.failed { runtime.stop.store(true, Ordering::Release); }
    let state = *world.resource::<EmbeddedProjectState>();
    if embedded.ready && !embedded.failed && !runtime.ready_sent {
        if embedded.transport.send(Message::HotPatchReady { pid: std::process::id(), world_id: state.world_id,
            aslr_reference: runtime.aslr_reference, base_build: runtime.base_build.clone() }).is_ok() {
            runtime.ready_sent = true;
        }
    }
    if runtime.reply.is_none() {
        let outcome = match runtime.receiver.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(mpsc::TryRecvError::Disconnected) => runtime.in_flight.as_ref().map(|request|
                Outcome { request: request.clone(), result: Err("函数体补丁后台槽已停止；原预览现场保留".into()) }),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        if let Some(outcome) = outcome {
            // 工作者已经完成实际跳表提交；不得在仅收到请求或开始装载时增加epoch。
            let expected = runtime.in_flight.take();
            if expected.as_ref().is_some_and(|expected| expected.patch_id == outcome.request.patch_id
                && expected.source_generation == outcome.request.source_generation) {
                if outcome.result.is_ok() {
                    runtime.epoch += 1;
                    world.resource_mut::<FunctionHotpatchState>().epoch = runtime.epoch;
                    world.resource_mut::<HotPatchChanges>().set_changed();
                    world.write_message(HotPatched);
                }
                runtime.reply = Some((outcome, runtime.epoch));
            }
        }
    }
    if let Some((outcome, epoch)) = runtime.reply.take() {
        if embedded.transport.send(outcome.message(epoch)).is_err() {
            // 有界输出暂满时保留唯一结果，不由普通遥测覆盖真实采用回执。
            runtime.reply = Some((outcome, epoch));
        }
    }
    world.insert_non_send(embedded); world.insert_non_send(runtime);
}
