//! 通用工具工作台消息；字段、动作与业务ID由插件拥有，宿主不识别装备或技能类型。
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE_BYTES: usize = 512 * 1024;
pub const MAX_PLUGINS: usize = 16;
pub const MAX_TABS: usize = 32;
pub const MAX_ROWS: usize = 64 * 6;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    pub plugin_id: String,
    pub project_id: String,
    pub project_root: String,
    pub asset_root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Open,
    /// 场景领域沿用场景的草稿身份和统一保存，领域正文由插件拥有。
    SceneDomain {
        request: crate::scene_domain::SceneDomainRequest,
    },
    /// 目录窗口按整行偏移取数，保留连续滚动而非强制分页。
    Browse {
        offset: u32,
        columns: u8,
    },
    Activate {
        document: u32,
    },
    Close {
        document: u32,
        discard: bool,
    },
    Action {
        document: u32,
        action: String,
        value: String,
    },
    Edit {
        document: u32,
        field: String,
        value: String,
    },
    Save {
        document: u32,
    },
    Search {
        value: String,
        page: u32,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub plugin_id: String,
    pub tool: String,
    pub revision: u64,
    pub active: u32,
    pub tabs: Vec<Document>,
    pub sections: Vec<Choice>,
    pub section: String,
    pub actions: Vec<Action>,
    pub fields: Vec<Field>,
    pub rows: Vec<Choice>,
    pub search: String,
    pub page: u32,
    pub pages: u32,
    pub status: String,
    pub error: Option<String>,
    pub preview: Option<Preview>,
    pub clipboard: Option<String>,
    /// 可选工作台布局；旧插件缺省时仍使用通用工具页。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workbench: Option<Workbench>,
    /// 时间轴属于插件文档；宿主只呈现轨道和手势，不解释任何游戏技能类型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<Timeline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_domain: Option<crate::scene_domain::SceneDomainSnapshot>,
    /// 用户明确点击产生的跨工具交接；宿主核对当前来源页后，仅向已安装目标提交公开字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<ToolHandoff>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolHandoff {
    pub target: String,
    pub edits: Vec<HandoffEdit>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffEdit {
    pub field: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub id: u32,
    pub title: String,
    pub dirty: bool,
    /// 真实原子保存成功才增加；宿主不能以请求已提交代替保存回执。
    pub saved_sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub value: String,
    pub kind: FieldKind,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldKind {
    Text {
        multiline: bool,
        readonly: bool,
    },
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    /// 有符号小数与原u32合同分开，避免技能时长、位移和倍率被静默截断。
    Decimal {
        min: f64,
        max: f64,
        step: f64,
        precision: u8,
    },
    Choice {
        options: Vec<Choice>,
    },
    ChoiceButtons {
        options: Vec<Choice>,
    },
    Picker {
        action: String,
    },
    Label,
    Asset {
        extensions: Vec<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub revision: u64,
    pub model: Option<String>,
    pub icon: Option<String>,
    pub details: String,
    /// 领域草稿只交给工程运行端解释；SDK定义的中性作者prefab kind可由编辑器本地显示。
    /// 两条路径均不允许宿主或SDK反向链接游戏类型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimePreview>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePreview {
    pub revision: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// 所有时间单位为秒；条块ID由插件稳定分配，拖动结束后才生成一条作者历史。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timeline {
    pub duration: f64,
    pub cursor: f64,
    pub playing: bool,
    pub zoom: f64,
    #[serde(default)]
    pub pan: f64,
    /// 插件可声明带最小观察窗口的准确可见跨度；缺省沿用 duration / zoom。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<f64>,
    pub lanes: Vec<TimelineLane>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineLane {
    pub id: String,
    pub label: String,
    pub blocks: Vec<TimelineBlock>,
    #[serde(default, skip_serializing_if = "TimelineLanePresentation::is_default")]
    pub presentation: TimelineLanePresentation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineBlock {
    pub id: String,
    pub label: String,
    pub start: f64,
    pub end: f64,
    pub selected: bool,
    #[serde(default, skip_serializing_if = "TimelineBlockPresentation::is_default")]
    pub presentation: TimelineBlockPresentation,
}

/// 只描述通用画布外观与已有时间轴动作入口，不携带领域轨道类型。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimelineLanePresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "timeline_false")]
    pub selected: bool,
    #[serde(skip_serializing_if = "timeline_false")]
    pub select: bool,
    #[serde(skip_serializing_if = "timeline_false")]
    pub add: bool,
}
impl TimelineLanePresentation {
    fn is_default(&self) -> bool { self == &Self::default() }
}

/// 参考细带通过稳定主条块身份跟随分层；只读参考永远不能成为第二个拖动目标。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimelineBlockPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "timeline_false")]
    pub point: bool,
    #[serde(skip_serializing_if = "timeline_false")]
    pub read_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_of: Option<String>,
    #[serde(skip_serializing_if = "timeline_zero_band")]
    pub reference_band: u8,
    #[serde(skip_serializing_if = "timeline_true")]
    pub resize_start: bool,
    #[serde(skip_serializing_if = "timeline_true")]
    pub resize_end: bool,
    #[serde(skip_serializing_if = "timeline_false")]
    pub open_end: bool,
}
impl Default for TimelineBlockPresentation {
    fn default() -> Self {
        Self { color: None, point: false, read_only: false, reference_of: None,
            reference_band: 0, resize_start: true, resize_end: true, open_end: false }
    }
}
impl TimelineBlockPresentation {
    fn is_default(&self) -> bool { self == &Self::default() }
}
fn timeline_false(value: &bool) -> bool { !value }
fn timeline_true(value: &bool) -> bool { *value }
fn timeline_zero_band(value: &u8) -> bool { *value == 0 }

/// 插件声明文档布局和动作身份，宿主不依赖任何玩法类型。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workbench {
    pub library_title: String,
    pub title: String,
    pub badge: String,
    pub primary: Vec<String>,
    pub menu: Vec<String>,
    pub menu_open: bool,
    pub groups: Vec<FieldGroup>,
    pub catalog: Option<CatalogView>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldGroup {
    pub title: String,
    pub help: String,
    pub fields: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogView {
    pub title: String,
    #[serde(default)]
    pub help: String,
    pub action: String,
    pub total: u32,
    pub offset: u32,
    pub columns: u8,
}

impl Snapshot {
    /// 跨库返回的不可信长度在进入主线程前受限；全目录永远不作为UI快照传递。
    pub fn validate(&self, expected: &str) -> Result<(), String> {
        if self.plugin_id != expected || self.tool.is_empty() || self.tool.len() > 128 {
            return Err("插件自报身份或工具名称不匹配".into());
        }
        if self.tabs.len() > MAX_TABS
            || self.rows.len() > MAX_ROWS
            || self.fields.len() > 64
            || self.sections.len() > 16
            || self.actions.len() > 32
        {
            return Err("插件工作台超过界面预算".into());
        }
        let mut ids = std::collections::HashSet::new();
        if self
            .tabs
            .iter()
            .any(|tab| tab.id == 0 || !ids.insert(tab.id))
        {
            return Err("插件文档身份重复或使用保留的目录身份".into());
        }
        if self.active != 0 && !ids.contains(&self.active) {
            return Err("插件活动文档不在打开集合中".into());
        }
        let short = |s: &str| s.len() <= 1024 && !s.contains('\0');
        let text = |s: &str| s.len() <= 16 * 1024 && !s.contains('\0');
        let choices = |list: &[Choice]| {
            let mut ids = std::collections::HashSet::new();
            list.len() <= MAX_ROWS
                && list.iter().all(|c| {
                    !c.id.is_empty() && short(&c.id) && short(&c.label) && ids.insert(&c.id)
                })
        };
        if !choices(&self.sections)
            || !choices(&self.rows)
            || !short(&self.search)
            || !text(&self.status)
            || self.error.as_deref().is_some_and(|s| !text(s))
            || self.tabs.iter().any(|t| !short(&t.title))
            || self
                .actions
                .iter()
                .any(|a| !short(&a.id) || !short(&a.label))
        {
            return Err("插件标签、搜索或消息超出预算".into());
        }
        let mut fields = std::collections::HashSet::new();
        for field in &self.fields {
            if field.id.is_empty()
                || !short(&field.id)
                || !short(&field.label)
                || !text(&field.value)
                || !fields.insert(&field.id)
            {
                return Err("插件字段身份或长度无效".into());
            }
            match &field.kind {
                FieldKind::Picker { action } if action.is_empty() || !short(action) => {
                    return Err("插件选择器动作无效".into());
                }
                FieldKind::Choice { options } | FieldKind::ChoiceButtons { options }
                    if !choices(options) =>
                {
                    return Err("插件选项超出预算或身份重复".into());
                }
                // V1专业数值框限定u32整数，避免宿主偷偷截断小数；小数控件应扩展协议后接入。
                FieldKind::Number { min, max, step }
                    if ![min, max, step]
                        .iter()
                        .all(|v| v.is_finite() && v.fract() == 0.)
                        || *min < 0.
                        || *max > u32::MAX as f64
                        || min > max
                        || *step <= 0.
                        || field.value.parse::<u32>().is_err() =>
                {
                    return Err("插件V1数字字段必须是有界u32整数".into());
                }
                FieldKind::Decimal {
                    min,
                    max,
                    step,
                    precision,
                } if ![min, max, step].iter().all(|v| v.is_finite())
                    || min > max
                    || *step <= 0.
                    || *precision > 9
                    || field
                        .value
                        .parse::<f64>()
                        .map_or(true, |v| !v.is_finite() || v < *min || v > *max) =>
                {
                    return Err("插件小数字段必须有有限范围、步长和有效数值".into());
                }
                FieldKind::Asset { extensions }
                    if extensions.len() > 8
                        || extensions.iter().any(|s| {
                            s.len() > 16 || !s.bytes().all(|b| b.is_ascii_alphanumeric())
                        }) =>
                {
                    return Err("插件资源扩展名无效".into());
                }
                _ => {}
            }
        }
        if let Some(view) = &self.workbench {
            let actions: std::collections::HashSet<_> =
                self.actions.iter().map(|a| a.id.as_str()).collect();
            if !short(&view.library_title)
                || !short(&view.title)
                || !short(&view.badge)
                || view.primary.len() > 16
                || view.menu.len() > 16
                || view.groups.len() > 16
                || view
                    .primary
                    .iter()
                    .chain(&view.menu)
                    .any(|id| !actions.contains(id.as_str()))
                || view.groups.iter().any(|g| {
                    !short(&g.title)
                        || !text(&g.help)
                        || g.fields.len() > 64
                        || g.fields.iter().any(|id| !fields.contains(id))
                })
            {
                return Err("插件工作台布局引用无效或超出预算".into());
            }
            if let Some(list) = &view.catalog {
                if !(1..=6).contains(&list.columns)
                    || !short(&list.title)
                    || !text(&list.help)
                    || !short(&list.action)
                    || list.action.is_empty()
                    || list.offset > list.total
                    || !list.offset.is_multiple_of(list.columns as u32)
                    || self.rows.len() > 64 * list.columns as usize
                    || self.rows.len() as u64 + list.offset as u64 > list.total as u64
                {
                    return Err("插件连续目录窗口无效".into());
                }
            }
        }
        if let Some(preview) = &self.preview {
            if !text(&preview.details)
                || preview
                    .model
                    .iter()
                    .chain(preview.icon.iter())
                    .any(|p| !asset_path(p))
            {
                return Err("插件预览路径必须是资产根内的相对路径".into());
            }
            if let Some(runtime) = &preview.runtime {
                if runtime.revision == 0
                    || runtime.kind.is_empty()
                    || !short(&runtime.kind)
                    || serde_json::to_vec(&runtime.payload)
                        .map_or(true, |bytes| bytes.len() > 384 * 1024)
                {
                    return Err("插件运行预览身份或正文超过预算".into());
                }
            }
        }
        if let Some(timeline) = &self.timeline {
            let mut lanes = std::collections::HashSet::new();
            let mut blocks = std::collections::HashSet::new();
            let span = timeline.span.unwrap_or(timeline.duration / timeline.zoom);
            let color_valid = |color: Option<[f32; 4]>| color.is_none_or(|rgba| {
                rgba.into_iter().all(|channel| channel.is_finite() && (0.0..=1.0).contains(&channel))
            });
            if !timeline.duration.is_finite()
                || timeline.duration <= 0.
                || !timeline.cursor.is_finite()
                || timeline.cursor < 0.
                || timeline.cursor > timeline.duration
                || !timeline.zoom.is_finite()
                || !(0.1..=100.).contains(&timeline.zoom)
                || !timeline.pan.is_finite()
                || timeline.pan < 0.
                || timeline.pan > timeline.duration
                // 单个输入有限仍可能在除法中溢出/下溢；宿主只接收可安全归一化的完整窗口。
                || !span.is_finite() || span <= 0.
                || !(timeline.pan + span).is_finite()
                || timeline.lanes.len() > 16
                || timeline.lanes.iter().any(|lane| {
                    lane.id.is_empty()
                        || !short(&lane.id)
                        || !short(&lane.label)
                        || !lanes.insert(&lane.id)
                        || !color_valid(lane.presentation.color)
                        || lane.blocks.iter().any(|block| {
                            block.id.is_empty()
                                || !short(&block.id)
                                || !short(&block.label)
                                || !blocks.insert(&block.id)
                                || !block.start.is_finite()
                                || !block.end.is_finite()
                                || block.start < 0.
                                || block.end < block.start
                                // 参考时序允许超过当前主轴终点，由可见窗口裁剪，不能扩大主轴。
                                || (block.end > timeline.duration && block.presentation.reference_of.is_none())
                                || !color_valid(block.presentation.color)
                                || block.presentation.reference_band > 1
                                || block.presentation.reference_of.as_deref().is_some_and(|id| {
                                    id.is_empty() || !short(id) || !block.presentation.read_only
                                        || !lane.blocks.iter().any(|other| other.id == id
                                            && !other.presentation.read_only)
                                })
                        })
                })
                || blocks.len() > 2048
            {
                return Err("插件时间轴身份、时间范围或可见条块超过预算".into());
            }
        }
        if self.clipboard.as_deref().is_some_and(|s| !text(s)) {
            return Err("插件剪贴板内容超出预算".into());
        }
        if self.handoff.as_ref().is_some_and(|handoff| {
            handoff.target.is_empty()
                || handoff.target.len() > 1024
                || handoff.target.contains('\0')
                || handoff.edits.is_empty()
                || handoff.edits.len() > 8
                || handoff.edits.iter().any(|edit| {
                    edit.field.is_empty()
                        || edit.field.len() > 1024
                        || edit.value.len() > 1024
                        || edit.field.contains('\0')
                        || edit.value.contains('\0')
                })
        }) {
            return Err("跨工具交接目标或字段超出预算".into());
        }
        if let Some(domain) = &self.scene_domain {
            domain.validate()?;
        }
        Ok(())
    }
}

fn asset_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains([':', '\\', '\0'])
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
