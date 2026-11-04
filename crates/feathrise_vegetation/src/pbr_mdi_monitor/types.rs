//! PBR MDI 排队镜像的纯值类型。

use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d};
use bevy::pbr::Shadow;
use bevy::render::{
    render_phase::BinnedPhaseItem, sync_world::MainEntity, view::RetainedViewEntity,
};

/// 单个实际进入MDI阶段的可见实体及其官方分箱键。
pub(super) struct PbrMdiPhaseEntry<BPI>
where
    BPI: BinnedPhaseItem,
{
    pub(super) view: RetainedViewEntity, // 主相机或单个灯光子视图的稳定身份。
    pub(super) entity: MainEntity,       // 与公开普通分箱一致的主世界实体身份。
    pub(super) batch_set_key: BPI::BatchSetKey, // 决定一次多绘制提交归组的完整官方键。
    pub(super) bin_key: BPI::BinKey,     // 决定批集内具体Mesh分箱的完整官方键。
}

/// 一个PBR阶段在F3中使用的统一提交统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PbrPhaseStats {
    pub views: u32,       // 至少包含一个目标普通或MDI分箱的视图数量。
    pub draw_groups: u32, // MDI批集、普通批次或独立实体形成的实际提交组数量。
    pub bins: u32,        // 同一视图中命中目标的唯一官方分箱数量。
    pub instances: u32,   // 目标实体跨视图累计实例数。
}

/// 当前完整Render阶段的三类MDI实体镜像。
#[derive(Default)]
pub struct PbrMdiQueueMirror {
    pub(super) opaque: Vec<PbrMdiPhaseEntry<Opaque3d>>, // 主画面不透明MDI条目。
    pub(super) alpha_mask: Vec<PbrMdiPhaseEntry<AlphaMask3d>>, // 主画面遮罩MDI条目。
    pub(super) shadow: Vec<PbrMdiPhaseEntry<Shadow>>,   // 方向光、点光与聚光灯阴影MDI条目。
}

impl PbrMdiQueueMirror {
    /// 清除上一帧逻辑长度并保留诊断开启期间的稳定容量。
    pub(super) fn clear(&mut self) {
        self.opaque.clear(); // 主画面Opaque条目不跨帧继承。
        self.alpha_mask.clear(); // 主画面AlphaMask条目不跨帧继承。
        self.shadow.clear(); // 阴影视图条目不跨帧继承。
    }
}
