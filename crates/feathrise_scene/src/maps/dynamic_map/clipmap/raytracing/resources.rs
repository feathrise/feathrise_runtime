//! 场景只拥有固定高度代理的准备状态；启用由真实 Solari 能力与正式画质请求共同决定。

use super::types::{ProxyCapture, ProxyGarbage, ProxyIdentity, ProxyResult};
use bevy::{prelude::*, tasks::Task};
use feathrise_map_content::SparseHeightTileAsset;

#[derive(Resource, Default)]
pub struct ClipmapRaytracingRequest {
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipmapRaytracingPhase {
    #[default]
    Disabled,
    WaitingForTerrain,
    LoadingPages,
    Capturing,
    Building,
    Ready,
    Failed,
}

impl ClipmapRaytracingPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "关闭",
            Self::WaitingForTerrain => "等待正式地形",
            Self::LoadingPages => "加载高度页",
            Self::Capturing => "准备高度输入",
            Self::Building => "构建地形代理",
            Self::Ready => "地形代理就绪",
            Self::Failed => "地形代理失败",
        }
    }
}

/// F1/光追生命周期读取真实代理准备情况，Ready 仍需 Solari 自己的 GPU 预热。
#[derive(Resource, Default)]
pub struct ClipmapRaytracingStatus {
    pub phase: ClipmapRaytracingPhase,
    pub mip_level: u8,
    pub grid_cells: [u32; 2],
    pub detail: Option<String>,
}

#[derive(Resource, Default)]
pub(crate) struct ClipmapRaytracingRuntime {
    pub(super) ticket: u64,
    pub(super) requested: Option<ProxyIdentity>,
    pub(super) capture: Option<ProxyCapture>,
    pub(super) task: Option<Task<ProxyResult>>,
    pub(super) started: Option<std::time::Instant>,
    pub(super) failed: Option<ProxyIdentity>,
    pub(super) anchor: Option<Entity>,
    pub(super) mesh: Option<Handle<Mesh>>,
    pub(super) pending_io: Vec<Handle<SparseHeightTileAsset>>, // 取消加载仍占唯一 IO 槽到真实完成。
    pub(super) release: Option<Task<()>>,
    pub(super) retired: Vec<ProxyGarbage>, // 新捕获等到队列清空，避免连续换图累积大结果。
}

impl ClipmapRaytracingRuntime {
    pub(crate) fn occupies_io_slot(&self) -> bool {
        !self.pending_io.is_empty()
            || self
                .capture
                .as_ref()
                .is_some_and(|input| input.formal.len() < input.refs.len())
        // 连续父页加载之间也保留槽位，防止移动环带把三个槽反复占满使光追饿死。
    }
}

/// RenderApp按实际绑定顺序发布的高度实例路由；仅当前锚点可消费。
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClipmapRaytracingInstanceRoute {
    pub anchor: Option<Entity>,
    pub index: u32,
}
