//! 动态场景区域边界状态、诊断与F12未保存覆盖契约。

use bevy::prelude::*; // Entity与Resource。
use feathrise_scene_content::{SceneBoundariesAsset, SceneId};
use std::sync::Arc; // 草稿发布后以不可变共享所有权交给后台派生。 // 正式文件与编辑草稿共享同一数据结构。

/// 动态场景区域边界加载阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicSceneBoundaryPhase {
    /// 当前场景未声明边界文件，也没有F12未保存覆盖。
    #[default]
    Disabled,
    /// 正在等待目录或边界JSON资产。
    Loading,
    /// 完整静态碰撞已经原子采用。
    Ready,
    /// 首次加载失败没有放行场景，或热更失败后继续保留上一完整边界。
    Failed,
}

/// F12和诊断面板可读取的区域边界运行时证据。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicSceneBoundaryStatus {
    /// 已完成结果所属未保存作者修订；文件结果为None，关闭门不能把旧碰撞当作新草稿。
    pub override_revision: Option<u64>,
    /// 后台严格保存校验结果；开放外框可预览但必须修正后才能关闭F12。
    pub validation_error: Option<String>,
    /// 当前加载阶段。
    pub phase: DynamicSceneBoundaryPhase,
    /// 当前边界所属动态场景。
    pub scene_id: Option<SceneId>,
    /// 已原子采用的静态碰撞段数。
    pub segment_count: usize,
    /// 成功采用次数。
    pub revision: u64,
    /// 当前等待项、成功摘要或失败原因。
    pub detail: String,
}

/// F12把当前未保存边界草稿发布给正式动态场景运行时的轻量覆盖层。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicSceneBoundaryOverride {
    /// 覆盖所属场景；None表示文件重新拥有权威。
    pub scene_id: Option<SceneId>,
    /// 编辑器边界专用修订；同一修订不会重复重建碰撞。
    pub revision: u64,
    /// 当前完整作者草稿；绘制中的开放路径允许即时产生已有线段。
    pub document: Option<Arc<SceneBoundariesAsset>>,
}

impl DynamicSceneBoundaryOverride {
    /// 发布一份与当前F12边界修订绑定的未保存覆盖。
    pub fn publish(&mut self, scene_id: SceneId, revision: u64, document: SceneBoundariesAsset) {
        if self.scene_id.as_ref() == Some(&scene_id) && self.revision == revision {
            return; // 稳定帧不得反复触发相同静态碰撞重建。
        }
        self.scene_id = Some(scene_id); // 覆盖只能属于一张动态场景。
        self.revision = revision; // 保存作者专用身份供迟到状态拒收。
        self.document = Some(Arc::new(document)); // 分帧捕获已完成，移交所有权不再次复制整张作者文档。
    }

    /// 当前场景保存完成或草稿不再脏时归还文件权威。
    pub fn clear_for_scene(&mut self, scene_id: &SceneId) {
        if self.scene_id.as_ref() != Some(scene_id) {
            return; // 其他场景不能清除当前未保存覆盖。
        }
        self.scene_id = None; // 下一次运行时更新改为读取AssetServer句柄。
        self.revision = self.revision.wrapping_add(1); // 即使清除也必须产生可观察的新身份。
        self.document = None; // 释放作者草稿副本。
    }
}

/// 动态场景边界唯一写侧状态。
#[derive(Resource, Debug)]
pub(in crate::scenes::dynamic_scene) struct DynamicSceneBoundaryState {
    pub(super) generation: u64, // 所有来源事件共同递增，拒绝迟到结果。
    pub(super) derivation: super::derivation::Derivation, // single-flight槽不随离图丢弃。
    /// 目录、资产事件或F12覆盖变化后请求重新核对。
    pub(super) dirty: bool,
    /// 当前正在等待或已经采用的场景身份。
    pub(super) desired_scene_id: Option<SceneId>,
    /// 已安装完整碰撞所属场景。
    pub(super) installed_scene_id: Option<SceneId>,
    /// 当前完整空气墙根实体。
    pub(super) root: Option<Entity>,
    /// 上次观察到的场景目录修订。
    pub(super) catalog_revision: u64,
    /// 上次观察到的F12覆盖场景身份。
    pub(super) override_scene_id: Option<SceneId>,
    /// 上次观察到的F12覆盖修订。
    pub(super) override_revision: u64,
    /// 当前完整碰撞来自F12覆盖还是文件资产。
    pub(super) installed_from_override: bool,
}

impl DynamicSceneBoundaryState {
    // 完整子场景准备席位必须计入边界派生，不能在热更时提前归还并发额度。
    pub(in crate::scenes::dynamic_scene) fn background_busy(&self) -> bool { self.derivation.background_busy() }
    pub(in crate::scenes::dynamic_scene) fn background_finished(&self) -> bool { self.derivation.background_finished() }
}

impl Default for DynamicSceneBoundaryState {
    fn default() -> Self {
        Self {
            generation: 0,
            derivation: default(),
            dirty: true,                    // 首次进入场景必须主动核对目录。
            desired_scene_id: None,           // OnEnter从ActiveDynamicScene取得真正目标。
            installed_scene_id: None,         // 尚未安装任何碰撞。
            root: None,                     // 尚未创建原子替换根。
            catalog_revision: 0,            // 第一次目录应用必然形成变化。
            override_scene_id: None,          // 默认没有F12覆盖。
            override_revision: 0,           // 默认覆盖修订为零。
            installed_from_override: false, // 初始文件与覆盖都未拥有当前结果。
        }
    }
}
