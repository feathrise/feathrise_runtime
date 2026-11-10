//! 编辑器写入、SDK 启动和导出共同使用的工程启动场景文档。

use crate::SceneId;
use serde::{Deserialize, Deserializer, Serialize};

pub const SCENE_STARTUP_FILE_NAME: &str = "startup.scene.json";
pub const SCENE_STARTUP_FORMAT_VERSION: u32 = 2;
pub const MAX_SCENE_STARTUP_BYTES: usize = 64 * 1024;

/// 单独保存工程启动项；修改选择无需改写目录、场景源或工程描述文件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneStartup {
    format: u32,
    project_id: String,
    pub revision: u64,
    #[serde(default, deserialize_with = "present_scene_id", skip_serializing_if = "Option::is_none")]
    startup_scene_id: Option<SceneId>,
}

fn present_scene_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<SceneId>, D::Error> {
    // 无选择时省略字段；有字段时必须是有效身份，null 不能让坏设置回退到首个场景。
    SceneId::deserialize(deserializer).map(Some)
}

impl SceneStartup {
    /// 编辑器、导出和独立启动直接解析同一份原文，重复键不能被中间值折叠。
    pub fn parse(bytes: &[u8], project_id: &str) -> Result<Self, String> {
        if bytes.len() > MAX_SCENE_STARTUP_BYTES {
            return Err("启动设置超过64KiB上限".into());
        }
        let document: Self =
            serde_json::from_slice(bytes).map_err(|error| format!("启动设置JSON无效：{error}"))?;
        document.validate(project_id)?;
        Ok(document)
    }

    pub fn empty(project_id: &str) -> Self {
        Self {
            format: SCENE_STARTUP_FORMAT_VERSION,
            project_id: project_id.into(),
            revision: 1,
            startup_scene_id: None,
        }
    }

    pub fn selected(&self) -> Option<&str> {
        self.startup_scene_id.as_ref().map(SceneId::as_str)
    }

    pub fn select_scene(&mut self, id: &str) -> Result<bool, String> {
        let id = SceneId::parse(id)?;
        if self.startup_scene_id.as_ref() == Some(&id) {
            return Ok(false);
        }
        // 先检查修订号，溢出时保留原启动项和修订身份。
        let revision = self.revision.checked_add(1).ok_or("启动设置修订已达上限")?;
        self.startup_scene_id = Some(id);
        self.revision = revision;
        Ok(true)
    }

    pub fn validate(&self, project_id: &str) -> Result<(), String> {
        if self.format != SCENE_STARTUP_FORMAT_VERSION {
            return Err("不支持的启动场景设置格式".into());
        }
        if self.project_id != project_id {
            return Err("启动场景设置不属于当前工程".into());
        }
        if self.revision == 0 {
            return Err("启动设置修订不能为零".into());
        }
        if let Some(id) = self.selected() {
            SceneId::parse(id)?;
        }
        Ok(())
    }
}
