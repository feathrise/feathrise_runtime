//! 常驻会话自身保留一份待采用快照、活动来源和退役值；后台队列分别有界，不生成额外 World。

use super::{
    snapshot::PreparedSnapshot,
    transport::{Configuration, Transport},
};
use bevy::prelude::*;
use feathrise_editor_protocol::{CameraState, Mode, OverlayLine};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Viewport {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
    pub focused: bool,
    pub excluded_rects: Vec<[u32; 4]>,
    pub excluded_shapes: Vec<feathrise_editor_protocol::ViewportUiShape>,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            visible: false,
            focused: false,
            excluded_rects: Vec::new(),
            excluded_shapes: Vec::new(),
        }
    }
}

pub(super) struct PendingMode {
    pub request_id: u64,
    pub mode: Mode,
    pub revision: u64,
    pub started: Instant,
    pub native_started: bool,
}

/// 工程窗口请求与作者窗口租约分开；编辑临时窗口化不写回玩家偏好或工程文件。
pub(super) struct GameWindowLease {
    pub mode: bevy::window::WindowMode,
    pub applied_mode: bevy::window::WindowMode,
    pub mode_pending: bool,
    pub resolution: bevy::window::WindowResolution,
    pub position: bevy::window::WindowPosition,
    pub decorations: bool,
    pub cursor: bevy::window::CursorOptions,
    pub native_style: Option<(isize, isize)>,
    pub editor_viewport: Option<Viewport>,
    pub placement: Option<NativePlacement>,
    pub restoring_placement: bool,
    pub native_geometry: Option<NativeGeometry>,
    pub geometry_request: Option<GameGeometryRequest>,
}

/// 上次向工程发布的完整原生几何；MIN的0尺寸和MAX尺寸都是事实，不是新的normal请求。
pub(super) struct NativeGeometry {
    pub resolution: bevy::window::WindowResolution,
    pub position: bevy::window::WindowPosition,
}

/// 真正Game Update写入的字段才形成命令；发出一次后等待读回，不能持续撤销原生MIN/MAX。
#[derive(Default)]
pub(super) struct GameGeometryRequest {
    pub size: Option<UVec2>,
    pub position: Option<bevy::window::WindowPosition>,
    pub issued: bool,
}

/// 保存GetWindowPlacement原坐标语义；normal不是最大化客户区，也不接受编辑租约回写。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NativePlacement {
    pub normal: [i32; 4],
    pub minimized_position: [i32; 2],
    pub maximized_position: [i32; 2],
    pub flags: u32,
    pub show_command: u32,
    pub minimized: bool,
    pub maximized: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeStage {
    Prepare,
    AwaitWindowed,
    AwaitRestored,
    AwaitAttach,
    AwaitGameRestored,
    AwaitApplied,
    AwaitVisible,
}

/// 只有原生承载完整成功才产生模式结果；失败先反向恢复原承载，再报告原请求失败。
pub(super) struct NativeHandoff {
    pub request_id: u64,
    pub previous: Mode,
    pub target: Mode,
    pub stage: NativeStage,
    pub stage_frame: u64,
    pub started: Instant,
    pub failure: Option<String>,
    pub restore_focus: bool,
    pub source_visible: bool,
}

pub(super) struct ApplyingSnapshot {
    pub value: PreparedSnapshot,
    pub cursor: usize,
    pub published: bool,
    pub quiescent_frame: Option<u64>,
}

pub(super) struct Runtime {
    pub config: Configuration,
    pub transport: Transport,
    pub hello: bool,
    pub ready: bool,
    pub failed: bool,
    pub stopping: bool,
    pub hwnd: Option<usize>,
    pub window_lease: Option<GameWindowLease>,
    pub native_child: bool,
    pub native_mode: Mode,
    pub native_handoff: Option<NativeHandoff>,
    pub native_result: Option<(u64, Result<(), String>)>,
    pub window_return_requested: bool,
    pub cursor_focus_owned: bool,
    pub cursor_anchor_pending: bool,
    pub cursor_last_grab: Option<bevy::window::CursorGrabMode>,
    pub cursor_reanchors: u64,
    pub viewport: Viewport,
    pub last_viewport: Option<Viewport>,
    pub region_mode: Option<Mode>,
    pub camera: Option<CameraState>,
    pub overlay: Option<(u64, Vec<OverlayLine>)>,
    pub author_tools: Option<(u64, feathrise_editor_protocol::AuthorToolPreview)>,
    pub game_camera: Option<(Entity, Transform, Projection)>,
    pub camera_mode: Mode,
    pub pending: Option<ApplyingSnapshot>,
    pub pending_mode: Option<PendingMode>,
    pub retired: Option<Box<dyn Send>>,
    pub paths: Vec<String>,
    pub readiness_cursor: usize,
    pub sources_ready: bool,
    pub sources_settled: bool,
    pub source_failed: bool,
    pub source_evidence_lost: bool,
    pub source_reads: std::sync::Arc<super::asset_overlay::ReadEvidence>,
    pub source_handles: Vec<bevy::asset::UntypedHandle>,
    pub source_ready_frame: u64,
    pub applied_frame: u64,
    pub frames: u64,
    pub ticks: u64,
    pub last_status: Instant,
    pub requested_return: bool,
    pub input_suspended: bool,
    pub previews: super::preview::State,
    pub author_view: Option<feathrise_editor_protocol::AuthorViewState>,
    pub author_camera: Option<Entity>,
    pub hotpatch_available: bool,
}

impl Runtime {
    pub(super) fn new(config: Configuration, transport: Transport) -> Self {
        Self {
            config,
            transport,
            hello: false,
            ready: false,
            failed: false,
            stopping: false,
            hwnd: None,
            window_lease: None,
            native_child: false,
            native_mode: Mode::Editing,
            native_handoff: None,
            native_result: None,
            window_return_requested: false,
            cursor_focus_owned: false,
            cursor_anchor_pending: false,
            cursor_last_grab: None,
            cursor_reanchors: 0,
            viewport: default(),
            last_viewport: None,
            region_mode: None,
            camera: None,
            overlay: None,
            author_tools: None,
            game_camera: None,
            camera_mode: Mode::Editing,
            pending: None,
            pending_mode: None,
            retired: None,
            paths: Vec::new(),
            readiness_cursor: 0,
            sources_ready: true,
            sources_settled: true,
            source_failed: false,
            source_evidence_lost: false,
            source_reads: default(),
            source_handles: Vec::new(),
            source_ready_frame: 0,
            applied_frame: 0,
            frames: 0,
            ticks: 0,
            last_status: Instant::now(),
            requested_return: false,
            input_suspended: true,
            previews: default(),
            author_view: None,
            author_camera: None,
            hotpatch_available: false,
        }
    }
}
