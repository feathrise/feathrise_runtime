//! 外部作者的中性显示值；领域负责正式工厂，公共显示层只认识线、完整Scene0和骨骼附件。
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorLines {
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub ground_groups: Vec<AuthorGroundGroup>,
    pub anchors: Vec<AuthorAnchor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorGroundGroup {
    pub position: [f32; 3],
    pub first: usize,
    pub count: usize,
    pub anchor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorAnchor {
    pub id: String,
    pub position: [f32; 3],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorPose {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorPrefab {
    /// None为工程定义的内置胶囊；外部模型保留完整Scene0和原骨架。
    pub model: Option<String>,
    pub transform: AuthorPose,
    pub attachments: Vec<AuthorAttachment>,
    pub idle: Option<AuthorAnimation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorAttachment {
    pub model: String,
    pub parent_node: String,
    pub transform: AuthorPose,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorAnimation {
    pub asset: String,
    pub name: String,
}

/// 编辑器本地作者预览的中性合同；业务插件先完成正式工厂转换，宿主不解析游戏角色正文。
pub const AUTHOR_PREFAB_PREVIEW_KIND: &str = "feathrise.author_prefab.v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorPrefabPreview {
    pub prefab: AuthorPrefab,
    pub playing: bool,
    pub speed: f32,
}
impl AuthorPrefabPreview {
    pub fn validate(&self) -> Result<(), String> {
        self.prefab.validate()?;
        if !self.speed.is_finite() || !(0.05..=4.0).contains(&self.speed) {
            return Err("作者动画预览速度须在0.05至4倍之间".into());
        }
        Ok(())
    }
}

/// 嵌入画布只接收完整后台结果；正式外观payload由安装的游戏领域解释。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorVisualContent {
    pub key: String,
    pub lines: AuthorLines,
    pub runtime: Option<serde_json::Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorVisualPosition {
    pub key: String,
    pub position: Option<[f32; 3]>,
    pub yaw_degrees: f32,
    pub elevation: f32,
}

impl AuthorLines {
    /// 大数组校验仅在后台执行；单线网格和区域锚点必须作为同一完整版本发布。
    pub fn validate(&self) -> Result<(), String> {
        if self.positions.len() != self.colors.len()
            || self.positions.len() % 2 != 0
            || self
                .positions
                .iter()
                .flatten()
                .chain(self.colors.iter().flatten())
                .any(|v| !v.is_finite())
        {
            return Err("作者线网格位置、颜色或线段数量无效".into());
        }
        let mut ids = std::collections::HashSet::new();
        for anchor in &self.anchors {
            if anchor.id.is_empty()
                || anchor.id.len() > 1024
                || !ids.insert(&anchor.id)
                || anchor.position.iter().any(|v| !v.is_finite())
            {
                return Err("作者候选锚点无效".into());
            }
        }
        let mut end = 0;
        for group in &self.ground_groups {
            let next = group
                .first
                .checked_add(group.count)
                .ok_or("作者投射范围溢出")?;
            if group.first < end
                || next > self.positions.len()
                || group.position.iter().any(|v| !v.is_finite())
                || group.anchor.as_ref().is_some_and(|id| !ids.contains(id))
            {
                return Err("作者投射分组无效".into());
            }
            end = next;
        }
        Ok(())
    }
}
impl AuthorPrefab {
    pub fn validate(&self) -> Result<(), String> {
        fn path(path: &str) -> bool {
            !path.is_empty()
                && path.len() <= 1024
                && !path.contains(['\\', ':', '\0', '#'])
                && !path.starts_with('/')
                && path
                    .split('/')
                    .all(|v| !v.is_empty() && v != "." && v != "..")
        }
        fn pose(p: &AuthorPose) -> bool {
            p.translation
                .iter()
                .chain(p.rotation.iter())
                .chain(p.scale.iter())
                .all(|v| v.is_finite())
                && p.scale.iter().all(|v| v.abs() > f32::EPSILON)
                && p.rotation.iter().map(|v| v * v).sum::<f32>() > 0.5
        }
        if self.model.as_ref().is_some_and(|v| !path(v))
            || !pose(&self.transform)
            || self.attachments.len() > 32
            || self.attachments.iter().any(|v| {
                !path(&v.model)
                    || v.parent_node.is_empty()
                    || v.parent_node.len() > 1024
                    || !pose(&v.transform)
            })
            || self
                .idle
                .as_ref()
                .is_some_and(|v| !path(&v.asset) || v.name.is_empty() || v.name.len() > 1024)
        {
            return Err("作者完整模型、附件或动画描述无效".into());
        }
        Ok(())
    }
}
