//! 无GPU回归验证真实截图事件的所有权移动、迟到拒收、证据身份和非阻塞停止回执。
use super::*;

type Fixture = (
    App,
    async_channel::Sender<Packet>,
    mpsc::Receiver<Output>,
    async_channel::Sender<Result<(), String>>,
);

fn app() -> Fixture {
    let (input_tx, input) = async_channel::bounded(2);
    let (output, output_rx) = mpsc::sync_channel(2);
    let (writer_complete, writer_done) = async_channel::bounded(1);
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<AuthorRuntimePreviewStatus>()
        .init_resource::<AuthorRuntimePreviewEvidence>()
        .insert_resource(Target(Handle::default()))
        .insert_resource(Peer {
            input,
            output,
            writer_done,
            reader: None,
            writer: None,
            stopping: None,
            project: "fixture".into(),
            session: 7,
            revision: 1,
            kind: "fixture.preview".into(),
            error_sent: None,
            capture_pending: None,
            last_capture: Instant::now() - Duration::from_secs(1),
            scene: None,
            camera: [0.67, 0.25, 6.5],
        })
        .add_systems(PreUpdate, receive)
        .add_systems(PostUpdate, capture);
    #[cfg(feature = "validation")]
    app.init_resource::<super::super::validation::Probe>();
    (app, input_tx, output_rx, writer_complete)
}

fn ready(app: &mut App) {
    let root = app.world_mut().spawn_empty().id();
    app.insert_resource(AuthorRuntimePreview {
        revision: 1,
        kind: "fixture.preview".into(),
        payload: serde_json::Value::Null,
    })
    .insert_resource(AuthorRuntimePreviewScene {
        revision: 1,
        kind: "fixture.preview".into(),
        root,
    })
    .insert_resource(AuthorRuntimePreviewStatus {
        revision: 1,
        kind: "fixture.preview".into(),
        ready: true,
        error: None,
    })
    .insert_resource(AuthorRuntimePreviewEvidence {
        revision: 1,
        kind: "fixture.preview".into(),
        data: serde_json::json!({"elapsed":0.5,"vfx_emitted":2}),
    });
}

fn screenshot(app: &mut App) -> (Entity, Image, usize) {
    let entity = app.world().resource::<Peer>().capture_pending.unwrap().0;
    let mut image = Image::new_target_texture(
        wire::WIDTH,
        wire::HEIGHT,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.data = Some(vec![27; wire::MAX_PIXELS]);
    let address = image.data.as_ref().unwrap().as_ptr() as usize;
    (entity, image, address)
}

#[test]
fn completed_screenshot_moves_pixels_and_current_execution_evidence() {
    let (mut app, _input, output, _done) = app();
    ready(&mut app);
    app.update();
    let (entity, image, address) = screenshot(&mut app);
    app.world_mut()
        .trigger(ScreenshotCaptured { entity, image });
    let Output::Capture(packet, image) = output.try_recv().unwrap() else {
        panic!("必须是实际截图回传")
    };
    assert_eq!(
        image.data.as_ref().unwrap().as_ptr() as usize,
        address,
        "不得在主帧深拷贝像素"
    );
    assert_eq!(packet.data["vfx_emitted"], 2);
    assert!(app.world().resource::<Peer>().capture_pending.is_none());
}

#[test]
fn stale_gpu_callback_cannot_replace_new_kind_or_revision() {
    let (mut app, _input, output, _done) = app();
    ready(&mut app);
    app.update();
    let (entity, image, _) = screenshot(&mut app);
    {
        let mut peer = app.world_mut().resource_mut::<Peer>();
        peer.revision = 2;
        peer.kind = "another.preview".into();
    }
    app.world_mut()
        .trigger(ScreenshotCaptured { entity, image });
    assert!(output.try_recv().is_err());
    assert!(app.world().resource::<Peer>().capture_pending.is_none());
}

#[test]
fn same_revision_gpu_callback_rejects_a_replaced_scene_root() {
    let (mut app, _input, output, _done) = app();
    ready(&mut app);
    app.update();
    let (entity, image, _) = screenshot(&mut app);
    let other = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<AuthorRuntimePreviewScene>()
        .root = other;
    app.world_mut()
        .trigger(ScreenshotCaptured { entity, image });
    assert!(output.try_recv().is_err());
    assert!(app.world().resource::<Peer>().capture_pending.is_none());
}

#[test]
fn not_ready_or_foreign_status_never_starts_gpu_capture() {
    let (mut app, _input, _output, _done) = app();
    ready(&mut app);
    app.world_mut()
        .resource_mut::<AuthorRuntimePreviewStatus>()
        .kind = "foreign".into();
    app.update();
    assert!(app.world().resource::<Peer>().capture_pending.is_none());
    {
        let mut state = app.world_mut().resource_mut::<AuthorRuntimePreviewStatus>();
        state.kind = "fixture.preview".into();
        state.ready = false;
    }
    app.update();
    assert!(app.world().resource::<Peer>().capture_pending.is_none());
}

#[test]
fn stop_retries_full_queue_and_exits_only_after_background_flush() {
    let (mut app, input, output, done) = app();
    let packet = Packet {
        project: "fixture".into(),
        session: 7,
        revision: 1,
        message: "stop".into(),
        kind: String::new(),
        data: serde_json::Value::Null,
    };
    for _ in 0..2 {
        app.world()
            .resource::<Peer>()
            .output
            .try_send(Output::Control(packet.clone()))
            .unwrap();
    }
    input.try_send(packet).unwrap();
    input.close();
    app.update();
    assert!(
        app.should_exit().is_none(),
        "队列满时不能阻塞或丢停止回执后直接退出"
    );
    output.try_recv().unwrap();
    output.try_recv().unwrap();
    app.update();
    let Output::Control(stopped) = output.try_recv().unwrap() else {
        panic!("必须回复停止")
    };
    assert_eq!(stopped.message, "stopped");
    assert!(app.should_exit().is_none(), "入队不等于已写管道");
    done.try_send(Ok(())).unwrap();
    app.update();
    assert_eq!(app.should_exit(), Some(AppExit::Success));
}

#[test]
fn finite_f64_camera_yaw_is_normalized_before_narrowing_to_f32() {
    let (mut app, input, _output, _done) = app();
    input
        .try_send(Packet {
            project: "fixture".into(),
            session: 7,
            revision: 1,
            message: "camera".into(),
            kind: String::new(),
            data: serde_json::json!({"yaw":1e300,"pitch":0.3,"distance":4.0}),
        })
        .unwrap();
    app.update();
    assert!(
        app.world()
            .resource::<Peer>()
            .camera
            .iter()
            .all(|value| value.is_finite())
    );
}

#[test]
fn animation_and_skill_render_evidence_share_the_wire_budget() {
    // 真实失败来自两份各自合规的证据合并；UUID与数字均取本协议的最大编码预算。
    let animation = serde_json::json!({
        "capture_frame":u32::MAX,"clip":"ffffffff-ffff-ffff-ffff-ffffffffffff",
        "current_pose_delta":1.0,"derived_clip":true,
        "graph":"ffffffff-ffff-ffff-ffff-ffffffffffff","kind":"rpg.skill.v1",
        "matched_targets":16,"maximum_pose_delta":1.0,"player":u64::MAX,"player_name":"Player",
        "pose_sample":{"baseline_rotation":[0.0,0.0,0.0,1.0],"baseline_translation":[0.0,0.0,0.0],
            "entity":u64::MAX,"rotation":[0.0,0.0,1.0,0.0],
            "target":"ffffffff-ffff-ffff-ffff-ffffffffffff","translation":[0.0,0.0,0.0]},
        "revision":u64::MAX,"root":u64::MAX,"samples":u64::MAX,"scan_complete":true,"seek":1.0,
        "source":"real-animation-player-graph-and-animated-targets"
    });
    let domain = serde_json::json!({"initialized":true,"elapsed":999.5,"completed":false,
        "vfx_emitted":u64::MAX,"vfx_render":{"root":u64::MAX,"ticket":u64::MAX,
            "frame":u32::MAX,"meshes":16}});
    let combined = capture_evidence(Some(&domain), Some(animation)).unwrap();
    assert!(wire::evidence_within_budget(&combined));
    assert_eq!(combined["vfx_render"]["meshes"], 16);
    assert_eq!(combined["animation_probe"]["matched_targets"], 16);
    let packet = Packet {
        project: "fixture".into(),
        session: 7,
        revision: 1,
        message: "frame".into(),
        kind: "rpg.skill.v1".into(),
        data: combined,
    };
    assert!(wire::write(&mut Vec::new(), &packet, &vec![0; wire::MAX_PIXELS]).is_ok());
}

#[test]
fn oversized_evidence_reports_current_request_error_without_terminating_preview() {
    let (mut app, _input, output, _done) = app();
    ready(&mut app);
    app.world_mut()
        .resource_mut::<AuthorRuntimePreviewEvidence>()
        .data = serde_json::json!({"too_large":"x".repeat(wire::MAX_EVIDENCE)});
    app.update();
    let Output::Control(packet) = output.try_recv().unwrap() else {
        panic!("超预算应回传请求错误")
    };
    assert_eq!(packet.message, "error");
    assert_eq!(packet.revision, 1);
    assert!(
        packet.data["message"]
            .as_str()
            .unwrap()
            .contains("证据超过")
    );
    assert!(app.should_exit().is_none());
    assert!(app.world().resource::<Peer>().capture_pending.is_none());
}
