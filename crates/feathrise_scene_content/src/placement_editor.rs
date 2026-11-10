//! 显式摆放对象的编辑器名称、逻辑分组和保护状态；玩法不消费显隐与锁定。

use serde::{Deserialize, Serialize};

/// 名称和组名按字符计限，兼容中文；不允许控制字符进入树行与作者文件。
pub const MAX_PLACEMENT_EDITOR_NAME_CHARS: usize = 128;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePlacementEditorMetadata {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 同名组在当前场景内共同显示，组是逻辑目录，不改变实体父级或模型源。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

impl ScenePlacementEditorMetadata {
    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.group.is_empty() && !self.hidden && !self.locked
    }

    pub fn selectable(&self) -> bool {
        !self.hidden && !self.locked
    }

    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [("对象名称", &self.name), ("对象组名", &self.group)] {
            if value.chars().count() > MAX_PLACEMENT_EDITOR_NAME_CHARS
                || value.chars().any(char::is_control)
            {
                return Err(format!("{label}最多128个字符，且不能包含控制字符"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_chinese_and_editor_only_protection() {
        let value = ScenePlacementEditorMetadata {
            name: "村口路灯".into(),
            group: "村口".into(),
            hidden: true,
            locked: true,
        };
        let text = serde_json::to_string(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<ScenePlacementEditorMetadata>(&text).unwrap(),
            value
        );
        assert!(!value.selectable());
        assert!(value.validate().is_ok());
        assert!(
            serde_json::from_str::<ScenePlacementEditorMetadata>("{}")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn names_use_character_limits_and_reject_multiline_values() {
        let mut value = ScenePlacementEditorMetadata {
            name: "灯".repeat(128),
            ..Default::default()
        };
        assert!(value.validate().is_ok());
        value.name.push('灯');
        assert!(value.validate().is_err());
        value.name = "屋顶\n模型".into();
        assert!(value.validate().is_err());
    }
}
