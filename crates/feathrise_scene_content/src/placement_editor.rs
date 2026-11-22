//! 显式摆放的编辑信息及可选姿态余量，旧摆放沿用现有 Y 轴字段。

use bevy::math::{Quat, Vec4};
use serde::{Deserialize, Serialize};

/// 名称和组名按字符计限，兼容中文；不允许控制字符进入树行与作者文件。
pub const MAX_PLACEMENT_EDITOR_NAME_CHARS: usize = 128;

/// 可选的 X/Z 姿态余量。浮点位模式保留元数据原有的精确 Eq，磁盘仍是 XYZW 数组。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "[f32; 4]", into = "[f32; 4]")]
pub struct ScenePlacementRotationOffset([u32; 4]);

impl ScenePlacementRotationOffset {
    pub(crate) fn quat(self) -> Quat {
        Quat::from_array(self.into())
    }
}

impl TryFrom<[f32; 4]> for ScenePlacementRotationOffset {
    type Error = &'static str;

    fn try_from(value: [f32; 4]) -> Result<Self, Self::Error> {
        let value = Vec4::from_array(value);
        // 已归一化的存档只校验，不再改变浮点位，否则同一作者快照每次反序列化都可能改值。
        let unit = if (value.length_squared() - 1.0).abs() <= 4.0 * f32::EPSILON {
            value
        } else {
            value.try_normalize().ok_or("摆放旋转必须是有限的非零四元数")?
        };
        let unit = if unit.w < 0.0 { -unit } else { unit };
        Ok(Self(unit.to_array().map(|value| if value == 0.0 { 0 } else { value.to_bits() })))
    }
}

impl From<ScenePlacementRotationOffset> for [f32; 4] {
    fn from(value: ScenePlacementRotationOffset) -> Self {
        value.0.map(f32::from_bits)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePlacementEditorMetadata {
    /// 常规主 GLB 只写标准节点姿态。旧 JSON 用此可选字段延续未烘焙的完整旋转。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_offset: Option<ScenePlacementRotationOffset>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 同名组在当前场景内共同显示，组是逻辑目录，不改变实体父级或模型源。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// 旧摆放默认参与场景。关闭参与仍保留完整摆放，以便作者重新启用。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

impl ScenePlacementEditorMetadata {
    pub fn is_empty(&self) -> bool {
        self.rotation_offset.is_none()
            && self.name.is_empty()
            && self.group.is_empty()
            && !self.hidden
            && !self.locked
            && !self.disabled
    }

    pub fn selectable(&self) -> bool {
        !self.hidden && !self.locked && self.participates()
    }

    pub const fn supports_participation() -> bool {
        true
    }

    pub const fn participates(&self) -> bool {
        !self.disabled
    }

    pub fn set_participation(&mut self, enabled: bool) {
        self.disabled = !enabled;
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
        let mut value = ScenePlacementEditorMetadata {
            name: "村口路灯".into(),
            group: "村口".into(),
            hidden: true,
            locked: true,
            ..Default::default()
        };
        let text = serde_json::to_string(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<ScenePlacementEditorMetadata>(&text).unwrap(),
            value
        );
        assert!(!value.selectable());
        assert!(value.validate().is_ok());
        assert!(value.participates(), "仅编辑器隐藏和锁定的摆放仍参与正式场景");
        value.set_participation(false);
        let decoded: ScenePlacementEditorMetadata =
            serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert!(!decoded.participates());
        assert_eq!(decoded, value);
        let legacy: ScenePlacementEditorMetadata = serde_json::from_str("{}").unwrap();
        assert!(legacy.is_empty());
        assert!(legacy.participates());
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
