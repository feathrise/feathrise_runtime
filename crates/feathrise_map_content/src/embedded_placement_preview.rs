//! 编辑器原手势的跨进程投影；只含稳定作者身份，绝不传递 ECS Entity。

use crate::MapId;
use serde::{Deserialize, Serialize};

pub const PLACEMENT_PREVIEW_FORMAT: &str = "feathrise-placement-preview-v1";
pub const MAX_PLACEMENT_PREVIEW_MEMBERS: usize = 256;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PlacementPreviewKind {
    Absolute,
    GroupDelta,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlacementPreviewFinalMember {
    pub id: String,
    /// 原作者提交或取消后的来源局部TRS；不得传临时pivot下的局部值。
    pub local: [f32; 16],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PlacementPreviewCommand {
    Begin {
        gesture: u64,
        map_id: MapId,
        source_revision: u64,
        ids: Vec<String>,
        kind: PlacementPreviewKind,
    },
    Update {
        gesture: u64,
        sequence: u64,
        transform: [f32; 16],
    },
    End {
        gesture: u64,
        sequence: u64,
        source_revision: u64,
        transform: Option<[f32; 16]>,
        cancelled: bool,
        #[serde(default)]
        final_members: Vec<PlacementPreviewFinalMember>,
    },
}
