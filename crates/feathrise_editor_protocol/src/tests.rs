//! 进程边界回归：任意拆包、预算、完整性、迟到代次和邮箱背压独立于 Bevy 验证。

use crate::*;
use std::{
    io::{self, Cursor, Read},
    sync::Arc,
};

fn identity() -> SessionIdentity {
    SessionIdentity {
        project_id: "中文工程-a".into(),
        session: 701,
        generation: 1,
    }
}

#[test]
fn author_view_parameters_round_trip_and_reject_invalid_grid() {
    let view = AuthorViewState { shading:[3.,1.,0.,1.],wire:[1.,1.,0.,0.],
        grid:Some([[0.,0.,0.,1.],[0.,1.,0.,0.],[1.,0.,0.,0.],[0.,0.,1.,0.]]),
        terrain_flat:false,terrain_grid:true,terrain_backfaces:false,physics_debug:false };
    let envelope=Envelope::new(identity(),1,Message::SetAuthorView { view });
    let mut input=Cursor::new(encoded(&envelope));
    assert_eq!(read_frame(&mut input,&Limits::default()).unwrap().unwrap().message,envelope.message);
    for bad in [f32::NAN,-1.,0.] {
        let mut view=view;
        view.grid.as_mut().unwrap()[0][3]=bad;
        assert!(Envelope::new(identity(),1,Message::SetAuthorView{view}).validate(&Limits::default()).is_err());
    }
}

fn control(sequence: u64) -> Envelope {
    Envelope::new(
        identity(),
        sequence,
        Message::SetMode {
            request_id: sequence,
            mode: Mode::Editing,
            required_revision: 0,
        },
    )
}

fn snapshot() -> Envelope {
    Envelope::new(
        identity(),
        2,
        Message::Snapshot {
            transaction: 4,
            revision: 7,
            scene_id: "SCENE-a".into(),
            format: "feathrise-scene-v2".into(),
        },
    )
    .with_payload(Arc::new((0..131_071).map(|index| index as u8).collect()))
}

fn encoded(message: &Envelope) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, message, &Limits::default()).unwrap();
    bytes
}

#[test]
fn previous_frame_version_and_map_identity_are_rejected() {
    let snapshot = snapshot();
    let mut encoded = encoded(&snapshot);
    encoded[8..12].copy_from_slice(&1_u32.to_le_bytes());
    let mut input = Cursor::new(encoded);
    let error = read_frame(&mut input, &Limits::default()).unwrap_err();
    assert!(matches!(error, ProtocolError::Invalid(ref reason) if reason.contains("协议版本")));
    assert_eq!(input.position(), 56); // 固定帧头失败时未分配或读取旧快照正文。

    let mut metadata = serde_json::to_value(snapshot).unwrap();
    let message = metadata["message"].as_object_mut().unwrap();
    let id = message.remove("scene_id").expect("场景身份必须以 scene_id 编码");
    message.insert("map_id".into(), id);
    assert!(serde_json::from_value::<Envelope>(metadata).is_err());
}

#[test]
fn author_previews_keep_binary_payload_and_enforce_kind_budget_before_reading_it() {
    for kind in [PreviewKind::Placement, PreviewKind::Vegetation, PreviewKind::AuthorDomain, PreviewKind::SurfaceTrail] {
        let mut message = Envelope::new(identity(), 3, Message::Preview {
            kind, sequence: 8, scene_id: "map-a".into(), format: "author-preview-v1".into(),
        });
        assert!(message.validate(&Limits::default()).is_err());
        message.payload = Some(Arc::new(vec![0, 255, 17, 3]));
        let bytes = encoded(&message);
        let restored = read_frame(&mut Cursor::new(&bytes), &Limits::default()).unwrap().unwrap();
        assert_eq!(restored.message, message.message);
        assert_eq!(restored.payload, message.payload);
        let mut oversized = bytes;
        oversized[16..24].copy_from_slice(&((MAX_PREVIEW_BYTES + 1) as u64).to_le_bytes());
        assert!(matches!(read_frame(&mut Cursor::new(oversized), &Limits::default()), Err(ProtocolError::Limit(_))),
            "元信息已指出是Preview时，应在分配/读取大正文之前按64MiB分类预算拒绝");
        let ack = Envelope::new(identity(), 4, Message::PreviewApplied { kind, sequence: 8 });
        assert!(ack.validate(&Limits::default()).is_ok());
        assert!(ack.clone().with_payload(Arc::new(vec![1])).validate(&Limits::default()).is_err());
        assert!(Envelope::new(identity(), 5, Message::PreviewApplied {kind, sequence: 0})
            .validate(&Limits::default()).is_err());
    }
}

struct Fragmented {
    cursor: Cursor<Vec<u8>>,
    maximum: usize,
}
impl Read for Fragmented {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer.len().min(self.maximum);
        self.cursor.read(&mut buffer[..count])
    }
}

#[test]
fn arbitrary_fragmentation_preserves_complete_snapshot_and_following_control() {
    let expected = snapshot();
    let stream = [encoded(&expected), encoded(&control(3))].concat();
    for maximum in [1, 3, 55, 56, 4096, 32_768] {
        let mut reader = Fragmented {
            cursor: Cursor::new(stream.clone()),
            maximum,
        };
        let received = read_frame(&mut reader, &Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(received.identity, expected.identity);
        assert_eq!(received.message, expected.message);
        assert_eq!(received.payload, expected.payload);
        assert_eq!(
            read_frame(&mut reader, &Limits::default())
                .unwrap()
                .unwrap()
                .sequence,
            3
        );
        assert!(
            read_frame(&mut reader, &Limits::default())
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn truncated_header_metadata_and_payload_are_errors_not_successful_eof() {
    let frame = encoded(&snapshot());
    for length in [1, 7, 55, 56, 100, frame.len() - 1] {
        let result = read_frame(&mut Cursor::new(&frame[..length]), &Limits::default());
        assert!(
            matches!(result, Err(ProtocolError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
        );
    }
    assert!(
        read_frame(&mut Cursor::new([]), &Limits::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn checksum_covers_metadata_and_raw_payload() {
    let frame = encoded(&snapshot());
    for offset in [56 + 18, frame.len() - 1] {
        let mut corrupted = frame.clone();
        corrupted[offset] ^= 1;
        assert!(read_frame(&mut Cursor::new(corrupted), &Limits::default()).is_err());
    }
}

#[test]
fn malicious_declared_lengths_are_rejected_before_reading_or_allocating_body() {
    let mut header = encoded(&control(1));
    header.truncate(56);
    header[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(
        read_frame(&mut Cursor::new(header), &Limits::default()),
        Err(ProtocolError::Limit(_))
    ));
    let limits = Limits {
        metadata_bytes: 32 * 1024,
        snapshot_bytes: 64 * 1024,
    };
    assert!(matches!(
        write_frame(&mut Vec::new(), &snapshot(), &limits),
        Err(ProtocolError::Limit(_))
    ));
    // 超过原型 64 KiB 的完整正文在正式预算内保持全部字节，没有对象数截断。
    assert!(write_frame(&mut Vec::new(), &snapshot(), &Limits::default()).is_ok());
}

#[test]
fn empty_snapshots_and_control_payloads_cannot_bypass_message_contract() {
    let mut missing = snapshot();
    missing.payload = None;
    assert!(write_frame(&mut Vec::new(), &missing, &Limits::default()).is_err());
    let extra = control(1).with_payload(Arc::new(vec![1, 2, 3]));
    assert!(write_frame(&mut Vec::new(), &extra, &Limits::default()).is_err());
}

#[test]
fn foreign_sessions_duplicates_and_future_generations_are_rejected() {
    let mut guard = SessionGuard::new(identity());
    assert_eq!(guard.accept(&control(1)).unwrap(), Disposition::Current);
    assert!(guard.accept(&control(1)).is_err());
    let mut other = control(2);
    other.identity.project_id = "other-project".into();
    assert!(guard.accept(&other).is_err());
    other.identity = identity();
    other.identity.session += 1;
    assert!(guard.accept(&other).is_err());
    other.identity = identity();
    other.identity.generation += 1;
    assert!(guard.accept(&other).is_err());
    guard.advance_generation(2).unwrap();
    assert_eq!(
        guard.accept(&control(2)).unwrap(),
        Disposition::StaleGeneration
    );
    assert_eq!(guard.accept(&other).unwrap(), Disposition::Current);
    assert!(guard.advance_generation(2).is_err());
}

#[test]
fn generation_zero_cannot_enter_the_wire_contract() {
    let mut message = control(1);
    message.identity.generation = 0;
    assert!(matches!(
        write_frame(&mut Vec::new(), &message, &Limits::default()),
        Err(ProtocolError::Invalid(_))
    ));
}

#[test]
fn handshake_checks_actual_build_and_required_capabilities() {
    let ready = Envelope::new(
        identity(),
        1,
        Message::Ready {
            pid: 44,
            hwnd: 88,
            world_id: 9,
            build_identity: "artifact-sha256".into(),
            capabilities: vec!["resident-world-v1".into(), "scene-snapshot-v1".into()],
        },
    );
    assert!(
        SessionGuard::verify_handshake(&ready, "artifact-sha256", &["resident-world-v1"]).is_ok()
    );
    assert!(SessionGuard::verify_handshake(&ready, "old-artifact", &[]).is_err());
    assert!(SessionGuard::verify_handshake(&ready, "artifact-sha256", &["missing"]).is_err());
    assert!(SessionGuard::verify_handshake(&control(1), "artifact-sha256", &[]).is_err());
}

#[test]
fn mailbox_coalesces_latest_and_rejects_late_completion_after_consumption() {
    let mailbox = LatestMailbox::default();
    assert_eq!(
        mailbox.publish(1, Arc::new(vec![1])),
        PublishResult::Published
    );
    assert_eq!(
        mailbox.publish(3, Arc::new(vec![3])),
        PublishResult::Published
    );
    assert_eq!(mailbox.publish(2, Arc::new(vec![2])), PublishResult::Stale);
    assert_eq!(mailbox.take().unwrap(), (3, Arc::new(vec![3])));
    assert!(mailbox.take().is_none());
    assert_eq!(mailbox.publish(2, Arc::new(vec![2])), PublishResult::Stale);
    assert_eq!(
        mailbox.clone().publish(4, Arc::new(vec![4])),
        PublishResult::Published
    );
    assert_eq!(mailbox.take().unwrap().0, 4);
}

#[test]
fn invalid_numeric_camera_and_unidentified_ready_fail_closed() {
    let camera = Envelope::new(
        identity(),
        1,
        Message::SetCamera {
            camera: CameraState {
                position: [f32::NAN, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                vertical_fov_radians: 1.0,
                orthographic_height: None,
            },
        },
    );
    assert!(camera.validate(&Limits::default()).is_err());
    let ready = Envelope::new(
        identity(),
        1,
        Message::Ready {
            pid: 1,
            hwnd: 0,
            world_id: 1,
            build_identity: "build".into(),
            capabilities: vec![],
        },
    );
    assert!(ready.validate(&Limits::default()).is_err());
}

#[test]
fn orthographic_height_roundtrips_and_invalid_extents_are_rejected() {
    let camera = CameraState {
        position: [0.0, 10.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        vertical_fov_radians: 1.0,
        orthographic_height: Some(48.0),
    };
    let message = Envelope::new(identity(), 1, Message::SetCamera { camera });
    let decoded = read_frame(&mut Cursor::new(encoded(&message)), &Limits::default())
        .unwrap()
        .unwrap();
    assert_eq!(decoded.message, message.message);
    for height in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
        let invalid = Envelope::new(
            identity(),
            1,
            Message::SetCamera {
                camera: CameraState {
                    orthographic_height: Some(height),
                    ..camera
                },
            },
        );
        assert!(invalid.validate(&Limits::default()).is_err());
    }
}

fn overlay_line() -> OverlayLine {
    OverlayLine {
        start: [1.0, 2.0, 3.0],
        end: [4.0, 5.0, 6.0],
        start_color: [1.0, 0.0, 0.5, 1.0],
        end_color: [0.0, 1.0, 0.5, 1.0],
    }
}

#[test]
fn full_overlay_budget_roundtrips_and_empty_list_clears_previous_lines() {
    for lines in [vec![overlay_line(); 1024], Vec::new()] {
        let message = Envelope::new(identity(), 1, Message::SetOverlay { revision: 8, lines });
        let frame = encoded(&message);
        if let Message::SetOverlay { lines, .. } = &message.message {
            assert_eq!(frame.len() > 32 * 1024, !lines.is_empty());
        }
        let received = read_frame(&mut Cursor::new(frame), &Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(received.message, message.message);
    }
}

#[test]
fn overlay_rejects_excess_lines_nonfinite_numbers_and_missing_revision() {
    let mut invalid_position = overlay_line();
    invalid_position.start[0] = f32::NAN;
    let mut invalid_color = overlay_line();
    invalid_color.end_color[3] = f32::INFINITY;
    for (revision, lines) in [
        (1, vec![overlay_line(); 1025]),
        (1, vec![invalid_position]),
        (1, vec![invalid_color]),
        (0, Vec::new()),
    ] {
        let message = Envelope::new(identity(), 1, Message::SetOverlay { revision, lines });
        assert!(write_frame(&mut Vec::new(), &message, &Limits::default()).is_err());
    }
}

#[test]
fn ordinary_control_cannot_borrow_overlay_metadata_budget_on_read_or_write() {
    use sha2::{Digest, Sha256};
    // 原始诊断恰好在 8 KiB 限额内，JSON 转义后超过普通控制预算；读写都要独立拒绝。
    let message = Envelope::new(
        identity(),
        1,
        Message::Failure {
            request_id: 1,
            code: "fixture".into(),
            message: "\0".repeat(8 * 1024),
        },
    );
    assert!(message.validate(&Limits::default()).is_ok());
    assert!(matches!(
        write_frame(&mut Vec::new(), &message, &Limits::default()),
        Err(ProtocolError::Limit(_))
    ));
    let metadata = serde_json::to_vec(&message).unwrap();
    assert!(metadata.len() > 32 * 1024 && metadata.len() < 512 * 1024);
    let mut header = [0_u8; 56];
    header[..8].copy_from_slice(b"FTHRIPC1");
    header[8..12].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(metadata.len() as u32).to_le_bytes());
    let mut digest = Sha256::new();
    digest.update(&header[..24]);
    digest.update(&metadata);
    header[24..56].copy_from_slice(&digest.finalize());
    let frame = [header.as_slice(), metadata.as_slice()].concat();
    assert!(matches!(
        read_frame(&mut Cursor::new(frame), &Limits::default()),
        Err(ProtocolError::Limit(_))
    ));
}

fn viewport(excluded_rects: Vec<[u32; 4]>) -> Envelope {
    Envelope::new(
        identity(),
        1,
        Message::SetViewport {
            x: -12,
            y: 48,
            width: 800,
            height: 600,
            visible: true,
            focused: false,
            excluded_rects,
            excluded_shapes: Vec::new(),
        },
    )
}

#[test]
fn viewport_orientation_shapes_roundtrip_and_reject_invalid_or_unbounded_masks() {
    use crate::{ViewportUiShape, MAX_VIEWPORT_EXCLUDED_SHAPES};
    let circle = ViewportUiShape::Ellipse { outer: [10, 10, 30, 30], inner: Some([13, 13, 27, 27]) };
    let line = ViewportUiShape::Quad { points: [[10, 40], [12, 38], [32, 58], [30, 60]] };
    let mut message = viewport(Vec::new());
    if let Message::SetViewport { excluded_shapes, .. } = &mut message.message {
        *excluded_shapes = vec![circle.clone(), line];
    }
    assert!(message.validate(&Limits::default()).is_ok());
    let json = serde_json::to_vec(&message).unwrap();
    let restored: Envelope = serde_json::from_slice(&json).unwrap();
    assert_eq!(message.message, restored.message);
    for shapes in [
        vec![circle; MAX_VIEWPORT_EXCLUDED_SHAPES + 1],
        vec![ViewportUiShape::Ellipse { outer: [0, 0, 10, 10], inner: Some([-1, 2, 8, 8]) }],
        vec![ViewportUiShape::Ellipse { outer: [i32::MAX, 0, i32::MAX, 10], inner: None }],
        vec![ViewportUiShape::Quad { points: [[0, 0], [10, 10], [10, 0], [0, 10]] }],
    ] {
        if let Message::SetViewport { excluded_shapes, .. } = &mut message.message { *excluded_shapes = shapes; }
        assert!(message.validate(&Limits::default()).is_err());
    }
}

#[test]
fn viewport_exclusions_roundtrip_at_budget_and_empty_list_clears_them() {
    for rectangles in [
        Vec::new(),
        vec![[0, 0, 800, 600]; MAX_VIEWPORT_EXCLUDED_RECTS],
    ] {
        let message = viewport(rectangles);
        let received = read_frame(&mut Cursor::new(encoded(&message)), &Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(received.message, message.message);
    }
}

#[test]
fn viewport_exclusions_reject_excess_count_empty_bounds_overflow_and_outside_edges() {
    for rectangles in [
        vec![[1, 2, 3, 4]; MAX_VIEWPORT_EXCLUDED_RECTS + 1],
        vec![[0, 0, 0, 1]],
        vec![[0, 0, 1, 0]],
        vec![[800, 0, 1, 1]],
        vec![[0, 600, 1, 1]],
        vec![[799, 0, 2, 1]],
        vec![[0, 599, 1, 2]],
        vec![[u32::MAX, 0, 1, 1]],
        vec![[0, u32::MAX, 1, 1]],
    ] {
        let message = viewport(rectangles);
        assert!(write_frame(&mut Vec::new(), &message, &Limits::default()).is_err());
        // 绕过本端writer生成摘要正确的伪帧，接收端也必须独立拒绝同一无效裁剪。
        use sha2::{Digest, Sha256};
        let metadata = serde_json::to_vec(&message).unwrap();
        let mut header = [0_u8; 56];
        header[..8].copy_from_slice(b"FTHRIPC1");
        header[8..12].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&(metadata.len() as u32).to_le_bytes());
        let mut digest = Sha256::new();
        digest.update(&header[..24]);
        digest.update(&metadata);
        header[24..56].copy_from_slice(&digest.finalize());
        let frame = [header.as_slice(), metadata.as_slice()].concat();
        assert!(matches!(
            read_frame(&mut Cursor::new(frame), &Limits::default()),
            Err(ProtocolError::Invalid(_))
        ));
    }
}

#[test]
fn viewport_exclusions_default_empty_for_legacy_metadata() {
    // 消息格式兼容读取缺省字段；是否允许连接仍由新必需能力握手单独约束。
    let message: Message = serde_json::from_value(serde_json::json!({
        "kind":"set_viewport","x":0,"y":0,"width":800,"height":600,"visible":true,"focused":false
    }))
    .unwrap();
    let Message::SetViewport { excluded_rects, .. } = message else {
        panic!("视口消息未解码");
    };
    assert!(excluded_rects.is_empty());
}
