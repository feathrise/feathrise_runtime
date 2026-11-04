//! 显式验收从正式Gizmo资产取实际几何；消息中的工具计数不能代替真实绘制和GPU像素。

use super::EmbeddedProjectState;
use crate::project::embedded::{resources::Runtime, tools};
use bevy::{
    gizmos::{GizmoAsset, GizmoHandles, GizmoMeshSystems, gizmos::GizmoBufferView},
    prelude::*,
};
use feathrise_editor_protocol::AuthorTool;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::any::TypeId;

const MAX_VERTICES: usize = 32 * 1024;
const MAX_SAMPLE_SEGMENTS: usize = 32;

#[derive(Clone, Default, PartialEq, Resource)]
pub(in crate::project::embedded) struct ToolRenderEvidence {
    revision: Option<u64>,
    mode: String,
    pending: bool,
    viewport_visible: bool,
    visible: bool,
    items: usize,
    brushes: usize,
    transforms: usize,
    selections: usize,
    semantic_content: [u8; 32],
    geometry_content: [u8; 32],
    ordinary_segments: usize,
    spatial_segments: usize,
    clipmap_input: Value,
    clipmap_render: Option<feathrise_scene::ClipmapBrushOverlayEvidence>,
    clipmap_matches_preview: bool,
    clipmap_samples: Vec<Sample>,
    samples: Vec<Sample>,
    error: Option<String>,
}

#[derive(Clone, PartialEq, Serialize)]
struct Sample {
    world: [f32; 3],
    position: [f32; 2],
    srgb: [f32; 3],
}

struct Segment {
    start: Vec3,
    end: Vec3,
    start_color: LinearRgba,
    end_color: LinearRgba,
}

pub(in crate::project::embedded) fn install(app: &mut App) {
    app.init_resource::<ToolRenderEvidence>().add_systems(
        Last,
        collect
            .after(GizmoMeshSystems)
            .after(crate::project::embedded::window::synchronize),
    );
}

pub(in crate::project::embedded) fn collect(world: &mut World) {
    let mut evidence = ToolRenderEvidence::default();
    let mut clipmap_preview = None;
    let (width, height) = {
        let runtime = world.non_send::<Runtime>();
        let state = world.resource::<EmbeddedProjectState>();
        evidence.revision = runtime.author_tools.as_ref().map(|(revision, _)| *revision);
        evidence.mode = format!("{:?}", state.mode);
        evidence.pending = runtime.pending.is_some();
        evidence.viewport_visible = runtime.viewport.visible;
        if let Some(preview) = tools::visible(state, runtime) {
            evidence.visible = true;
            evidence.items = preview.items.len();
            match serde_json::to_vec(preview) {
                Ok(bytes) => evidence.semantic_content = Sha256::digest(bytes).into(),
                Err(error) => evidence.error = Some(error.to_string()),
            }
            for item in &preview.items {
                match item {
                    AuthorTool::Brush(brush) => {
                        evidence.brushes += 1;
                        if brush.clipmap.is_some() {
                            clipmap_preview = Some(brush.clone());
                        }
                    }
                    AuthorTool::Transform(_) => evidence.transforms += 1,
                    AuthorTool::Selection(_) => evidence.selections += 1,
                }
            }
        }
        (runtime.viewport.width, runtime.viewport.height)
    };
    // 这是实际发布给地形材质消费者的小参数；真正材质采用证据由场景库独立提供。
    if let Some(brush) = world
        .get_resource::<feathrise_scene::DynamicClipmapVegetationOverlay>()
        .and_then(|overlay| overlay.brush.as_ref())
    {
        evidence.clipmap_input = json!({"map":brush.map_id.as_str(),"revision":brush.revision,
            "missing_pages":brush.missing_pages.map(|page|page.to_array()),
            "center":brush.center.to_array(),"radius":brush.radius,"strength":brush.strength,
            "color":brush.color.to_f32_array()});
    }
    evidence.clipmap_render = feathrise_scene::clipmap_brush_overlay_evidence(world);
    if let (Some(preview), Some(actual)) = (&clipmap_preview, &evidence.clipmap_render) {
        evidence.clipmap_matches_preview = actual.bound
            && actual.brush == [preview.center[0], preview.center[2], preview.radius, preview.strength.unwrap_or(0.0)]
            && actual.color == preview.color
            // 缺页信用必须与接收World真实发布的本地输入一致，不能比较另一World的作者页表。
            && evidence.clipmap_input["missing_pages"] == json!(actual.missing_pages);
        if actual.visible && evidence.clipmap_matches_preview {
            // 自有平坦夹具用原命中高度预测刷圈投影；实际绑定与最终颜色仍由材质和PNG独立证明。
            // 此处不生成Gizmo，不把预测点冒充GPU地形顶点或真实高度采样。
            let color = LinearRgba::from_f32_array(actual.color);
            let center = Vec3::new(actual.brush[0], preview.center[1], actual.brush[1]);
            let points = (0..32).map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / 32.0;
                (
                    center + Vec3::new(angle.cos(), 0.0, angle.sin()) * actual.brush[2],
                    color,
                )
            });
            evidence.clipmap_samples = project_points(world, points, width, height);
        }
    }
    let mut segments = Vec::new();
    let mut digest = Sha256::new();
    let result = (|| {
        let handles = world
            .get_resource::<GizmoHandles>()
            .ok_or("正式Gizmo句柄尚未建立")?;
        let assets = world
            .get_resource::<Assets<GizmoAsset>>()
            .ok_or("正式Gizmo资产尚未建立")?;
        let groups = [
            TypeId::of::<tools::AuthorToolsGizmos>(),
            TypeId::of::<tools::AuthorSpatialToolsGizmos>(),
        ];
        let mut vertices = 0;
        for (index, group) in groups.into_iter().enumerate() {
            digest.update([index as u8]);
            let before = segments.len();
            if let Some(asset) = handles
                .handles()
                .get(&group)
                .and_then(Option::as_ref)
                .and_then(|handle| assets.get(handle))
            {
                append(
                    asset.buffer().buffer(),
                    &mut segments,
                    &mut digest,
                    &mut vertices,
                )?;
            }
            if index == 0 {
                evidence.ordinary_segments = segments.len() - before;
            } else {
                evidence.spatial_segments = segments.len() - before;
            }
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        evidence.error = Some(error);
    }
    evidence.geometry_content = digest.finalize().into();
    if !segments.is_empty() {
        evidence.samples = project(world, &segments, width, height);
    }
    world.insert_resource(evidence);
}

fn append(
    view: GizmoBufferView<'_>,
    segments: &mut Vec<Segment>,
    digest: &mut Sha256,
    vertices: &mut usize,
) -> Result<(), String> {
    *vertices = vertices
        .saturating_add(view.list_positions.len())
        .saturating_add(view.strip_positions.len());
    if *vertices > MAX_VERTICES {
        return Err("工具实际Gizmo资产超过固定取证预算".into());
    }
    if view.list_positions.len() != view.list_colors.len()
        || view.strip_positions.len() != view.strip_colors.len()
        || view.list_positions.len() % 2 != 0
    {
        return Err("工具实际Gizmo资产坐标与颜色不完整".into());
    }
    for (points, colors) in view
        .list_positions
        .chunks_exact(2)
        .zip(view.list_colors.chunks_exact(2))
    {
        push(points, colors, segments, digest)?;
    }
    for (points, colors) in view
        .strip_positions
        .windows(2)
        .zip(view.strip_colors.windows(2))
    {
        if points.iter().any(|point| point.is_nan()) {
            continue;
        } // Bevy的真实线带分隔符不产生线段。
        push(points, colors, segments, digest)?;
    }
    Ok(())
}

fn push(
    points: &[Vec3],
    colors: &[LinearRgba],
    segments: &mut Vec<Segment>,
    digest: &mut Sha256,
) -> Result<(), String> {
    let colors_array = [colors[0].to_f32_array(), colors[1].to_f32_array()];
    if points.iter().any(|point| !point.is_finite())
        || colors_array
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
    {
        return Err("工具实际Gizmo资产包含非法值".into());
    }
    for value in points[0]
        .to_array()
        .into_iter()
        .chain(points[1].to_array())
        .chain(colors_array.into_iter().flatten())
    {
        digest.update(value.to_bits().to_le_bytes());
    }
    segments.push(Segment {
        start: points[0],
        end: points[1],
        start_color: colors[0],
        end_color: colors[1],
    });
    Ok(())
}

fn project(world: &mut World, segments: &[Segment], width: u32, height: u32) -> Vec<Sample> {
    let stride = segments.len().div_ceil(MAX_SAMPLE_SEGMENTS).max(1);
    let points = segments
        .iter()
        .step_by(stride)
        .take(MAX_SAMPLE_SEGMENTS)
        .flat_map(|segment| {
            [0.25, 0.5, 0.75].map(|fraction| {
                let a = Vec4::from_array(segment.start_color.to_f32_array());
                let b = Vec4::from_array(segment.end_color.to_f32_array());
                (
                    segment.start.lerp(segment.end, fraction),
                    LinearRgba::from_f32_array(a.lerp(b, fraction).to_array()),
                )
            })
        });
    project_points(world, points, width, height)
}

fn project_points(
    world: &mut World,
    points: impl IntoIterator<Item = (Vec3, LinearRgba)>,
    width: u32,
    height: u32,
) -> Vec<Sample> {
    let Ok(entity) = world
        .query_filtered::<Entity, With<feathrise_scene_core::view::PrimarySceneView>>()
        .single(world)
    else {
        return Vec::new();
    };
    let Some(camera) = world.get::<Camera>(entity) else {
        return Vec::new();
    };
    let Some(transform) = world.get::<GlobalTransform>(entity) else {
        return Vec::new();
    };
    let Some(scale) = camera.target_scaling_factor() else {
        return Vec::new();
    };
    let mut samples = Vec::new();
    for (world, color) in points {
        let Ok(position) = camera
            .world_to_viewport(transform, world)
            .map(|point| point * scale)
        else {
            continue;
        };
        if !position.is_finite()
            || position.x < 0.0
            || position.y < 0.0
            || position.x >= width as f32
            || position.y >= height as f32
        {
            continue;
        }
        let [r, g, b, alpha] = color.to_f32_array();
        let display = Color::linear_rgba(r, g, b, alpha).to_srgba();
        samples.push(Sample {
            world: world.to_array(),
            position: position.to_array(),
            srgb: [display.red, display.green, display.blue],
        });
    }
    samples
}

impl ToolRenderEvidence {
    pub(in crate::project::embedded) fn ready(&self) -> bool {
        self.error.is_none()
    }

    pub(in crate::project::embedded) fn update_digest(&self, digest: &mut Sha256) {
        digest.update(self.semantic_content);
        digest.update(self.geometry_content);
        digest.update(self.ordinary_segments.to_le_bytes());
        digest.update(self.spatial_segments.to_le_bytes());
        if let Ok(bytes) = serde_json::to_vec(&self.clipmap_input) {
            digest.update(bytes);
        }
        if let Ok(bytes) = serde_json::to_vec(&self.clipmap_render) {
            digest.update(bytes);
        }
        digest.update([u8::from(self.clipmap_matches_preview)]);
    }

    pub(in crate::project::embedded) fn report(&self) -> Value {
        json!({"source":"actual-project-GizmoAssets-after-GizmoMeshSystems", "revision":self.revision,
            "mode":self.mode,"pending":self.pending,"viewport_visible":self.viewport_visible,"visible":self.visible,
            "semantic_items":self.items,"brushes":self.brushes,"transforms":self.transforms,"selections":self.selections,
            "semantic_sha256":hex(&self.semantic_content),"geometry_sha256":hex(&self.geometry_content),
            "visible_segments":self.ordinary_segments+self.spatial_segments,"ordinary_segments":self.ordinary_segments,
            "spatial_segments":self.spatial_segments,"clipmap_input":self.clipmap_input,
            "clipmap_render":self.clipmap_render,"clipmap_matches_preview":self.clipmap_matches_preview,
            "clipmap_samples":self.clipmap_samples,
            "samples":self.samples,"error":self.error})
    }

    /// 对实际Gizmo线投影位置取邻域像素；此结果还须由宿主独立重读PNG复核。
    pub(in crate::project::embedded) fn inspect_pixels(
        &self,
        width: u32,
        height: u32,
        pixel: impl Fn(u32, u32) -> [u8; 4],
    ) -> Value {
        let mut report = self.report();
        for (field, result, sources) in [
            ("samples", "matched_samples", &self.samples),
            (
                "clipmap_samples",
                "clipmap_matched_samples",
                &self.clipmap_samples,
            ),
        ] {
            let mut matched = 0;
            if let Some(samples) = report[field].as_array_mut() {
                for (sample, source) in samples.iter_mut().zip(sources) {
                    let x = source.position[0].round() as i32;
                    let y = source.position[1].round() as i32;
                    let mut hit = false;
                    for dy in -3..=3 {
                        for dx in -3..=3 {
                            let (x, y) = (x + dx, y + dy);
                            if x >= 0 && y >= 0 && x < width as i32 && y < height as i32 {
                                hit |= matches_color(source.srgb, pixel(x as u32, y as u32));
                            }
                        }
                    }
                    sample["matched"] = json!(hit);
                    matched += usize::from(hit);
                }
            }
            report[result] = json!(matched);
        }
        report["sample_radius"] = json!(3);
        report
    }
}

fn matches_color(expected: [f32; 3], pixel: [u8; 4]) -> bool {
    let actual = [
        f32::from(pixel[0]) / 255.0,
        f32::from(pixel[1]) / 255.0,
        f32::from(pixel[2]) / 255.0,
    ];
    let (minimum, maximum) = actual
        .iter()
        .fold((1.0_f32, 0.0_f32), |(low, high), value| {
            (low.min(*value), high.max(*value))
        });
    let expected_chroma = expected.iter().copied().fold(f32::NEG_INFINITY, f32::max)
        - expected.iter().copied().fold(f32::INFINITY, f32::min);
    expected
        .iter()
        .zip(&actual)
        .all(|(expected, actual)| (expected - actual).abs() <= 0.42)
        && (expected_chroma < 0.15 || maximum - minimum > 0.12)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_buffer(points: &[Vec3], colors: &[LinearRgba]) -> (usize, [u8; 32]) {
        // 与真实GizmoBufferView一致借用Vec；切片仅作为测试数据的简洁输入。
        let points = points.to_vec();
        let colors = colors.to_vec();
        let empty_positions = Vec::new();
        let empty_colors = Vec::new();
        let mut digest = Sha256::new();
        let mut segments = Vec::new();
        append(
            GizmoBufferView {
                list_positions: &points,
                list_colors: &colors,
                strip_positions: &empty_positions,
                strip_colors: &empty_colors,
            },
            &mut segments,
            &mut digest,
            &mut 0,
        )
        .unwrap();
        (segments.len(), digest.finalize().into())
    }

    #[test]
    fn actual_geometry_identity_changes_after_move_clear_and_same_count_color_change() {
        let colors = [LinearRgba::RED; 2];
        let original = read_buffer(&[Vec3::ZERO, Vec3::X], &colors);
        assert_eq!(original.0, 1);
        assert_ne!(
            original,
            read_buffer(&[Vec3::Y, Vec3::Y + Vec3::X], &colors)
        );
        assert_ne!(
            original,
            read_buffer(&[Vec3::ZERO, Vec3::X], &[LinearRgba::GREEN; 2])
        );
        assert_ne!(original, read_buffer(&[], &[]));
        let mut selected = ToolRenderEvidence::default();
        selected.ordinary_segments = original.0;
        selected.geometry_content = original.1;
        let cleared = ToolRenderEvidence::default();
        assert!(selected != cleared, "清选择不能复用旧实际几何的截图身份");
        let mut selected_hash = Sha256::new();
        selected.update_digest(&mut selected_hash);
        let mut clear_hash = Sha256::new();
        cleared.update_digest(&mut clear_hash);
        assert_ne!(
            selected_hash.finalize(),
            clear_hash.finalize(),
            "不可变截图路径必须包含实际工具内容"
        );
    }

    #[test]
    fn buffers_reject_invalid_values_and_respect_nan_strip_separators() {
        let positions = vec![
            Vec3::ZERO,
            Vec3::X,
            Vec3::splat(f32::NAN),
            Vec3::Y,
            Vec3::Y + Vec3::X,
        ];
        let colors = vec![LinearRgba::GREEN; 5];
        let empty_positions = Vec::new();
        let empty_colors = Vec::new();
        let mut segments = Vec::new();
        append(
            GizmoBufferView {
                list_positions: &empty_positions,
                list_colors: &empty_colors,
                strip_positions: &positions,
                strip_colors: &colors,
            },
            &mut segments,
            &mut Sha256::new(),
            &mut 0,
        )
        .unwrap();
        assert_eq!(segments.len(), 2, "不同线带不能连接成不存在的工具几何");
        let invalid_positions = vec![Vec3::ZERO];
        let invalid_colors = vec![LinearRgba::GREEN];
        assert!(
            append(
                GizmoBufferView {
                    list_positions: &invalid_positions,
                    list_colors: &invalid_colors,
                    strip_positions: &empty_positions,
                    strip_colors: &empty_colors
                },
                &mut Vec::new(),
                &mut Sha256::new(),
                &mut 0
            )
            .is_err()
        );
        assert!(
            !matches_color([1.0, 0.7, 0.1], [100, 100, 100, 255]),
            "灰背景不能冒充金色选框"
        );
        assert!(matches_color([1.0, 0.7, 0.1], [255, 178, 25, 255]));
    }
}
