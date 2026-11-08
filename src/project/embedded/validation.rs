//! 显式 validation 才编译的真实工程子窗口取证；使用 GPU 回读，不把宿主截图当成子窗口画面。

use super::{
    EmbeddedProjectState,
    resources::{Runtime, Viewport},
};
use bevy::{
    prelude::*,
    render::view::screenshot::ScreenshotCaptured,
    tasks::{IoTaskPool, Task, futures::check_ready},
};
use feathrise_editor_protocol::Mode;
use std::{path::PathBuf, time::{Duration, Instant}};

use super::validation_capture as capture_gate;

#[path = "tool_validation.rs"]
pub(super) mod tool_validation;
use tool_validation::ToolRenderEvidence;

#[derive(Resource)]
struct Evidence {
    directory: PathBuf,
    observed: Option<CaptureIdentity>,
    captured: Option<CaptureIdentity>,
    stable: u8,
    pending: Option<(String, serde_json::Value, CaptureIdentity)>,
    writer: Option<Task<Result<(), String>>>,
    render_ticket: u64,
    next_render_diagnostic: Option<Instant>,
}

/// 同一作者修订也可能发生尺寸、控件留孔或焦点变化，不能沿用旧原生窗口取证。
#[derive(Clone, PartialEq)]
struct CaptureIdentity {
    revision: u64,
    // 函数体补丁不改PID/World/地图修订，必须另用真实采用代次拒绝旧GPU回调。
    patch_epoch: u64,
    mode: Mode,
    viewport: Viewport,
    // 请求焦点不等于原生输入已启用；实际许可变化也必须使旧截图和迟到GPU回读失效。
    native_window_enabled: Option<bool>,
    author_view: Option<feathrise_editor_protocol::AuthorViewState>,
    camera: Option<feathrise_editor_protocol::CameraState>,
    overlay: OverlayIdentity,
    tools: ToolRenderEvidence,
    // 工具栏与领域场景共同约束GPU回读身份，任一状态改变都不得沿用旧截图。
    toolbar: super::preview_toolbar::RuntimeEvidence,
    extension:Option<super::AuthorDomainExtensionScene>,
}

/// 选框与真实边重合时会完全覆盖黑线；截图身份必须包含当前实际可见的原工具线，不能复用旧高亮图。
#[derive(Clone, Default, PartialEq)]
struct OverlayIdentity {
    revision: Option<u64>,
    visible_lines: usize,
    content: [u8;32],
}
impl OverlayIdentity {
    fn current(state: &EmbeddedProjectState, pending: bool,
        overlay: Option<(u64,&[feathrise_editor_protocol::OverlayLine])>) -> Self {
        use sha2::{Digest,Sha256};
        let Some((revision,lines)) = overlay else { return Self::default(); };
        let mut identity = Self { revision:Some(revision),..default() };
        // 与正式overlay::draw共用可见语义；Playing/整图切换/旧修订不会显示缓存的作者线。
        if state.mode != Mode::Editing || state.revision==0 || pending || revision!=state.revision { return identity; }
        identity.visible_lines = lines.len();
        let mut digest = Sha256::new();
        // 协议已限制至1024条固定小输入，只在显式验收中记录；无需序列化或复制整份线数组。
        for line in lines {
            for value in line.start.iter().chain(&line.end).chain(&line.start_color).chain(&line.end_color) {
                digest.update(value.to_bits().to_le_bytes());
            }
        }
        identity.content = digest.finalize().into();
        identity
    }
    fn report(&self) -> serde_json::Value {
        serde_json::json!({"revision":self.revision,"visible_lines":self.visible_lines,
            "content_sha256":self.content.iter().map(|byte|format!("{byte:02x}")).collect::<String>()})
    }
}

impl CaptureIdentity {
    fn current(state: &EmbeddedProjectState, runtime: &Runtime, tools: &ToolRenderEvidence,
        toolbar: super::preview_toolbar::RuntimeEvidence, extension:Option<&super::AuthorDomainExtensionScene>,
        patch_epoch: u64) -> Self {
        Self {
            revision: state.revision,
            patch_epoch,
            mode: state.mode,
            viewport: {
                #[cfg(windows)]
                { super::window::capture_viewport(runtime, state.mode) }
                #[cfg(not(windows))]
                { runtime.viewport.clone() }
            },
            native_window_enabled: {
                #[cfg(windows)]
                { runtime.hwnd.map(|hwnd| unsafe {
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd as *mut _) != 0
                }) }
                #[cfg(not(windows))]
                { None }
            },
            author_view: runtime.author_view,
            camera: runtime.camera,
            overlay: OverlayIdentity::current(state,runtime.pending.is_some(),
                runtime.overlay.as_ref().map(|(revision,lines)|(*revision,lines.as_slice()))),
            tools: tools.clone(),
            toolbar,
            extension:extension.cloned(),
        }
    }

    fn native_input_ready(&self) -> bool {
        self.native_window_enabled == Some(self.mode == Mode::Playing)
            && (self.mode != Mode::Playing || (self.viewport.visible && self.viewport.focused))
    }

    fn input_evidence(&self) -> serde_json::Value {
        serde_json::json!({"requested_visible":self.viewport.visible,
            "requested_focused":self.viewport.focused,"native_enabled":self.native_window_enabled})
    }
}

pub(super) fn install(app: &mut App, project: &crate::project::ProjectContext) -> Result<(), String> {
    let Some(directory) = std::env::var_os("NEW_EDITOR_EMBEDDED_GAME_SMOKE_DIR").map(PathBuf::from)
    else {
        return Ok(());
    };
    if !directory.is_absolute() {
        return Err("嵌入运行验收输出必须是绝对夹具目录".into());
    }
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    if directory == project.root
        || project
            .asset_roots
            .iter()
            .any(|root| directory.starts_with(root))
    {
        return Err("嵌入运行验收输出不能覆盖工程根或作者资产".into());
    }
    super::preview_validation::install(app, directory.clone());
    tool_validation::install(app);
    app.add_plugins(capture_gate::ReadinessPlugin)
    .init_resource::<capture_gate::Slot>().insert_resource(Evidence {
        directory,
        observed: None,
        captured: None,
        stable: 0,
        pending: None,
        writer: None,
        render_ticket: 0,
        next_render_diagnostic: None,
    })
    .add_systems(Last, capture_ready.after(tool_validation::collect));
    Ok(())
}

fn capture_ready(world: &mut World) {
    let Some(mut evidence) = world.remove_resource::<Evidence>() else {
        return;
    };
    if let Some(task) = evidence.writer.as_mut() {
        if let Some(result) = check_ready(task) {
            evidence.writer = None;
            if let Err(error) = result {
                error!("嵌入运行截图失败：{error}");
                evidence.captured = None;
            }
        }
        world.insert_resource(evidence);
        return;
    }
    let state = *world.resource::<EmbeddedProjectState>();
    #[cfg(feature = "function_hotpatch")]
    let patch_epoch = world.get_resource::<super::FunctionHotpatchState>().map_or(0, |state| state.epoch());
    #[cfg(not(feature = "function_hotpatch"))]
    let patch_epoch = 0;
    let toolbar = super::preview_toolbar::runtime_evidence(world);
    let runtime = world.non_send::<Runtime>();
    let identity = CaptureIdentity::current(&state, runtime, world.resource::<ToolRenderEvidence>(), toolbar,
        world.get_resource::<super::AuthorDomainExtensionScene>(), patch_epoch);
    if evidence.observed.as_ref() != Some(&identity) {
        evidence.observed = Some(identity.clone());
        evidence.captured = None;
        evidence.stable = 0;
        evidence.render_ticket = evidence.render_ticket.wrapping_add(1).max(1);
        evidence.next_render_diagnostic = None;
    }
    if evidence.captured.as_ref() == Some(&identity)
        || evidence.pending.is_some()
        || !runtime.ready
        || runtime.native_handoff.is_some()
        || runtime.native_result.is_some()
        || !runtime.sources_ready
        || !identity.tools.ready()
        || !super::systems::scene_ready(world)
    {
        world.insert_resource(evidence);
        return;
    }
    // 后台首张Playing图可能早于实际输入许可；此时仅保留有界等待诊断，不能把禁用图冻结为已捕获。
    if !identity.native_input_ready() {
        record_capture_wait(&mut evidence, &state, runtime, "native-input", serde_json::Value::Null);
        world.insert_resource(evidence);
        return;
    }
    evidence.stable = evidence.stable.saturating_add(1);
    if evidence.stable < 8 {
        world.insert_resource(evidence);
        return;
    }
    let frame = runtime.frames;
    let ticks = runtime.ticks;
    #[cfg(windows)]
    let window_region = super::window::evidence(runtime, state.mode)
        .unwrap_or_else(|error| serde_json::json!({"verified":false,"error":error}));
    #[cfg(not(windows))]
    let window_region =
        serde_json::json!({"verified":false,"error":"当前平台不支持原生子窗口区域验收"});
    let mode = match state.mode {
        Mode::Editing => "editing",
        Mode::Playing => "playing",
    };
    // CPU身份稳定只解决消息交接，不能证明第一次face-wire变体已被GPU创建。
    // 单槽RenderApp真实回执还核对当前相机、附件采样数、物理尺寸和提取后的四球/网格参数。
    let (render_ready, render_readiness) = capture_gate::poll_readiness(world, evidence.render_ticket);
    if !render_ready {
        record_capture_wait(&mut evidence, &state, world.non_send::<Runtime>(), "render-readiness", render_readiness);
        world.insert_resource(evidence);
        return;
    }
    let base = format!("{}-{}-{mode}", std::process::id(), state.revision);
    let placements = world
        .query::<(
            &feathrise_scene::MapPlacementInstance,
            &GlobalTransform,
        )>()
        .iter(world)
        .filter(|(instance, _)| instance.is_active())
        .take(128)
        .map(|(instance, transform)| {
            serde_json::json!({ "id":instance.id(), "map":instance.map_id().as_str(),
            "position":transform.translation().to_array(), "asset_target":instance.asset_target() })
        })
        .collect::<Vec<_>>();
    let author_view = super::author_view::evidence(world, identity.author_view);
    let preview_toolbar = super::preview_toolbar::evidence(world);
    let wire_geometry = capture_gate::wire_geometry::project(world);
    let report = serde_json::json!({"pid":std::process::id(), "world_id":state.world_id,
        "revision":state.revision, "patch_epoch":identity.patch_epoch, "mode":mode, "frame":frame, "ticks":ticks,
        "placements":placements, "window_region":window_region,"author_view":author_view,
        "capture_input":identity.input_evidence(),
        "preview_toolbar":preview_toolbar,
        "render_readiness":render_readiness,
        "author_overlay":identity.overlay.report(),
        "author_tools":identity.tools.report(),
        "author_extension":identity.extension.as_ref().map(super::AuthorDomainExtensionScene::report),
        "wire_geometry_projection":wire_geometry,
        "source":"real-project-world", "screenshot":format!("{base}.png")});
    let Some(request)=capture_gate::request(world,capture_gate::Owner::World) else {
        world.insert_resource(evidence);return;
    };
    evidence.pending = Some((base, report, identity.clone()));
    evidence.captured = Some(identity);
    world.insert_resource(evidence);
    world.entity_mut(request).observe(captured);
}

fn record_capture_wait(evidence: &mut Evidence, state: &EmbeddedProjectState, runtime: &Runtime, waiting_for: &str, readiness: serde_json::Value) {
    let now = Instant::now();
    if evidence.next_render_diagnostic.is_some_and(|next| now < next) { return; }
    evidence.next_render_diagnostic = Some(now + Duration::from_secs(2));
    let directory = evidence.directory.clone();
    // 仅在写入输入等待回执时读取窗口归属，保留游戏线程看到的活动窗口与焦点。
    #[cfg(windows)]
    let native_input = (waiting_for == "native-input").then(|| {
        use windows_sys::Win32::UI::{
            Input::KeyboardAndMouse::{GetActiveWindow, GetFocus},
            WindowsAndMessaging::GetForegroundWindow,
        };
        // SAFETY: 这些查询没有输入参数，返回的句柄只用于记录。
        unsafe {
            serde_json::json!({"guest_hwnd":runtime.hwnd,
                "foreground_hwnd":GetForegroundWindow() as usize as u64,
                "active_hwnd":GetActiveWindow() as usize as u64,
                "focus_hwnd":GetFocus() as usize as u64})
        }
    });
    #[cfg(not(windows))]
    let native_input = serde_json::Value::Null;
    let report = serde_json::json!({"pid":std::process::id(),"world_id":state.world_id,
        "revision":state.revision,"mode":format!("{:?}",state.mode),"frame":runtime.frames,
        "waiting_for":waiting_for,"capture_input":evidence.observed.as_ref().map(CaptureIdentity::input_evidence),
        "native_input":native_input,"render_readiness":readiness,"not_capture":true});
    // 编译中/失败也保存明确证据；两秒最多一次并复用截图后台槽，不能阻塞帧或无限排队。
    evidence.writer = Some(IoTaskPool::get().spawn(async move {
        let base = format!("{}-author-view-wait",std::process::id());
        let pending = directory.join(format!("{base}.pending.json"));
        std::fs::write(&pending,serde_json::to_vec_pretty(&report).map_err(|error|error.to_string())?)
            .map_err(|error|error.to_string())?;
        std::fs::rename(pending,directory.join(format!("{base}.json"))).map_err(|error|error.to_string())
    }));
}

fn captured(
    event: On<ScreenshotCaptured>,
    mut evidence: ResMut<Evidence>,
    runtime: NonSend<Runtime>,
    state: Res<EmbeddedProjectState>,
    tools: Res<ToolRenderEvidence>,
    toolbar: super::preview_toolbar::RuntimeEvidenceQuery,
    extension:Option<Res<super::AuthorDomainExtensionScene>>,
    #[cfg(feature = "function_hotpatch")]
    patch:Option<Res<super::FunctionHotpatchState>>,
    mut slot: ResMut<capture_gate::Slot>,
) {
    if !slot.release(capture_gate::Owner::World,event.entity) { return; }
    let Some((base, mut report, identity)) = evidence.pending.take() else {
        return;
    };
    // GPU回读可能晚于下一次窗口尺寸、留孔或输入许可切换；丢弃迟到像素，稳定后重新拍。
    #[cfg(feature = "function_hotpatch")]
    let patch_epoch = patch.as_ref().map_or(0, |state| state.epoch());
    #[cfg(not(feature = "function_hotpatch"))]
    let patch_epoch = 0;
    let current = CaptureIdentity::current(&state, &runtime, &tools, toolbar.current(),extension.as_deref(), patch_epoch);
    if identity != current || !current.native_input_ready()
        // 原生区域取样也可能与身份读取之间跨过启用变化，不能把早期禁用声明配上稍后的有效像素。
        || report["window_region"]["window_enabled"].as_bool() != current.native_window_enabled
        || event.image.width() != identity.viewport.width
        || event.image.height() != identity.viewport.height
    {
        evidence.captured = None;
        return;
    }
    let directory = evidence.directory.clone();
    let image = event.image.clone(); // 仅显式小夹具验收回读，PNG编码和落盘在唯一后台槽。
    evidence.writer = Some(IoTaskPool::get().spawn(async move {
        use sha2::{Digest, Sha256};
        let pixels = image.data.as_ref().ok_or("工程 GPU 截图没有回读像素")?;
        report["capture"] = serde_json::json!({"width":image.width(),"height":image.height(),
            "sha256":Sha256::digest(pixels).iter().map(|byte|format!("{byte:02x}")).collect::<String>(),"source":"Bevy ScreenshotCaptured"});
        let pending_image = directory.join(format!("{base}.pending.png"));
        let pixels = image.try_into_dynamic().map_err(|e| e.to_string())?.to_rgba8();
        report["author_tools"] = identity.tools.inspect_pixels(pixels.width(),pixels.height(),|x,y|pixels.get_pixel(x,y).0);
        report["author_tools"]["rgba_sha256"] = serde_json::json!(Sha256::digest(pixels.as_raw())
            .iter().map(|byte|format!("{byte:02x}")).collect::<String>());
        if let Some(projection) = report.as_object_mut().and_then(|report|report.remove("wire_geometry_projection"))
            .filter(|projection| !projection.is_null()) {
            report["wire_geometry"] = capture_gate::wire_geometry::inspect(
                projection,pixels.width(),pixels.height(),|x,y|pixels.get_pixel(x,y).0);
        }
        pixels.save(&pending_image).map_err(|e| e.to_string())?;
        // 同修订原控件切换各自留档；保留既有base路径供原生区域验收，同时不覆盖前一档画面。
        let view_key = serde_json::to_vec(&identity.author_view).map_err(|e|e.to_string())?;
        // 同四球参数清选择也要保留新的独立证据，不能覆盖或误读仍有金色选框的截图。
        let mut view_digest = Sha256::new();
        view_digest.update(view_key); view_digest.update(identity.overlay.content);
        view_digest.update(identity.patch_epoch.to_le_bytes());
        view_digest.update(identity.overlay.visible_lines.to_le_bytes());
        identity.tools.update_digest(&mut view_digest);
        view_digest.update(serde_json::to_vec(&identity.toolbar).map_err(|error|error.to_string())?);
        if let Some(extension)=&identity.extension {view_digest.update(serde_json::to_vec(&extension.report()).map_err(|e|e.to_string())?);}
        // 相同工具状态往返也保留各自回读，不覆盖首次清选择或恢复后的像素证据。
        view_digest.update(report["frame"].as_u64().unwrap_or(0).to_le_bytes());
        let view_hash = view_digest.finalize().iter().take(8).map(|byte|format!("{byte:02x}")).collect::<String>();
        let view_base = format!("{base}-author-view-{view_hash}");
        let view_pending_image = directory.join(format!("{view_base}.pending.png"));
        std::fs::copy(&pending_image, &view_pending_image).map_err(|e|e.to_string())?;
        std::fs::rename(view_pending_image, directory.join(format!("{view_base}.png"))).map_err(|e|e.to_string())?;
        let mut view_report = report.clone();
        view_report["screenshot"] = serde_json::json!(format!("{view_base}.png"));
        let view_pending = directory.join(format!("{view_base}.pending.json"));
        std::fs::write(&view_pending, serde_json::to_vec_pretty(&view_report).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        std::fs::rename(view_pending, directory.join(format!("{view_base}.json"))).map_err(|e|e.to_string())?;
        // alias继续服务旧验收读者；正文指向不可变的epoch/帧文件，后续模式切换不能覆盖历史证据。
        let epoch_base = format!("{base}-patch-{}-frame-{}",identity.patch_epoch,report["frame"].as_u64().unwrap_or(0));
        let epoch_pending_image = directory.join(format!("{epoch_base}.pending.png"));
        std::fs::copy(&pending_image,&epoch_pending_image).map_err(|e|e.to_string())?;
        std::fs::rename(epoch_pending_image,directory.join(format!("{epoch_base}.png"))).map_err(|e|e.to_string())?;
        report["screenshot"] = serde_json::json!(format!("{epoch_base}.png"));
        let epoch_pending_report = directory.join(format!("{epoch_base}.pending.json"));
        std::fs::write(&epoch_pending_report,serde_json::to_vec_pretty(&report).map_err(|e|e.to_string())?)
            .map_err(|e|e.to_string())?;
        std::fs::rename(epoch_pending_report,directory.join(format!("{epoch_base}.json"))).map_err(|e|e.to_string())?;
        // 同修订几何会重拍；先完整写临时文件再替换，宿主轮询不能读到被截断的PNG或JSON。
        std::fs::rename(&pending_image, directory.join(format!("{base}.png"))).map_err(|e| e.to_string())?;
        let pending_report = directory.join(format!("{base}.pending.json"));
        std::fs::write(&pending_report, serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(&pending_report, directory.join(format!("{base}.json"))).map_err(|e| e.to_string())
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_map_revision_requires_new_capture_after_geometry_or_occlusion_changes() {
        let original = CaptureIdentity {
            revision: 3,
            patch_epoch: 0,
            mode: Mode::Editing,
            native_window_enabled: Some(false),
            author_view: None,
            camera: None,
            overlay: OverlayIdentity::default(),
            tools: ToolRenderEvidence::default(),
            toolbar: default(),
            extension:None,
            viewport: Viewport {
                width: 800,
                height: 600,
                excluded_rects: vec![[10, 10, 20, 20]],
                ..default()
            },
        };
        assert!(original == original.clone());
        let mut patched = original.clone(); patched.patch_epoch = 1;
        assert!(original != patched,"同PID/World/地图修订的新补丁必须取新的真实GPU像素");
        let mut resized = original.clone();
        resized.viewport.width = 900;
        assert!(original != resized);
        let mut moved_ui = original.clone();
        moved_ui.viewport.excluded_rects[0][0] = 30;
        assert!(original != moved_ui);
        let mut playing = original.clone();
        playing.mode = Mode::Playing;
        assert!(original != playing);
        let mut shading = original.clone();
        shading.author_view = Some(feathrise_editor_protocol::AuthorViewState {
            shading:[1.0,0.0,0.0,1.0],wire:[0.0;4],grid:None,
            terrain_flat:false,terrain_grid:false,terrain_backfaces:false,physics_debug:false,
        });
        assert!(original != shading, "原四球控件不改地图修订，也必须重新取真实GPU像素");
        let mut moved_camera = original.clone();
        moved_camera.camera = Some(feathrise_editor_protocol::CameraState {
            position:[1.,2.,3.],rotation:[0.,0.,0.,1.],vertical_fov_radians:0.8,orthographic_height:None,
        });
        assert!(original != moved_camera,"同修订相机移动也必须拒绝旧投影与迟到像素");
    }

    #[test]
    fn native_input_permission_requires_new_gpu_capture_even_with_unchanged_focus_request() {
        let disabled = CaptureIdentity {
            revision: 3, patch_epoch: 0, mode: Mode::Playing,
            viewport: Viewport { width: 800, height: 600, visible: true, focused: true, ..default() },
            native_window_enabled: Some(false), author_view: None, camera: None,
            overlay: OverlayIdentity::default(), tools: ToolRenderEvidence::default(),
            toolbar: default(), extension: None,
        };
        assert!(!disabled.native_input_ready(), "请求已有焦点时也不能采用实际禁用的Playing图");
        let mut enabled = disabled.clone(); enabled.native_window_enabled = Some(true);
        assert!(enabled.native_input_ready());
        assert!(disabled != enabled, "实际输入许可变化必须拒绝旧缓存和迟到GPU回读");
        assert!(disabled.viewport == enabled.viewport, "本回归必须覆盖请求几何完全未变的情况");
        let mut unfocused = enabled.clone(); unfocused.viewport.focused = false;
        assert!(!unfocused.native_input_ready(), "真实启用仍不能替代宿主请求焦点");
        let mut hidden = enabled.clone(); hidden.viewport.visible = false;
        assert!(!hidden.native_input_ready());
        let mut missing_window = enabled.clone(); missing_window.native_window_enabled = None;
        assert!(!missing_window.native_input_ready());
        let mut editing = enabled; editing.mode = Mode::Editing;
        assert!(!editing.native_input_ready(), "编辑态输入尚未归还时同样不得截图通过");
        editing.native_window_enabled = Some(false);
        assert!(editing.native_input_ready());
    }

    #[test]
    fn overlay_capture_identity_rejects_cleared_moved_and_hidden_selection_lines() {
        use feathrise_editor_protocol::OverlayLine;
        let state = EmbeddedProjectState { mode:Mode::Editing,world_id:1,revision:7,transitioning:false };
        let mut lines = vec![OverlayLine {start:[0.,0.,0.],end:[1.,0.,0.],
            start_color:[1.,0.5,0.,1.],end_color:[1.,0.5,0.,1.]}];
        let selected = OverlayIdentity::current(&state,false,Some((7,&lines)));
        assert_eq!(selected.visible_lines,1);
        let clear = OverlayIdentity::current(&state,false,Some((7,&[])));
        assert_eq!(clear.visible_lines,0);
        assert!(selected != clear,"清选择之后不可复用被原高亮覆盖的像素");
        lines[0].start[0]=0.25;
        assert!(selected != OverlayIdentity::current(&state,false,Some((7,&lines))),
            "相同线数但位置已改变也必须拒绝迟到截图");
        assert_eq!(OverlayIdentity::current(&state,true,Some((7,&lines))).visible_lines,0);
        assert_eq!(OverlayIdentity::current(&state,false,Some((6,&lines))).visible_lines,0);
        let playing = EmbeddedProjectState {mode:Mode::Playing,..state};
        assert_eq!(OverlayIdentity::current(&playing,false,Some((7,&lines))).visible_lines,0);
    }
}
