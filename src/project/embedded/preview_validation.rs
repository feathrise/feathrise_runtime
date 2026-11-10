//! 实时手势的独立真实GPU截图；每个预览序号独立存证，不复用整图修订截图或ACK作为像素证据。

use super::{EmbeddedProjectState, resources::{Runtime, Viewport}};
use bevy::{prelude::*, render::view::screenshot::ScreenshotCaptured,
    tasks::{IoTaskPool, Task, futures::check_ready}};
use feathrise_editor_protocol::{Mode, PreviewKind};
use feathrise_vegetation::blade::{GpuBladeGpuReadbackStats, GpuBladeRenderStats};
use std::path::PathBuf;
use super::validation_capture as capture_gate;
use super::validation::tool_validation::{self, ToolRenderEvidence};

#[path = "preview_diagnostics.rs"]
mod diagnostics;

#[derive(Clone, PartialEq)]
struct Identity { kind: PreviewKind, sequence: u64, generation: u64, viewport: Viewport, tools: ToolRenderEvidence }
#[derive(Resource)]
struct Evidence {
    directory: PathBuf,
    done: [u64; 4],
    observed: Option<Identity>,
    stable: u8,
    sample_baseline: u64,
    pending: Option<(Identity, serde_json::Value)>,
    writer: Option<Task<Result<(), String>>>,
    diagnostics: diagnostics::State,
}

pub(super) fn install(app: &mut App, directory: PathBuf) {
    app.insert_resource(Evidence { directory, done: [0; 4], observed: None, stable: 0,
        sample_baseline: 0, pending: None, writer: None, diagnostics: default() })
        .add_systems(Last, capture.after(super::window::synchronize).after(tool_validation::collect));
}

pub(super) fn install_render(app: &mut App) {
    if app.world().contains_resource::<Evidence>() { diagnostics::install_render(app); }
}

fn capture(world: &mut World) {
    let Some(mut evidence) = world.remove_resource::<Evidence>() else { return; };
    // 即使首个域被就绪门挡住，也独立留下真实等待原因；不拿状态记录代替截图或GPU成功。
    diagnostics::record(world, &mut evidence);
    if let Some(task) = evidence.writer.as_mut() {
        if let Some(result) = check_ready(task) {
            evidence.writer = None;
            if let Err(error) = result { error!("实时预览取证失败：{error}"); evidence.done = [0; 4]; }
        }
        world.insert_resource(evidence); return;
    }
    let runtime = world.non_send::<Runtime>();
    let state = world.resource::<EmbeddedProjectState>();
    if state.mode != Mode::Editing || !runtime.ready || runtime.pending.is_some() || evidence.pending.is_some() {
        world.insert_resource(evidence); return;
    }
    let applied = runtime.previews.applied;
    let target = (0..super::preview::KINDS.len()).find(|index| applied[*index] > evidence.done[*index]);
    let Some(index) = target else { world.insert_resource(evidence); return; };
    let kind = super::preview::KINDS[index];
    let identity = Identity { kind, sequence: applied[index], generation: runtime.transport.identity.generation,
        viewport: runtime.viewport.clone(), tools: world.resource::<ToolRenderEvidence>().clone() };
    let gpu = world.get_resource::<GpuBladeGpuReadbackStats>().copied().unwrap_or_default();
    let render = world.get_resource::<GpuBladeRenderStats>().copied().unwrap_or_default();
    if evidence.observed.as_ref() != Some(&identity) {
        evidence.observed = Some(identity.clone()); evidence.stable = 0; evidence.sample_baseline = gpu.sample_sequence;
    }
    evidence.stable = evidence.stable.saturating_add(1);
    if evidence.stable < 8 || !identity.tools.ready() || !super::systems::scene_ready(world)
        || (kind == PreviewKind::Vegetation && (gpu.sample_sequence <= evidence.sample_baseline
            || render.density_waiting_windows > 0 || render.height_waiting_windows > 0
            || render.compute_pipeline_waiting_stages > 0)) {
        world.insert_resource(evidence); return;
    }
    let content = runtime.previews.evidence(world, kind);
    if kind == PreviewKind::Vegetation {
        // 按真实DynamicBladeDraft发布版本与Compute回读对齐；纯ACK或旧GPU样本不得拍成成功证据。
        if !vegetation_source_ready(&content, gpu.source_revision) { world.insert_resource(evidence); return; }
    }
    let report = serde_json::json!({"pid":std::process::id(),"world_id":state.world_id,
        "scene_generation":identity.generation,"revision":state.revision,"preview_sequence":identity.sequence,
        "kind":super::preview::kind_name(kind),"mode":"editing","frame":runtime.frames,"content":content,
        "gpu":{"source_revision":gpu.source_revision,"sample_sequence":gpu.sample_sequence,
            "sample_baseline":evidence.sample_baseline,"sampled_windows":gpu.sampled_windows,
            "rendered_instances":gpu.rendered_instances,"density_survivors":gpu.density_survivors,
            "surface_survivors":gpu.surface_survivors,"scene_errors":gpu.scene_errors,
            "drawn_draws":render.drawn_draws,"input_ready_windows":render.input_ready_windows,
            "density_waiting_windows":render.density_waiting_windows},
        "source":"real-project-preview"});
    let Some(request)=capture_gate::request(world,capture_gate::Owner::Preview) else {
        world.insert_resource(evidence);return;
    };
    evidence.pending = Some((identity, report));
    world.insert_resource(evidence);
    world.entity_mut(request).observe(captured);
}

fn vegetation_source_ready(content: &serde_json::Value, source_revision: u64) -> bool {
    content["canvases"].as_array().is_some_and(|canvases| !canvases.is_empty() && canvases.iter().all(|canvas|
        canvas["source_revision"].as_u64().is_some()
        && canvas["source_revision"] == canvas["adopted_source_revision"]
        && canvas["draft_revision"].as_u64() == Some(source_revision)
        && source_revision > 0))
}

fn captured(event: On<ScreenshotCaptured>, mut evidence: ResMut<Evidence>, runtime: NonSend<Runtime>, state: Res<EmbeddedProjectState>, tools: Res<ToolRenderEvidence>,mut slot:ResMut<capture_gate::Slot>) {
    if !slot.release(capture_gate::Owner::Preview,event.entity) { return; }
    let Some((identity, mut report)) = evidence.pending.take() else { return; };
    let index = super::preview::index(identity.kind);
    if runtime.previews.applied[index] != identity.sequence || runtime.transport.identity.generation != identity.generation
        || runtime.viewport != identity.viewport || state.mode != Mode::Editing || *tools != identity.tools
        || event.image.width() != identity.viewport.width || event.image.height() != identity.viewport.height {
        return; // 迟到GPU像素不得匹配新的手势/窗口。
    }
    evidence.done[index] = identity.sequence;
    let directory = evidence.directory.clone();
    let image = event.image.clone();
    evidence.writer = Some(IoTaskPool::get().spawn(async move {
        use sha2::{Digest, Sha256};
        let base = format!("{}-preview-{}-{}", std::process::id(), super::preview::kind_name(identity.kind), identity.sequence);
        let pixels = image.data.as_ref().ok_or("实时预览GPU截图缺少像素")?;
        report["capture"] = serde_json::json!({"source":"Bevy ScreenshotCaptured","width":image.width(),"height":image.height(),
            "sha256":Sha256::digest(pixels).iter().map(|byte|format!("{byte:02x}")).collect::<String>()});
        report["screenshot"] = serde_json::json!(format!("{base}.png"));
        let temporary = directory.join(format!("{base}.pending.png"));
        let pixels = image.try_into_dynamic().map_err(|error|error.to_string())?.to_rgba8();
        // 连续笔划与原工具共用同一次GPU回读，防止刷草结果和刷圈证据来自不同帧。
        report["author_tools"] = identity.tools.inspect_pixels(pixels.width(),pixels.height(),|x,y|pixels.get_pixel(x,y).0);
        report["author_tools"]["rgba_sha256"] = serde_json::json!(Sha256::digest(pixels.as_raw())
            .iter().map(|byte|format!("{byte:02x}")).collect::<String>());
        pixels.save(&temporary).map_err(|error|error.to_string())?;
        std::fs::rename(temporary, directory.join(format!("{base}.png"))).map_err(|error|error.to_string())?;
        let temporary = directory.join(format!("{base}.pending.json"));
        std::fs::write(&temporary, serde_json::to_vec_pretty(&report).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
        std::fs::rename(temporary, directory.join(format!("{base}.json"))).map_err(|error|error.to_string())
    }));
}
