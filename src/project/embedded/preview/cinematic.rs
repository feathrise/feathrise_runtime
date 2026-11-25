//! 独立工程视口的演出草稿通道。文档在 IPC 线程解析，一次传送后可连续使用小型拖帧请求。
use super::super::resources::Runtime;
use crate::app::game_plugins::cinematics::CinematicPresentation;
use bevy::prelude::*;
use feathrise_cinematic::{Cinematic, CinematicFrame, FrameData, FrameMode};
use feathrise_editor_protocol::{Message, PreviewKind};
use std::sync::Arc;

pub(crate) const CAPABILITY: &str = "cinematic-preview-v1";
pub(super) enum Command {
    Document(Arc<Cinematic>),
    Seek(String, f32),
    Stop,
    Invalid(String),
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Seek {
    id: String,
    time: f32,
    playing: bool,
}

pub(super) fn decode(format: &str, bytes: &[u8]) -> Option<Command> {
    let parsed = match format {
        "cinematic-document-v1" => {
            Cinematic::from_slice(bytes).map(|doc| Command::Document(Arc::new(doc)))
        }
        "cinematic-seek-v1" => serde_json::from_slice::<Seek>(bytes)
            .map_err(|e| e.to_string())
            .and_then(|seek| {
                let _playing = seek.playing; // 游标始终由作者时钟推进，在项目进程中再次按墙钟播放会产生漂移。
                if !seek.time.is_finite() || seek.time < 0. {
                    Err("演出游标必须是非负有限秒数".into())
                } else {
                    Ok(Command::Seek(seek.id, seek.time))
                }
            }),
        "cinematic-stop-v1" => Ok(Command::Stop),
        _ => return None,
    };
    Some(parsed.unwrap_or_else(Command::Invalid))
}

#[derive(Default)]
pub(super) struct State {
    doc: Option<Arc<Cinematic>>,
    last: u64,
    ticket: Option<u64>,
    pending: Option<(u64, Option<f32>)>,
}

fn reply(runtime: &mut Runtime, sequence: u64, error: Option<String>) -> Result<(), String> {
    runtime.transport.send(match error {
        Some(message) => Message::Failure {
            request_id: sequence,
            code: "cinematic-preview".into(),
            message,
        },
        None => Message::PreviewApplied {
            kind: PreviewKind::AuthorDomain,
            sequence,
        },
    })
}

impl State {
    fn clear_frame(&mut self, world: &mut World) {
        if let Some(mut frame) = world.get_resource_mut::<CinematicFrame>()
            && frame
                .active
                .as_ref()
                .is_some_and(|f| f.mode == FrameMode::Preview && Some(f.ticket) == self.ticket)
        {
            frame.active = None;
        }
        self.ticket = None;
    }
}

pub(super) fn reset(world: &mut World, runtime: &mut Runtime) -> Result<(), String> {
    let state = &mut runtime.previews.cinematic;
    let pending = state.pending.take();
    state.clear_frame(world);
    *state = State::default();
    if let Some((seq, _)) = pending {
        reply(runtime, seq, Some("演出预览所属的快照已被替换".into()))?;
    }
    Ok(())
}

pub(super) fn receive(
    world: &mut World,
    runtime: &mut Runtime,
    map: &str,
    seq: u64,
    command: Command,
    valid: bool,
) -> Result<(), String> {
    let state = &mut runtime.previews.cinematic;
    if !valid || seq >> 63 == 0 || seq <= state.last || state.pending.is_some() {
        return reply(
            runtime,
            seq,
            Some("演出预览的场景、修订或序号已过期".into()),
        );
    }
    state.last = seq;
    match command {
        Command::Invalid(e) => reply(runtime, seq, Some(e)),
        Command::Document(doc) => {
            state.clear_frame(world);
            state.ticket = Some(seq);
            state.doc = Some(doc);
            reply(runtime, seq, None)
        }
        Command::Seek(id, time) => {
            let Some(doc) = state
                .doc
                .as_ref()
                .filter(|doc| doc.id == id && time <= doc.duration)
            else {
                return reply(
                    runtime,
                    seq,
                    Some("演出 ID 或指定游标与当前文档不一致".into()),
                );
            };
            let Some(mut frame) = world.get_resource_mut::<CinematicFrame>() else {
                return reply(runtime, seq, Some("工程未安装演出呈现".into()));
            };
            if frame
                .active
                .as_ref()
                .is_some_and(|f| f.mode != FrameMode::Preview)
            {
                return reply(runtime, seq, Some("当前游戏会话正在进行另一场演出".into()));
            }
            let ticket = *state.ticket.get_or_insert(seq);
            frame.active = Some(FrameData {
                ticket,
                time,
                mode: FrameMode::Preview,
                instance: Some(map.into()),
                document: doc.clone(),
            });
            state.pending = Some((seq, Some(time)));
            Ok(())
        }
        Command::Stop => {
            state.clear_frame(world);
            state.pending = Some((seq, None));
            Ok(())
        }
    }
}

pub(super) fn advance(world: &mut World, runtime: &mut Runtime) -> Result<(), String> {
    let state = &mut runtime.previews.cinematic;
    let Some((seq, time)) = state.pending else {
        return Ok(());
    };
    let Some(receipt) = world.get_resource::<CinematicPresentation>() else {
        return Ok(());
    };
    let error = if let Some(time) = time {
        let ticket = state.ticket.unwrap();
        if let Some(error) = receipt.error(ticket, time) {
            Some(error.to_owned())
        } else if receipt.ready(ticket, time) {
            None
        } else {
            return Ok(());
        }
    } else if receipt.camera.is_some() {
        return Ok(());
    } else {
        None
    };
    state.pending = None;
    if error.is_some() {
        state.clear_frame(world);
    }
    reply(runtime, seq, error)
}
