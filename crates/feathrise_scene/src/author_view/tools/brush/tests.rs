//! 直接核对统一renderer的正式Gizmo缓冲，防止跨视口迁移丢失姿态、强度或整平轮廓。

use super::*;
use bevy::gizmos::{AppGizmoBuilder, GizmoAsset, GizmoHandles, config::GizmoConfigStore};
use feathrise_author_tools::ClipmapBrushSurface;
use std::any::TypeId;

#[derive(Default, Reflect, GizmoConfigGroup)]
struct TestBrushGizmos;
#[derive(Resource)]
struct Preview(BrushPreview);

#[derive(Default)]
struct Geometry {
    lines: Vec<Vec3>,
    strips: Vec<Vec3>,
    line_colors: Vec<LinearRgba>,
    strip_colors: Vec<LinearRgba>,
}

fn brush(shape: BrushShape) -> BrushPreview {
    BrushPreview {
        center: [2.0, 5.0, -3.0],
        rotation: Quat::IDENTITY.to_array(),
        radius: 3.0,
        strength: Some(0.25),
        shape,
        half_height: 1.5,
        flatten: false,
        spatial: false,
        color: [0.04, 0.2, 0.8, 0.45],
        clipmap: None,
    }
}

fn draw(preview: Res<Preview>, mut gizmos: Gizmos<TestBrushGizmos>) {
    draw_brush(&mut gizmos, &preview.0);
}

fn geometry(preview: BrushPreview) -> Geometry {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<GizmoAsset>>()
        .init_resource::<GizmoConfigStore>()
        .init_gizmo_group::<TestBrushGizmos>()
        .insert_resource(Preview(preview))
        .add_systems(Update, draw);
    app.update();
    let world = app.world();
    let assets = world.resource::<Assets<GizmoAsset>>();
    let Some(asset) = world
        .resource::<GizmoHandles>()
        .handles()
        .get(&TypeId::of::<TestBrushGizmos>())
        .and_then(Option::as_ref)
        .and_then(|handle| assets.get(handle))
    else {
        return Geometry::default();
    };
    let buffer = asset.buffer().buffer();
    Geometry {
        lines: buffer.list_positions.to_vec(),
        strips: buffer.strip_positions.to_vec(),
        line_colors: buffer.list_colors.to_vec(),
        strip_colors: buffer.strip_colors.to_vec(),
    }
}

#[test]
fn planar_brush_keeps_source_plane_strength_radius_and_linear_color() {
    let mut preview = brush(BrushShape::Planar);
    let rotation = Quat::from_euler(EulerRot::XYZ, 0.3, 0.6, -0.2);
    preview.rotation = rotation.to_array();
    let center = Vec3::from_array(preview.center) + rotation * Vec3::Y * 0.08;
    let normal = rotation * Vec3::Y;
    let expected_color = linear_color(preview.color);
    let result = geometry(preview.clone());
    assert_eq!(result.lines.len(), 4); // 原中心十字只有两条线。
    let mut outer = false;
    let mut inner = false;
    for point in result.strips.iter().filter(|point| point.is_finite()) {
        let offset = *point - center;
        assert!(offset.dot(normal).abs() < 0.0001);
        let distance = offset.length();
        outer |= (distance - preview.radius).abs() < 0.0001;
        inner |= (distance - preview.radius * 0.25).abs() < 0.0001;
        assert!(
            (distance - preview.radius).abs() < 0.0001
                || (distance - preview.radius * 0.25).abs() < 0.0001
        );
    }
    assert!(outer && inner);
    assert!(
        result
            .line_colors
            .iter()
            .all(|color| *color == expected_color)
    );
    assert!(
        result
            .strips
            .iter()
            .zip(&result.strip_colors)
            .filter(|(point, _)| point.is_finite())
            .all(|(_, color)| *color == expected_color)
    );
    preview.strength = None; // 点位笔刷没有密度内圈。
    assert!(
        geometry(preview.clone())
            .strips
            .iter()
            .filter(|point| point.is_finite())
            .all(|point| (point.distance(center) - preview.radius).abs() < 0.0001)
    );
}

#[test]
fn volume_shapes_keep_original_extents_and_flatten_markers() {
    let center = Vec3::from_array(brush(BrushShape::Box).center);
    let box_preview = brush(BrushShape::Box);
    let edges = geometry(box_preview.clone());
    assert_eq!(edges.lines.len(), 24);
    assert!(edges.strips.is_empty());
    for point in &edges.lines {
        let extent = (*point - center).abs();
        assert!((extent - Vec3::new(3.0, 1.5, 3.0)).length() < 0.0001);
    }
    let cylinder = geometry(brush(BrushShape::Cylinder));
    assert_eq!(cylinder.lines.len(), 8);
    assert!(
        cylinder
            .strips
            .iter()
            .filter(|point| point.is_finite())
            .all(|point| ((point.y - center.y).abs() - 1.5).abs() < 0.0001)
    );
    let sphere = geometry(brush(BrushShape::Sphere));
    assert!(
        sphere
            .strips
            .iter()
            .filter(|point| point.is_finite())
            .all(|point| (point.distance(center) - 3.0).abs() < 0.0001)
    );
    let mut flatten = box_preview;
    flatten.flatten = true;
    let flattened = geometry(flatten);
    assert_eq!(flattened.lines.len(), 28);
    assert!(flattened.strips.is_empty()); // 方刷整平沿用十字，不额外叠球圈。
    assert!(
        flattened.line_colors[24..]
            .iter()
            .all(|color| *color == Color::srgb(1.0, 0.55, 0.15).to_linear())
    );
    assert!(
        flattened.lines[24..]
            .iter()
            .all(|point| point.y == center.y)
    );
}

#[test]
fn clipmap_uses_real_gpu_identity_and_never_creates_a_flat_fallback() {
    let mut preview = brush(BrushShape::Planar);
    let local_revision = [11, 22, 33];
    let local_missing_pages = [UVec4::new(1, 2, 3, 4); 16];
    preview.clipmap = Some(ClipmapBrushSurface {
        map_id: "ground".into(),
        terrain_id: None,
        height_path: "maps/ground/terrain/height_tiles.map_height_manifest.json".into(),
        authoring_height_revision: 9,
    });
    let result = geometry(preview.clone());
    assert!(result.lines.is_empty() && result.strips.is_empty());
    let overlay = clipmap_brush(&preview, local_revision, local_missing_pages).expect("已核验并绑定到本地的GPU参数");
    assert_eq!(overlay.map_id, MapId("ground".into()));
    assert_eq!(overlay.revision, local_revision);
    assert_eq!(overlay.missing_pages, local_missing_pages);
    assert_eq!(overlay.center, Vec2::new(2.0, -3.0));
    assert_eq!(overlay.strength, 0.25);
    assert_eq!(overlay.color, linear_color(preview.color));
}
