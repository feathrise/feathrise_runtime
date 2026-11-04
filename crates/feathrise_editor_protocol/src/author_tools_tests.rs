//! 工具合同回归覆盖真实语义、独立预算与读写双端拒绝，不借助Bevy或渲染资源。

use crate::*;
use sha2::{Digest, Sha256};
use std::{io::Cursor, sync::Arc};

fn brush() -> BrushPreview {
    BrushPreview {
        spatial: false,
        center: [1., 2., 3.],
        rotation: [0., 0., 0., 1.],
        radius: 4.,
        strength: Some(0.75),
        shape: BrushShape::Planar,
        half_height: 2.,
        flatten: false,
        color: [1., 0.5, 0., 1.],
        clipmap: Some(ClipmapBrushSurface {
            map_id: "地图-a".into(),
            terrain_id: Some("terrain-a".into()),
            height_path: "maps/terrain-a.height.png".into(),
            authoring_height_revision: 0,
        }),
    }
}

fn move_tool() -> TransformPreview {
    TransformPreview::Move {
        spatial: false,
        origin: [0., 1., 2.],
        length: 8.,
        axes: 0b111,
        active_axes: 0b001,
        plane: true,
        plane_active: false,
        plane_cross: true,
        plane_color: [0.5, 0.5, 0., 1.],
        color_active: [1.; 4],
        axis_colors: [[1., 0., 0., 1.], [0., 1., 0., 1.], [0., 0., 1., 1.]],
        anchor: Some([1., 2., 3.]),
        marker_radius: Some(0.2),
    }
}

fn selection() -> SelectionPreview {
    SelectionPreview {
        target: "objects/房屋-a".into(),
        center: [2., 4., 8.],
        // 镜像、非均匀缩放及剪切必须按完整半轴保留，不得退化为轴对齐包围框。
        half_axes: [[-2., 0., 0.], [1., 3., 0.], [0., 0.5, 4.]],
        color: [2., 0.5, 0., 1.],
    }
}

fn message(preview: AuthorToolPreview) -> Envelope {
    Envelope::new(
        SessionIdentity {
            project_id: "工具夹具".into(),
            session: 9,
            generation: 2,
        },
        1,
        Message::SetAuthorTools {
            revision: 7,
            preview,
        },
    )
}

/// 绕过writer产生摘要正确的原始帧，证明reader没有依赖发送端先行校验。
fn unchecked_frame(message: &Envelope) -> Vec<u8> {
    let metadata = serde_json::to_vec(message).unwrap();
    let mut header = [0_u8; 56];
    header[..8].copy_from_slice(b"FTHRIPC1");
    header[8..12].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(metadata.len() as u32).to_le_bytes());
    let mut digest = Sha256::new();
    digest.update(&header[..24]);
    digest.update(&metadata);
    header[24..56].copy_from_slice(&digest.finalize());
    [header.as_slice(), metadata.as_slice()].concat()
}

#[test]
fn author_tools_roundtrip_all_semantics_and_explicit_empty_clear() {
    let mut items = Vec::new();
    for shape in [
        BrushShape::Planar,
        BrushShape::Sphere,
        BrushShape::Box,
        BrushShape::Cylinder,
    ] {
        items.push(AuthorTool::Brush(BrushPreview { shape, ..brush() }));
    }
    items.extend([
        AuthorTool::Transform(move_tool()),
        AuthorTool::Transform(TransformPreview::Rotate {
            origin: [1., 2., 3.],
            radius: 4.,
            yaw_degrees: Some(25.),
            before_yaw_degrees: Some(-10.),
            color: [1.; 4],
        }),
        AuthorTool::Transform(TransformPreview::Scale {
            origin: [1., 2., 3.],
            right: [1., 0., 0.],
            up: [0., 1., 0.],
            half_size: 0.5,
            color: [1.; 4],
            cube: false,
        }),
        AuthorTool::Selection(selection()),
    ]);
    for preview in [AuthorToolPreview { items }, AuthorToolPreview::default()] {
        let expected = message(preview);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected, &Limits::default()).unwrap();
        let actual = read_frame(&mut Cursor::new(bytes), &Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(actual.message, expected.message);
        assert!(actual.payload.is_none());
    }
}

#[test]
fn clipmap_brush_wire_contains_only_stable_author_identity() {
    let surface = serde_json::to_value(brush().clipmap.unwrap()).unwrap();
    assert_eq!(surface, serde_json::json!({
        "map_id":"地图-a", "terrain_id":"terrain-a",
        "height_path":"maps/terrain-a.height.png", "authoring_height_revision":0
    }));
    for (field, value) in [
        ("revision", serde_json::json!([0, 7, u64::MAX])),
        ("missing_pages", serde_json::to_value([[u32::MAX, 0, 1, 2]; 16]).unwrap()),
    ] {
        let mut forbidden = surface.clone();
        forbidden.as_object_mut().unwrap().insert(field.into(), value);
        assert!(serde_json::from_value::<ClipmapBrushSurface>(forbidden).is_err(),
            "跨World合同必须拒绝本地GPU字段{field}");
    }
}

#[test]
fn author_tools_each_kind_has_an_independent_atomic_count_budget() {
    let mut full = AuthorToolPreview {
        items: vec![AuthorTool::Brush(brush()); MAX_AUTHOR_BRUSHES],
    };
    full.items.extend(vec![
        AuthorTool::Transform(move_tool());
        MAX_AUTHOR_TRANSFORMS
    ]);
    full.items.extend(vec![
        AuthorTool::Selection(selection());
        MAX_AUTHOR_SELECTIONS
    ]);
    assert_eq!(full.items.len(), MAX_AUTHOR_TOOLS);
    assert_eq!(full.segment_budget().unwrap(), MAX_AUTHOR_TOOL_SEGMENTS);
    assert_eq!(MAX_AUTHOR_BRUSHES, 16, "原空间线段10个盖印轮廓必须完整保留");
    for (item, limit) in [
        (AuthorTool::Brush(brush()), MAX_AUTHOR_BRUSHES),
        (AuthorTool::Transform(move_tool()), MAX_AUTHOR_TRANSFORMS),
        (AuthorTool::Selection(selection()), MAX_AUTHOR_SELECTIONS),
    ] {
        let preview = AuthorToolPreview {
            items: vec![item.clone(); limit + 1],
        };
        assert!(matches!(
            preview.validate(),
            Err(AuthorToolValidationError::Limit(_))
        ));
        assert!(matches!(
            write_frame(&mut Vec::new(), &message(preview), &Limits::default()),
            Err(ProtocolError::Limit(_))
        ));
        let mut over_total = full.clone();
        over_total.items.push(item);
        assert!(matches!(
            over_total.segment_budget(),
            Err(AuthorToolValidationError::Limit(_))
        ));
    }
}

fn changed_brush(change: impl FnOnce(&mut BrushPreview)) -> AuthorTool {
    let mut preview = brush();
    change(&mut preview);
    AuthorTool::Brush(preview)
}

fn changed_selection(change: impl FnOnce(&mut SelectionPreview)) -> AuthorTool {
    let mut preview = selection();
    change(&mut preview);
    AuthorTool::Selection(preview)
}

#[test]
fn author_tools_reject_nonfinite_invalid_sizes_identities_colors_and_quaternions() {
    let invalid = vec![
        changed_brush(|brush| brush.center[0] = f32::NAN),
        changed_brush(|brush| brush.radius = -1.),
        changed_brush(|brush| brush.radius = f32::INFINITY),
        changed_brush(|brush| brush.half_height = -1.),
        changed_brush(|brush| brush.strength = Some(-0.1)),
        changed_brush(|brush| brush.strength = Some(1.1)),
        changed_brush(|brush| brush.rotation = [0.; 4]),
        changed_brush(|brush| brush.rotation = [0., 0., 0., 100.]),
        changed_brush(|brush| brush.color[0] = -0.1),
        changed_brush(|brush| brush.color[3] = 1.1),
        changed_brush(|brush| brush.clipmap.as_mut().unwrap().map_id.clear()),
        changed_brush(|brush| brush.clipmap.as_mut().unwrap().map_id = "a".repeat(257)),
        changed_brush(|brush| brush.clipmap.as_mut().unwrap().terrain_id = Some(String::new())),
        changed_brush(|brush| brush.clipmap.as_mut().unwrap().height_path.clear()),
        changed_brush(|brush| brush.clipmap.as_mut().unwrap().height_path = "a\0b".into()),
        changed_brush(|brush| brush.clipmap.as_mut().unwrap().height_path = "a".repeat(4097)),
        changed_selection(|selection| selection.target.clear()),
        changed_selection(|selection| selection.target = "a\0b".into()),
        changed_selection(|selection| selection.target = "a".repeat(1025)),
        changed_selection(|selection| selection.center[0] = f32::INFINITY),
        changed_selection(|selection| {
            selection.center[0] = f32::MAX;
            selection.half_axes[0][0] = f32::MAX;
        }),
    ];
    // JSON的非有限可选浮点会变成null/None，接收端无法恢复原Some(NaN)。
    // 这类非法内存值由writer先拒绝，不能将归一后的wire当作非法数值。
    for strength in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let invalid = message(AuthorToolPreview {
            items: vec![changed_brush(|brush| brush.strength = Some(strength))],
        });
        assert!(matches!(
            write_frame(&mut Vec::new(), &invalid, &Limits::default()),
            Err(ProtocolError::Invalid(_))
        ));
    }
    for item in invalid {
        let message = message(AuthorToolPreview { items: vec![item] });
        assert!(matches!(
            write_frame(&mut Vec::new(), &message, &Limits::default()),
            Err(ProtocolError::Invalid(_))
        ));
        assert!(matches!(
            read_frame(
                &mut Cursor::new(unchecked_frame(&message)),
                &Limits::default()
            ),
            Err(ProtocolError::Invalid(_))
        ));
    }
}

#[test]
fn author_tools_transform_masks_and_screen_basis_cannot_drift_outside_contract() {
    let mut invalid = Vec::new();
    for (axes, active_axes) in [(0b1000, 0), (0b001, 0b010)] {
        let mut preview = move_tool();
        if let TransformPreview::Move {
            axes: enabled,
            active_axes: active,
            ..
        } = &mut preview
        {
            *enabled = axes;
            *active = active_axes;
        }
        invalid.push(preview);
    }
    invalid.push(TransformPreview::Rotate {
        origin: [0.; 3],
        radius: 1.,
        yaw_degrees: None,
        before_yaw_degrees: Some(f32::NAN),
        color: [1.; 4],
    });
    for (right, up, half_size) in [
        ([2., 0., 0.], [0., 1., 0.], 1.),
        ([1., 0., 0.], [1., 0., 0.], 1.),
        ([1., 0., 0.], [0., 1., 0.], -1.),
    ] {
        invalid.push(TransformPreview::Scale {
            origin: [0.; 3],
            right,
            up,
            half_size,
            color: [1.; 4],
            cube: false,
        });
    }
    for preview in invalid {
        assert!(
            message(AuthorToolPreview {
                items: vec![AuthorTool::Transform(preview)]
            })
            .validate(&Limits::default())
            .is_err()
        );
    }
}

#[test]
fn author_tools_require_revision_reject_payload_and_deny_unknown_fields() {
    let mut missing_revision = message(AuthorToolPreview::default());
    if let Message::SetAuthorTools { revision, .. } = &mut missing_revision.message {
        *revision = 0;
    }
    assert!(missing_revision.validate(&Limits::default()).is_err());
    let extra_payload = message(AuthorToolPreview::default()).with_payload(Arc::new(vec![1]));
    assert!(extra_payload.validate(&Limits::default()).is_err());
    let valid = message(AuthorToolPreview {
        items: vec![AuthorTool::Brush(brush())],
    });
    for depth in 0..3 {
        let mut value = serde_json::to_value(&valid).unwrap();
        let object = match depth {
            0 => &mut value["message"],
            1 => &mut value["message"]["preview"],
            _ => &mut value["message"]["preview"]["items"][0]["preview"],
        };
        object
            .as_object_mut()
            .unwrap()
            .insert("arbitrary_geometry".into(), serde_json::json!([]));
        assert!(serde_json::from_value::<Envelope>(value).is_err());
    }
}

#[test]
fn author_tools_metadata_has_its_own_128kib_read_and_write_budget() {
    let at_normal_budget = message(AuthorToolPreview {
        items: vec![
            AuthorTool::Selection(SelectionPreview {
                target: "a".repeat(128),
                ..selection()
            });
            MAX_AUTHOR_SELECTIONS
        ],
    });
    let normal_bytes = serde_json::to_vec(&at_normal_budget).unwrap().len();
    assert!((32 * 1024..128 * 1024).contains(&normal_bytes));
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &at_normal_budget, &Limits::default()).unwrap();
    assert_eq!(
        read_frame(&mut Cursor::new(bytes), &Limits::default())
            .unwrap()
            .unwrap()
            .message,
        at_normal_budget.message
    );

    // 单项身份和项数仍有效，但完整帧超过字节预算时不得截断后采用。
    let over_budget = message(AuthorToolPreview {
        items: vec![
            AuthorTool::Selection(SelectionPreview {
                target: "a".repeat(1024),
                ..selection()
            });
            MAX_AUTHOR_SELECTIONS
        ],
    });
    assert!(over_budget.validate(&Limits::default()).is_ok());
    assert!(serde_json::to_vec(&over_budget).unwrap().len() > 128 * 1024);
    assert!(matches!(
        write_frame(&mut Vec::new(), &over_budget, &Limits::default()),
        Err(ProtocolError::Limit(_))
    ));
    assert!(matches!(
        read_frame(
            &mut Cursor::new(unchecked_frame(&over_budget)),
            &Limits::default()
        ),
        Err(ProtocolError::Limit(_))
    ));
}
