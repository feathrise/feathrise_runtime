//! 专用匿名管道的有界后台收发；日志保持 stdout/stderr，控制帧绝不混入游戏日志。

use super::snapshot::{PreparedSnapshot, prepare};
use feathrise_editor_protocol::{
    Disposition, Envelope, Limits, Message, SessionGuard, SessionIdentity,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

pub(super) const CAPABILITIES: [&str; 11] = [
    "resident-world-v1",
    "scene-snapshot-v1",
    "embedded-viewport-v1",
    "editor-overlay-v1",
    "author-tools-v1",
    "editor-ui-occlusion-v1",
    "editor-ui-shapes-v1",
    "author-preview-v1",
    "author-view-v1",
    "preview-toolbar-v1",
    "native-preview-window-v1",
];

pub(crate) struct Configuration {
    pub identity: SessionIdentity,
    pub parent: usize,
    pub parent_pid: u32,
    read: usize,
    write: usize,
}

impl Configuration {
    pub(crate) fn from_args(project: &crate::project::ProjectContext) -> Result<Option<Self>, String> {
        let args: Vec<_> = std::env::args().collect();
        if !args.iter().any(|arg| arg == "--feathrise-embedded") {
            return Ok(None);
        }
        let number = |key: &str| -> Result<u64, String> {
            let positions: Vec<_> = args
                .iter()
                .enumerate()
                .filter(|(_, value)| value.as_str() == key)
                .collect();
            if positions.len() != 1 {
                return Err(format!("嵌入启动参数 {key} 缺少或重复"));
            }
            args.get(positions[0].0 + 1)
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| format!("嵌入启动参数 {key} 无效"))
        };
        let config = Self {
            identity: SessionIdentity {
                project_id: project.id.clone(),
                session: number("--session")?,
                generation: number("--generation")?,
            },
            parent: usize::try_from(number("--parent-window")?).map_err(|e| e.to_string())?,
            parent_pid: u32::try_from(number("--parent-pid")?).map_err(|e| e.to_string())?,
            read: usize::try_from(number("--ipc-read")?).map_err(|e| e.to_string())?,
            write: usize::try_from(number("--ipc-write")?).map_err(|e| e.to_string())?,
        };
        if config.read == config.write || config.read == usize::MAX || config.write == usize::MAX {
            return Err("控制管道需要两个有效且不同的继承句柄".into());
        }
        Ok(Some(config))
    }
}

pub(super) enum Inbound {
    Control(Message),
    Snapshot(PreparedSnapshot),
    Preview(super::preview::PreparedPreview),
    Rejected {
        transaction: u64,
        generation: u64,
        reason: String,
    },
    Disconnected(String),
}

pub(super) struct Transport {
    pub identity: SessionIdentity,
    pub build_identity: String,
    pub incoming: Receiver<Inbound>,
    pub outgoing: SyncSender<Envelope>,
    pub retire: SyncSender<Box<dyn Send>>,
    pub sequence: u64,
}

impl Transport {
    pub(super) fn send(&mut self, message: Message) -> Result<(), String> {
        self.sequence = self.sequence.checked_add(1).ok_or("控制顺序号已耗尽")?;
        self.outgoing
            .try_send(Envelope::new(self.identity.clone(), self.sequence, message))
            .map_err(|error| format!("控制回执通道繁忙或断开：{error}"))
    }
}

/// 纯控制流回归只借有界内存邮箱，不创建管道、窗口或伪造原生验收成功。
#[cfg(test)]
pub(super) fn test_runtime() -> (super::resources::Runtime, SyncSender<Inbound>, Receiver<Envelope>) {
    let identity = SessionIdentity { project_id: "window-control-test".into(), session: 1, generation: 1 };
    let config = Configuration { identity: identity.clone(), parent: 0, parent_pid: 0, read: 0, write: 0 };
    let (input, incoming) = mpsc::sync_channel(8);
    let (outgoing, output) = mpsc::sync_channel(16);
    let (retire, _) = mpsc::sync_channel(1);
    let transport = Transport { identity, build_identity: "test".into(), incoming, outgoing, retire, sequence: 0 };
    let mut runtime = super::resources::Runtime::new(config, transport);
    runtime.native_mode = feathrise_editor_protocol::Mode::Playing;
    runtime.input_suspended = false;
    (runtime, input, output)
}

pub(super) fn start(
    config: &Configuration,
    project: &crate::project::ProjectContext,
) -> Result<Transport, String> {
    let build_identity = executable_hash()?; // App 事件循环启动前计算；作者宿主始终保持响应。
    let (mut read, mut write) = take_channels(config.read, config.write)?;
    let (input_tx, input_rx) = mpsc::sync_channel(2);
    let (output_tx, output_rx) = mpsc::sync_channel::<Envelope>(16);
    let (retire_tx, retire_rx) = mpsc::sync_channel::<Box<dyn Send>>(2);
    let project = project.clone();
    let identity = config.identity.clone();
    let expected = build_identity.clone();
    thread::Builder::new()
        .name("feathrise-guest-read".into())
        .spawn(move || {
            let mut guard = SessionGuard::new(identity);
            let mut previous = std::sync::Arc::new(super::asset_overlay::Files::new());
            let mut accepted_revision = 0;
            let mut selected_map: Option<String> = None;
            let result = (|| -> Result<(), String> {
                let hello =
                    feathrise_editor_protocol::read_frame(&mut read, &Limits::default())
                        .map_err(|e| e.to_string())?
                        .ok_or("握手前控制管道关闭")?;
                guard.accept(&hello).map_err(|e| e.to_string())?;
                SessionGuard::verify_handshake(&hello, &expected, &CAPABILITIES)
                    .map_err(|e| e.to_string())?;
                input_tx
                    .send(Inbound::Control(hello.message))
                    .map_err(|e| e.to_string())?;
                while let Some(envelope) =
                    feathrise_editor_protocol::read_frame(&mut read, &Limits::default())
                        .map_err(|e| e.to_string())?
                {
                    if let Message::Snapshot { map_id, .. } = &envelope.message {
                        if envelope.identity.generation == guard.identity().generation {
                            if selected_map
                                .as_ref()
                                .is_some_and(|selected| selected != map_id)
                            {
                                return Err("同一地图代次不能替换地图身份".into());
                            }
                        } else if envelope.identity.generation
                            == guard.identity().generation.saturating_add(1)
                        {
                            if selected_map
                                .as_ref()
                                .is_none_or(|selected| selected == map_id)
                            {
                                return Err("下一代次必须由实际不同地图的完整快照发起".into());
                            }
                            guard
                                .advance_generation(envelope.identity.generation)
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    if guard.accept(&envelope).map_err(|e| e.to_string())?
                        == Disposition::StaleGeneration
                    {
                        continue;
                    }
                    let generation = envelope.identity.generation;
                    let input = match envelope.message {
                        Message::Preview { kind, sequence, map_id, format } => {
                            if selected_map.as_deref() != Some(&map_id) {
                                return Err("实时预览必须属于已选择地图".into());
                            }
                            Inbound::Preview(super::preview::prepare(kind, sequence, generation, &map_id,
                                &format, envelope.payload.as_deref().ok_or("实时预览缺少正文")?)?)
                        }
                        Message::Snapshot {
                            transaction,
                            revision,
                            map_id,
                            format,
                        } => {
                            selected_map = Some(map_id.clone());
                            if revision <= accepted_revision {
                                Inbound::Rejected {
                                    transaction,
                                    generation,
                                    reason: "作者快照修订倒退或重复".into(),
                                }
                            } else if format != super::SCENE_SNAPSHOT_FORMAT {
                                Inbound::Rejected {
                                    transaction,
                                    generation,
                                    reason: "作者快照格式不兼容".into(),
                                }
                            } else {
                                match prepare(
                                    envelope.payload.as_deref().ok_or("作者快照缺少正文")?,
                                    &map_id,
                                    transaction,
                                    revision,
                                    &project,
                                    &previous,
                                    generation,
                                ) {
                                    Ok(value) => {
                                        previous = value.files.clone();
                                        accepted_revision = revision;
                                        Inbound::Snapshot(value)
                                    }
                                    Err(reason) => Inbound::Rejected {
                                        transaction,
                                        generation,
                                        reason,
                                    },
                                }
                            }
                        }
                        message => Inbound::Control(message),
                    };
                    input_tx.send(input).map_err(|e| e.to_string())?; // 后台有界阻塞形成背压，主帧只 try_recv。
                }
                Err("作者控制管道已经关闭".into())
            })();
            if let Err(error) = result {
                let _ = input_tx.send(Inbound::Disconnected(error));
            }
        })
        .map_err(|e| e.to_string())?;
    thread::Builder::new()
        .name("feathrise-guest-write".into())
        .spawn(move || {
            while let Ok(envelope) = output_rx.recv() {
                if feathrise_editor_protocol::write_frame(
                    &mut write,
                    &envelope,
                    &Limits::default(),
                )
                .is_err()
                {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    thread::Builder::new()
        .name("feathrise-guest-retire".into())
        .spawn(move || {
            while let Ok(value) = retire_rx.recv() {
                drop(value);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Transport {
        identity: config.identity.clone(),
        build_identity,
        incoming: input_rx,
        outgoing: output_tx,
        retire: retire_tx,
        sequence: 0,
    })
}

pub(crate) fn executable_hash() -> Result<String, String> {
    let mut file = File::open(std::env::current_exe().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(windows)]
pub(crate) fn take_channels(read: usize, write: usize) -> Result<(File, File), String> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation,
    };
    // 只有完整嵌入 CLI 才进入本函数；两句柄来自父进程显式继承列表，接管后禁止传给工程后代。
    unsafe {
        let mut flags = 0;
        if GetHandleInformation(read as *mut _, &mut flags) == 0
            || GetHandleInformation(write as *mut _, &mut flags) == 0
        {
            return Err("控制管道句柄没有被当前进程继承".into());
        }
        let input = File::from_raw_handle(read as *mut _);
        let output = File::from_raw_handle(write as *mut _);
        if SetHandleInformation(read as *mut _, HANDLE_FLAG_INHERIT, 0) == 0
            || SetHandleInformation(write as *mut _, HANDLE_FLAG_INHERIT, 0) == 0
        {
            return Err(format!(
                "无法接管编辑器控制管道：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok((input, output))
    }
}

#[cfg(not(windows))]
pub(crate) fn take_channels(_: usize, _: usize) -> Result<(File, File), String> {
    Err("当前嵌入视口仅支持 Windows".into())
}
