//! 纯数据消息与固定预算；实体、资源句柄及作者文档类型不得穿过本合同。

use crate::{AuthorToolPreview, AuthorToolValidationError, ProtocolError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// 快照与增量消息改用 scene_id，旧进程必须在帧头处被拒绝，不能在进入采用队列后才发现字段不匹配。
pub const PROTOCOL_VERSION: u32 = 2;
/// 函数体热补丁是可选开发能力，不改变旧工程必须实现的常驻视口合同。
pub const FUNCTION_HOTPATCH_CAPABILITY: &str = "function-hotpatch-v1";
/// 中央作者浮层只允许固定数量的矩形裁剪，避免窗口区域重建随任意UI节点规模增长。
pub const MAX_VIEWPORT_EXCLUDED_RECTS: usize = 32;
/// XYZ 只保留三条轴线、六个端点和一个中心，不允许把任意 UI 树变成窗口区域。
pub const MAX_VIEWPORT_EXCLUDED_SHAPES: usize = 10;

/// 子视口局部物理像素中的作者轮廓；空心圆只让出圆环，内部仍显示工程画面。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewportUiShape {
    Ellipse { outer: [i32; 4], inner: Option<[i32; 4]> },
    Quad { points: [[i32; 2]; 4] },
}

impl ViewportUiShape {
    /// 固定三个物理取样点覆盖实心、轮廓和透明空隙，供两端各自核对真实窗口区域。
    pub fn probe_points(&self) -> [[i32; 2]; 3] {
        match self {
            Self::Ellipse { outer: [l, t, r, b], .. } => [
                [(l + r) / 2, (t + b) / 2], [*l, *t], [(l + r) / 2, t + 1],
            ],
            Self::Quad { points } => [
                [points.iter().map(|p| p[0]).sum::<i32>() / 4,
                    points.iter().map(|p| p[1]).sum::<i32>() / 4],
                points[0], [(points[0][0] + points[1][0]) / 2, (points[0][1] + points[1][1]) / 2],
            ],
        }
    }

    /// 允许边缘控件部分越过视口，最终由完整窗口区域裁切；先限制坐标和形状防止 GDI 溢出。
    pub fn valid(&self) -> bool {
        let coordinate = |value: i32| (-32_768..=65_536).contains(&value);
        let rect = |[l, t, r, b]: [i32; 4]| {
            [l, t, r, b].into_iter().all(coordinate)
                && r > l && b > t && r - l <= 32_768 && b - t <= 32_768
        };
        match self {
            Self::Ellipse { outer, inner } => rect(*outer) && inner.is_none_or(|inner| {
                rect(inner) && inner[0] >= outer[0] && inner[1] >= outer[1]
                    && inner[2] <= outer[2] && inner[3] <= outer[3]
            }),
            Self::Quad { points } => {
                if !points.iter().flatten().copied().all(coordinate) { return false; }
                let mut direction = 0;
                for index in 0..4 {
                    let a = points[index]; let b = points[(index + 1) % 4]; let c = points[(index + 2) % 4];
                    let cross = i64::from(b[0] - a[0]) * i64::from(c[1] - b[1])
                        - i64::from(b[1] - a[1]) * i64::from(c[0] - b[0]);
                    if cross == 0 || direction != 0 && direction != cross.signum() { return false; }
                    direction = cross.signum();
                }
                true
            }
        }
    }
}
pub const MAX_PREVIEW_BYTES: usize = 64 * 1024 * 1024;

/// 预算限制字节而非场景对象数量；完整超限返回失败，禁止截断成半份场景。
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub metadata_bytes: usize,
    pub snapshot_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            metadata_bytes: 512 * 1024,
            snapshot_bytes: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionIdentity {
    pub project_id: String,
    /// 每次启动重新生成的 nonce；重启不得复用上一进程的应答。
    pub session: u64,
    /// 切换场景或重建换代；同一场景普通草稿只推进 revision。
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Editing,
    Playing,
}

/// 各作者预览拥有独立的采用序号；传输顺序、场景代次仍由外层会话统一约束。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    Placement,
    Vegetation,
    AuthorDomain,
    SurfaceTrail,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraState {
    pub position: [f32; 3],
    /// 四元数按 xyzw 传递；接收端校验后归一化，不交换 Bevy 类型。
    pub rotation: [f32; 4],
    pub vertical_fov_radians: f32,
    /// 正交视图按固定垂直世界高度同步；None 为透视，避免六向视图退化为透视相机。
    #[serde(default)]
    pub orthographic_height: Option<f32>,
}

/// 作者辅助线只交换世界坐标和线性 RGBA；不泄漏实体、Gizmo 或渲染资源句柄。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayLine {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub start_color: [f32; 4],
    pub end_color: [f32; 4],
}

/// 原作者相机的固定显示参数，接收端复用同一渲染插件；不交换GPU句柄或场景对象。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorViewState {
    /// 原四球着色Uniform：模式0..3、轮廓、法线颜色、背景。
    pub shading: [f32; 4],
    pub wire: [f32; 4],
    /// 无限世界参考网格Uniform：origin_stride、plane、axis_u、axis_v；None明确关闭。
    /// 显示端将相位与平面距离锚定世界原点，不能继承资产姿态或相对吸附原点。
    pub grid: Option<[[f32; 4]; 4]>,
    pub terrain_flat: bool,
    pub terrain_grid: bool,
    pub terrain_backfaces: bool,
    pub physics_debug: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    /// 游戏窗口收到 F12 / Escape 只提出意图，由宿主统一分配模式事务号。
    RequestMode {
        mode: Mode,
    },
    Hello {
        build_identity: String,
        capabilities: Vec<String>,
    },
    Ready {
        pid: u32,
        hwnd: u64,
        world_id: u64,
        build_identity: String,
        capabilities: Vec<String>,
    },
    /// 只有显式开发构建与本轮受控产物目录均有效时发布；ASLR 来自实际运行进程。
    HotPatchReady {
        pid: u32,
        world_id: u64,
        aslr_reference: u64,
        base_build: String,
    },
    /// 大跳表只存在受检产物包；本消息绑定当前源码代次及相对产物路径。
    ApplyCodePatch {
        patch_id: u64,
        source_generation: u64,
        base_build: String,
        artifact: String,
        sha256: String,
    },
    /// 补丁失败属于独立采用结果，不可把健康旧实例误标为控制连接失败。
    CodePatchResult {
        patch_id: u64,
        source_generation: u64,
        base_build: String,
        pid: u32,
        world_id: u64,
        epoch: u64,
        applied: bool,
        reason: Option<String>,
    },
    SetMode {
        request_id: u64,
        mode: Mode,
        required_revision: u64,
    },
    /// 只有真实采用且切换完成后才应答；宿主不能把已发送请求当成状态事实。
    ModeApplied {
        request_id: u64,
        mode: Mode,
        revision: u64,
        world_id: u64,
    },
    SetCamera {
        camera: CameraState,
    },
    SetAuthorView {
        view: AuthorViewState,
    },
    /// 完整替换当前辅助线；修订必须与已采用场景一致，空列表显式清除旧辅助线。
    SetOverlay {
        revision: u64,
        lines: Vec<OverlayLine>,
    },
    /// 完整替换笔刷、变换手柄与选择框；语义只在该场景修订采用后显示，空集合清除。
    SetAuthorTools {
        revision: u64,
        preview: AuthorToolPreview,
    },
    SetViewport {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        visible: bool,
        focused: bool,
        /// 子窗口局部物理像素中的 [x,y,width,height]；完整替换，空列表清除旧裁剪。
        /// 由 editor-ui-occlusion-v1 能力协商启用，旧元信息缺省为空不改变协议帧版本。
        #[serde(default)]
        excluded_rects: Vec<[u32; 4]>,
        /// 由 editor-ui-shapes-v1 协商启用；原空矩形间隙不应露出父窗口背景。
        #[serde(default)]
        excluded_shapes: Vec<ViewportUiShape>,
    },
    /// 正文由 Envelope.payload 独立持有；format 的语义归 SDK 消费者。
    Snapshot {
        transaction: u64,
        revision: u64,
        scene_id: String,
        format: String,
    },
    SnapshotApplied {
        transaction: u64,
        revision: u64,
    },
    /// 原作者系统已产生的增量预览；正文格式由共享数据模块拥有，不传鼠标或ECS镜像。
    Preview {
        #[serde(rename = "domain")]
        kind: PreviewKind,
        sequence: u64,
        scene_id: String,
        format: String,
    },
    PreviewApplied {
        #[serde(rename = "domain")]
        kind: PreviewKind,
        sequence: u64,
    },
    Status {
        world_id: u64,
        revision: u64,
        frame: u64,
        ticks: u64,
        mode: Mode,
    },
    Failure {
        request_id: u64,
        code: String,
        message: String,
    },
    Stop,
    Stopped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub identity: SessionIdentity,
    pub sequence: u64,
    pub message: Message,
    /// 原始字节不编码成 JSON 数字或十六进制；克隆消息仅共享不可变正文。
    #[serde(skip)]
    pub payload: Option<Arc<Vec<u8>>>,
}

impl Envelope {
    pub fn new(identity: SessionIdentity, sequence: u64, message: Message) -> Self {
        Self {
            identity,
            sequence,
            message,
            payload: None,
        }
    }

    pub fn with_payload(mut self, payload: Arc<Vec<u8>>) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn validate(&self, limits: &Limits) -> Result<(), ProtocolError> {
        let fail = |message: &str| ProtocolError::Invalid(message.into());
        if self.identity.project_id.is_empty()
            || self.identity.project_id.len() > 256
            || self.identity.session == 0
            || self.identity.generation == 0
            || self.sequence == 0
        {
            return Err(fail("工程、会话、场景代次或顺序号无效"));
        }
        let payload_bytes = self.payload.as_ref().map_or(0, |bytes| bytes.len());
        if payload_bytes > limits.snapshot_bytes {
            return Err(ProtocolError::Limit("完整快照超过配置的字节预算".into()));
        }
        if matches!(self.message, Message::Snapshot { .. } | Message::Preview { .. }) != (payload_bytes != 0) {
            return Err(fail("只有 Snapshot 或 Preview 可以且必须携带非空二进制正文"));
        }
        if matches!(self.message, Message::Preview { .. }) && payload_bytes > MAX_PREVIEW_BYTES {
            return Err(ProtocolError::Limit("作者增量预览超过64MiB预算".into()));
        }
        match &self.message {
            Message::HotPatchReady { pid, world_id, aslr_reference, base_build } => {
                if *pid == 0 || *world_id == 0 || *aslr_reference == 0 || !patch_sha256(base_build) {
                    return Err(fail("热补丁就绪缺少真实进程、World、ASLR或基线身份"));
                }
            }
            Message::ApplyCodePatch { patch_id, source_generation, base_build, artifact, sha256 } => {
                if *patch_id == 0 || *source_generation == 0 || !patch_sha256(base_build)
                    || !patch_sha256(sha256) || !patch_relative_path(artifact) {
                    return Err(fail("热补丁请求身份、相对产物路径或摘要无效"));
                }
            }
            Message::CodePatchResult { patch_id, source_generation, base_build, pid, world_id, epoch, applied, reason } => {
                if *patch_id == 0 || *source_generation == 0 || !patch_sha256(base_build) || *pid == 0 || *world_id == 0
                    || *applied && (*epoch == 0 || reason.is_some())
                    || !*applied && reason.as_ref().is_none_or(|reason| reason.is_empty() || reason.len() > 8 * 1024) {
                    return Err(fail("热补丁结果的事务、运行身份或成功／失败事实无效"));
                }
            }
            Message::Hello {
                build_identity,
                capabilities,
            }
            | Message::Ready {
                build_identity,
                capabilities,
                ..
            } => {
                if build_identity.is_empty()
                    || build_identity.len() > 256
                    || capabilities.len() > 32
                    || capabilities
                        .iter()
                        .any(|cap| cap.is_empty() || cap.len() > 128)
                {
                    return Err(fail("握手版本或能力列表无效"));
                }
                if let Message::Ready {
                    pid,
                    hwnd,
                    world_id,
                    ..
                } = self.message
                    && (pid == 0 || hwnd == 0 || world_id == 0)
                {
                    return Err(fail("就绪回执缺少实际进程、窗口或 World 身份"));
                }
            }
            Message::SetMode { request_id, .. } if *request_id == 0 => {
                return Err(fail("模式请求缺少请求号"));
            }
            Message::ModeApplied {
                request_id,
                world_id,
                ..
            } if *request_id == 0 || *world_id == 0 => {
                return Err(fail("模式回执身份无效"));
            }
            Message::SetCamera { camera } => {
                let length: f32 = camera.rotation.iter().map(|value| value * value).sum();
                if !camera
                    .position
                    .iter()
                    .chain(&camera.rotation)
                    .all(|value| value.is_finite())
                    || !length.is_finite()
                    || length < 0.0001
                    || !camera.vertical_fov_radians.is_finite()
                    || !(0.01..3.13).contains(&camera.vertical_fov_radians)
                {
                    return Err(fail("相机包含非有限数、零四元数或无效视角"));
                }
                if camera
                    .orthographic_height
                    .is_some_and(|height| !height.is_finite() || height <= 0.0)
                {
                    return Err(fail("正交相机垂直高度必须为有限正数"));
                }
            }
            Message::SetAuthorView { view } => {
                if !view.shading.iter().chain(&view.wire).all(|value| value.is_finite())
                    || ![0.,1.,2.,3.].contains(&view.shading[0])
                    || view.shading[1..].iter().chain(&view.wire[..2]).any(|value| ![0.,1.].contains(value)) {
                    return Err(fail("作者着色参数必须为已知模式与布尔开关"));
                }
                if let Some(grid) = view.grid {
                    let length = |axis: &[f32;4]| axis[..3].iter().map(|value| value*value).sum::<f32>();
                    if !grid.iter().flatten().all(|value| value.is_finite()) || grid[0][3] <= 0.
                        || [1,2,3].into_iter().any(|index| (length(&grid[index])-1.).abs() > 0.001) {
                        return Err(fail("参考网格需要有限原点、正步长和单位工作平面轴"));
                    }
                }
            }
            Message::SetViewport {
                width,
                height,
                excluded_rects,
                excluded_shapes,
                ..
            } => {
                if *width > 32_768 || *height > 32_768 {
                    return Err(fail("视口尺寸超过支持范围"));
                }
                if excluded_rects.len() > MAX_VIEWPORT_EXCLUDED_RECTS {
                    return Err(fail("作者浮层裁剪超过32个矩形预算"));
                }
                if excluded_shapes.len() > MAX_VIEWPORT_EXCLUDED_SHAPES
                    || excluded_shapes.iter().any(|shape| !shape.valid())
                {
                    return Err(fail("作者轮廓必须为十个以内的有效圆环或凸四边形"));
                }
                // 先检查加法和完整边界，避免溢出坐标进入原生窗口区域API。
                if excluded_rects.iter().any(|[x, y, w, h]| {
                    *w == 0
                        || *h == 0
                        || x.checked_add(*w).is_none_or(|right| right > *width)
                        || y.checked_add(*h).is_none_or(|bottom| bottom > *height)
                }) {
                    return Err(fail("作者浮层裁剪必须为视口内非空且不溢出的局部像素矩形"));
                }
            }
            Message::SetAuthorTools { revision, preview } => {
                if *revision == 0 {
                    return Err(fail("作者工具缺少已采用场景修订"));
                }
                preview.validate().map_err(|error| match error {
                    AuthorToolValidationError::Limit(message) => ProtocolError::Limit(message.into()),
                    AuthorToolValidationError::Invalid(message) => ProtocolError::Invalid(message.into()),
                })?;
            }
            Message::SetOverlay { revision, lines } => {
                if *revision == 0 || lines.len() > 1024 {
                    return Err(fail("辅助线缺少场景修订或超过 1024 段预算"));
                }
                if lines.iter().any(|line| {
                    !line
                        .start
                        .iter()
                        .chain(&line.end)
                        .chain(&line.start_color)
                        .chain(&line.end_color)
                        .all(|value| value.is_finite())
                }) {
                    return Err(fail("辅助线坐标或线性颜色包含非有限数"));
                }
            }
            Message::Snapshot {
                transaction,
                revision,
                scene_id,
                format,
            } => {
                if *transaction == 0
                    || *revision == 0
                    || scene_id.is_empty()
                    || scene_id.len() > 256
                    || format.is_empty()
                    || format.len() > 128
                {
                    return Err(fail("完整快照的事务、修订或格式身份无效"));
                }
            }
            Message::SnapshotApplied {
                transaction,
                revision,
            } if *transaction == 0 || *revision == 0 => {
                return Err(fail("快照回执身份无效"));
            }
            Message::Preview { sequence, scene_id, format, .. } => {
                if *sequence == 0 || scene_id.is_empty() || scene_id.len() > 256
                    || format.is_empty() || format.len() > 128 {
                    return Err(fail("增量预览缺少有效序号、场景或格式身份"));
                }
            }
            Message::PreviewApplied { sequence, .. } if *sequence == 0 => {
                return Err(fail("增量预览回执缺少采用序号"));
            }
            Message::Status { world_id, .. } if *world_id == 0 => {
                return Err(fail("状态缺少 World 身份"));
            }
            Message::Failure { code, message, .. }
                if code.is_empty() || code.len() > 128 || message.len() > 8 * 1024 =>
            {
                return Err(fail("失败诊断无效或过长"));
            }
            _ => {}
        }
        Ok(())
    }
}

fn patch_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn patch_relative_path(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
        && !value.contains(':') && value.split(['/', '\\']).all(|part| !matches!(part, "" | "." | ".."))
}
