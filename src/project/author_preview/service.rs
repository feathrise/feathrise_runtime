//! 工程自有预览进程：后台接收完整草稿，领域插件生成真实场景，唯一离屏相机异步回传GPU像素。
use super::{
    wire::{self, Packet},
    *,
};
use bevy::{
    app::ScheduleRunnerPlugin,
    camera::{RenderTarget, visibility::RenderLayers},
    render::{
        render_resource::{TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    sync::mpsc::{self, SyncSender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(crate) fn requested() -> bool {
    std::env::args().any(|a| a == "--feathrise-author-preview")
}
#[derive(Debug)]
enum Output {
    Control(Packet),
    Capture(Packet, Image),
}
struct Shutdown {
    reply: Option<Packet>,
    started: Instant,
    flushed: bool,
}
#[derive(Resource)]
struct Peer {
    input: async_channel::Receiver<Packet>,
    output: SyncSender<Output>,
    writer_done: async_channel::Receiver<Result<(), String>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    stopping: Option<Shutdown>,
    project: String,
    session: u64,
    revision: u64,
    kind: String,
    error_sent: Option<(u64, String)>,
    capture_pending: Option<(Entity, Instant)>,
    last_capture: Instant,
    scene: Option<Entity>,
    camera: [f32; 3],
}
#[derive(Resource)]
struct Target(Handle<Image>);
#[derive(Component)]
struct PreviewCamera;

pub(crate) fn run(project: crate::project::ProjectContext, plugin: impl Plugin) -> Result<(), String> {
    std::env::set_current_dir(&project.root).map_err(|e| e.to_string())?;
    let args: Vec<_> = std::env::args().collect();
    let number = |key: &str| -> Result<u64, String> {
        let mut positions = args
            .iter()
            .enumerate()
            .filter(|(_, value)| value.as_str() == key);
        let value = positions
            .next()
            .and_then(|(index, _)| args.get(index + 1))
            .and_then(|value| value.parse().ok())
            .filter(|v: &u64| *v > 0)
            .ok_or_else(|| format!("作者预览参数{key}无效"))?;
        if positions.next().is_some() {
            return Err(format!("作者预览参数{key}重复"));
        }
        Ok(value)
    };
    let session = number("--preview-session")?;
    let input_handle = usize::try_from(number("--ipc-read")?).map_err(|e| e.to_string())?;
    let output_handle = usize::try_from(number("--ipc-write")?).map_err(|e| e.to_string())?;
    if input_handle == output_handle || input_handle == usize::MAX || output_handle == usize::MAX {
        return Err("作者预览必须使用两个不同且有效的继承管道句柄".into());
    }
    let (mut read, mut write) = crate::project::embedded::take_author_channels(input_handle, output_handle)?;
    let (hello, _) = wire::read(&mut read)?.ok_or("作者预览握手前断开")?;
    let actual = crate::project::embedded::author_executable_hash()?;
    if hello.message != "hello"
        || hello.project != project.id
        || hello.session != session
        || hello.data["executable_sha256"] != actual
    {
        return Err("作者预览工程、会话或实际可执行文件身份不匹配".into());
    }
    let (input_tx, input) = async_channel::bounded(2);
    let (output, output_rx) = mpsc::sync_channel::<Output>(2);
    let (writer_complete, writer_done) = async_channel::bounded(1);
    let reader = std::thread::Builder::new()
        .name("author-preview-read".into())
        .spawn(move || {
            while let Ok(Some((packet, _))) = wire::read(&mut read) {
                if input_tx.send_blocking(packet).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let writer = std::thread::Builder::new()
        .name("author-preview-write".into())
        .spawn(move || {
            while let Ok(value) = output_rx.recv() {
                let (packet, pixels) = match value {
                    Output::Control(p) => (p, Vec::new()),
                    Output::Capture(p, image) => match image.try_into_dynamic() {
                        Ok(image) => (p, image.into_rgba8().into_raw()),
                        Err(_) => (
                            Packet {
                                message: "error".into(),
                                data: serde_json::json!({"message":"GPU截图格式不能转换为RGBA8"}),
                                ..p
                            },
                            Vec::new(),
                        ),
                    },
                };
                if let Err(error) = wire::write(&mut write, &packet, &pixels) {
                    let _ = writer_complete.try_send(Err(error));
                    return;
                }
                if packet.message == "stopped" {
                    // 写入并flush完成后才确认；主线程不能入队后立即退出而截断停止回执。
                    let _ = writer_complete.try_send(Ok(()));
                    return;
                }
            }
            let _ = writer_complete.try_send(Err("作者预览输出队列提前关闭".into()));
        })
        .map_err(|e| e.to_string())?;
    output
        .send(Output::Control(Packet {
            project: project.id.clone(),
            session,
            revision: 0,
            message: "ready".into(),
            kind: String::new(),
            data: serde_json::json!({"executable_sha256":actual}),
        }))
        .map_err(|e| e.to_string())?;
    let display = feathrise_settings::StartupDisplayPreferences::default();
    let policy = feathrise_settings::StartupWindowPolicy {
        force_windowed: true,
    };
    let mut app = App::new();
    #[cfg(feature = "function_hotpatch")]
    app.init_resource::<bevy::ecs::HotPatchChanges>().add_message::<bevy::ecs::HotPatched>();
    crate::app::prepare_vulkan_environment();
    let assets = crate::project::assets::install(&mut app, &project)?;
    let plugins = crate::project::config::default_plugins(policy, display, assets, false)
        .disable::<WinitPlugin>()
        .set(WindowPlugin {
            primary_window: None,
            exit_condition: ExitCondition::DontExit,
            ..default()
        });
    app.insert_resource(feathrise_map_content::RuntimeAssetRoot::new(
        project.asset_root(),
    )?)
    .insert_resource(project.clone())
    .insert_resource(display)
    .insert_resource(policy)
    .insert_resource(crate::project::ProjectViewOrigin(Vec3::ZERO))
    .insert_resource(feathrise_scene_core::state::EditorStartup)
    .insert_resource(Peer {
        input,
        output,
        writer_done,
        reader: Some(reader),
        writer: Some(writer),
        stopping: None,
        project: project.id,
        session,
        revision: 0,
        kind: String::new(),
        error_sent: None,
        capture_pending: None,
        last_capture: Instant::now(),
        scene: None,
        camera: [0.67, 0.25, 6.5],
    })
    .init_resource::<AuthorRuntimePreviewStatus>()
    .init_resource::<AuthorRuntimePreviewEvidence>()
    .add_plugins(plugins)
    .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16)))
    .add_plugins(
        avian3d::PhysicsPlugins::default()
            .build()
            .disable::<avian3d::prelude::ColliderCachePlugin>(),
    )
    .add_plugins(bevy_hanabi::HanabiPlugin)
    .add_plugins(plugin)
    .add_systems(Startup, setup)
    .add_systems(PreUpdate, receive)
    .add_systems(
        PostUpdate,
        position_camera.before(bevy::transform::TransformSystems::Propagate),
    )
    .add_systems(
        PostUpdate,
        capture.after(bevy::transform::TransformSystems::Propagate),
    );
    if !app.is_plugin_added::<crate::app::game_plugins::SceneRuntimePlugin>() {
        app.add_plugins(crate::app::game_plugins::SceneRuntimePlugin);
    }
    #[cfg(feature = "validation")]
    super::validation::install(&mut app);
    match app.run() {
        bevy::app::AppExit::Success => Ok(()),
        e => Err(format!("作者预览退出失败：{e:?}")),
    }
}
fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_target_texture(
        wire::WIDTH,
        wire::HEIGHT,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = images.add(image);
    commands.spawn((
        PreviewCamera,
        Camera3d::default(),
        RenderTarget::Image(target.clone().into()),
        Msaa::Off,
        RenderLayers::layer(AUTHOR_RUNTIME_PREVIEW_LAYER),
        Transform::from_xyz(4., 2.6, 5.).looking_at(Vec3::new(0., 1., 0.), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 14000.,
            shadow_maps_enabled: false,
            ..default()
        },
        RenderLayers::layer(AUTHOR_RUNTIME_PREVIEW_LAYER),
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, -0.6, -0.8, 0.)),
    ));
    commands.insert_resource(Target(target));
}
fn receive(
    mut commands: Commands,
    mut peer: ResMut<Peer>,
    mut status: ResMut<AuthorRuntimePreviewStatus>,
    mut evidence: ResMut<AuthorRuntimePreviewEvidence>,
    mut exit: MessageWriter<AppExit>,
) {
    if peer.stopping.is_some() {
        finish_shutdown(&mut peer, &mut exit);
        return;
    }
    if let Ok(result) = peer.writer_done.try_recv() {
        let error = result
            .err()
            .unwrap_or_else(|| "输出线程在停止请求前结束".into());
        error!("作者预览输出失败：{error}");
        exit.write(AppExit::error());
        return;
    }
    for _ in 0..2 {
        let packet = match peer.input.try_recv() {
            Ok(value) => value,
            Err(async_channel::TryRecvError::Empty) => break,
            Err(_) => {
                error!("作者预览输入管道在停止请求前关闭");
                exit.write(AppExit::error());
                return;
            }
        };
        if packet.project != peer.project || packet.session != peer.session {
            continue;
        }
        if packet.message == "camera" {
            if packet.revision == peer.revision {
                peer.camera = [
                    packet.data["yaw"]
                        .as_f64()
                        .unwrap_or(0.67)
                        .rem_euclid(std::f64::consts::TAU) as f32,
                    (packet.data["pitch"].as_f64().unwrap_or(0.25) as f32).clamp(-1.5, 1.5),
                    (packet.data["distance"].as_f64().unwrap_or(6.5) as f32).clamp(0.2, 100.),
                ];
            }
            continue;
        }
        if packet.message == "stop" {
            let reply = Packet {
                message: "stopped".into(),
                data: serde_json::Value::Null,
                ..packet
            };
            peer.input.close();
            peer.stopping = Some(Shutdown {
                reply: Some(reply),
                started: Instant::now(),
                flushed: false,
            });
            finish_shutdown(&mut peer, &mut exit);
            return;
        }
        if !matches!(packet.message.as_str(), "preview" | "clear") {
            continue; // 不让错误方向的ready/frame/error推进请求身份。
        }
        if packet.revision <= peer.revision {
            continue;
        }
        peer.revision = packet.revision;
        peer.kind.clone_from(&packet.kind);
        peer.error_sent = None;
        *status = AuthorRuntimePreviewStatus::default();
        *evidence = AuthorRuntimePreviewEvidence::default();
        if packet.message == "preview" {
            commands.insert_resource(AuthorRuntimePreview {
                revision: packet.revision,
                kind: packet.kind,
                payload: packet.data,
            });
        } else if packet.message == "clear" {
            commands.remove_resource::<AuthorRuntimePreview>();
            commands.remove_resource::<AuthorRuntimePreviewScene>();
            // 根由领域插件拥有；SDK只清除引用，避免与领域clear在同一帧重复递归销毁。
            peer.scene = None;
        }
    }
}

fn finish_shutdown(peer: &mut Peer, exit: &mut MessageWriter<AppExit>) {
    let Some(mut shutdown) = peer.stopping.take() else {
        return;
    };
    if let Some(reply) = shutdown.reply.take() {
        match peer.output.try_send(Output::Control(reply)) {
            Ok(()) => (),
            Err(mpsc::TrySendError::Full(Output::Control(reply))) => shutdown.reply = Some(reply),
            Err(_) => {
                error!("作者预览停止回执无法写入：输出线程已关闭");
                exit.write(AppExit::error());
                return;
            }
        }
    }
    match peer.writer_done.try_recv() {
        Ok(Ok(())) => shutdown.flushed = true,
        Ok(Err(error)) => {
            error!("作者预览停止回执写入失败：{error}");
            exit.write(AppExit::error());
            return;
        }
        Err(_) => (),
    }
    // 只读取完成标志；不join、不等待管道或GPU。父进程已经关闭发送端，读线程会在EOF真实结束。
    if shutdown.flushed
        && peer.reader.as_ref().is_none_or(JoinHandle::is_finished)
        && peer.writer.as_ref().is_none_or(JoinHandle::is_finished)
    {
        exit.write(AppExit::Success);
        return;
    }
    if shutdown.started.elapsed() >= Duration::from_millis(1500) {
        error!(
            "作者预览停止超时：回执已刷入={}，输入线程结束={}，输出线程结束={}",
            shutdown.flushed,
            peer.reader.as_ref().is_none_or(JoinHandle::is_finished),
            peer.writer.as_ref().is_none_or(JoinHandle::is_finished)
        );
        exit.write(AppExit::error());
        return;
    }
    peer.stopping = Some(shutdown);
}

fn position_camera(
    peer: Res<Peer>,
    scene: Option<Res<AuthorRuntimePreviewScene>>,
    roots: Query<&GlobalTransform>,
    mut cameras: Query<&mut Transform, With<PreviewCamera>>,
) {
    let Some(scene) =
        scene.filter(|scene| scene.revision == peer.revision && scene.kind == peer.kind)
    else {
        return;
    };
    if let Ok(root) = roots.get(scene.root)
        && let Ok(mut camera) = cameras.single_mut()
    {
        let center = root.translation() + Vec3::Y;
        let [yaw, pitch, distance] = peer.camera;
        let orbit = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        ) * distance;
        *camera = Transform::from_translation(center + orbit).looking_at(center, Vec3::Y);
    }
}

/// 领域证据和通用验收探针合并后仍须再次验预算，不能让后台writer因补充字段而杀掉整进程。
fn capture_evidence(
    domain: Option<&serde_json::Value>,
    animation: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let mut data = match domain {
        Some(data) if wire::evidence_within_budget(data) => data.clone(),
        Some(_) => return Err("作者预览领域执行证据超过固定预算".into()),
        None => serde_json::Value::Null,
    };
    if let Some(animation) = animation {
        if !data.is_object() {
            data = serde_json::json!({});
        }
        data["animation_probe"] = animation;
    }
    if !wire::evidence_within_budget(&data) {
        return Err("作者预览执行证据与验收探针合并后超过4KiB、64节点或8层预算".into());
    }
    Ok(data)
}

fn send_error(peer: &mut Peer, request: &AuthorRuntimePreview, error: &str) {
    if peer
        .error_sent
        .as_ref()
        .is_some_and(|(revision, kind)| *revision == request.revision && *kind == request.kind)
    {
        return;
    }
    let packet = Packet {
        project: peer.project.clone(),
        session: peer.session,
        revision: request.revision,
        message: "error".into(),
        kind: request.kind.clone(),
        data: serde_json::json!({"message":error.chars().take(4096).collect::<String>()}),
    };
    if peer.output.try_send(Output::Control(packet)).is_ok() {
        peer.error_sent = Some((request.revision, request.kind.clone()));
    }
}

fn capture(
    mut commands: Commands,
    mut peer: ResMut<Peer>,
    target: Res<Target>,
    request: Option<Res<AuthorRuntimePreview>>,
    scene: Option<Res<AuthorRuntimePreviewScene>>,
    status: Res<AuthorRuntimePreviewStatus>,
    evidence: Res<AuthorRuntimePreviewEvidence>,
    #[cfg(feature = "validation")] probe: Res<super::validation::Probe>,
    #[cfg(feature = "validation")] frame: Res<bevy::diagnostic::FrameCount>,
    mut exit: MessageWriter<AppExit>,
) {
    if peer.stopping.is_some() {
        return;
    }
    let Some(request) = request else {
        return;
    };
    if status.revision == request.revision
        && status.kind == request.kind
        && let Some(error) = &status.error
    {
        send_error(&mut peer, &request, error);
        return;
    }
    // 领域完整就绪才允许采集；旧根、仍在加载的场景和普通ready握手均不能充当预览成功。
    if !status.ready || status.revision != request.revision || status.kind != request.kind {
        return;
    }
    let Some(scene) = scene.filter(|s| s.revision == request.revision && s.kind == request.kind)
    else {
        return;
    };
    peer.scene = Some(scene.root);
    if let Some((_, started)) = peer.capture_pending {
        if started.elapsed() >= Duration::from_secs(30) {
            // GPU回读没有结束就不归还槽或另起任务；明确退出，交父Job确认收口再重开。
            error!("作者预览GPU回读30秒内未结束");
            exit.write(AppExit::error());
        }
        return;
    }
    if peer.last_capture.elapsed() < Duration::from_millis(66) {
        return;
    }
    peer.last_capture = Instant::now();
    let domain = (evidence.revision == request.revision && evidence.kind == request.kind)
        .then_some(&evidence.data);
    #[cfg(not(feature = "validation"))]
    let animation = None;
    #[cfg(feature = "validation")]
    let animation = super::validation::snapshot(&probe, &scene, frame.0);
    let data = match capture_evidence(domain, animation) {
        Ok(data) => data,
        Err(error) => {
            send_error(&mut peer, &request, &error);
            return;
        }
    };
    let packet = Packet {
        project: peer.project.clone(),
        session: peer.session,
        revision: request.revision,
        message: "frame".into(),
        kind: request.kind.clone(),
        data,
    };
    let captured_root = scene.root;
    let capture = commands
        .spawn(Screenshot::image(target.0.clone()))
        .observe(
            move |mut event: On<ScreenshotCaptured>,
                  mut peer: ResMut<Peer>,
                  scene: Option<Res<AuthorRuntimePreviewScene>>| {
                if peer.capture_pending.as_ref().map(|pending| pending.0) != Some(event.entity) {
                    return;
                }
                peer.capture_pending = None;
                if peer.stopping.is_none()
                    && peer.revision == packet.revision
                    && peer.kind == packet.kind
                    && scene.as_ref().is_some_and(|scene| {
                        scene.root == captured_root
                            && scene.revision == packet.revision
                            && scene.kind == packet.kind
                    })
                {
                    // 此观察器独占该截图；移动完整像素所有权到后台，不在帧路径clone整张Image。
                    let image = std::mem::take(&mut event.image);
                    let _ = peer.output.try_send(Output::Capture(packet.clone(), image));
                }
            },
        )
        .id();
    peer.capture_pending = Some((capture, Instant::now()));
}

#[cfg(test)]
mod tests;
