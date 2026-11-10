//! 应用为现有诊断页提供的预设身份；不参与预设加载、散布、保存或运行时选种。

use bevy::prelude::Resource;

use super::state::LodControlContent;

/// F3/F4 的内容页映射由消费工程显式提供，公共库不内置任何游戏预设 ID。
/// 默认空映射保留完整通用植被运行；编辑器可另外按当前选中作者层提供诊断目标。
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct LodControlPresetBindings {
    profiles: [Option<String>; 9],
}

impl LodControlPresetBindings {
    pub fn bind(&mut self, content: LodControlContent, profile: impl Into<String>) {
        self.profiles[content.slot()] = Some(profile.into());
    }

    pub fn profile(&self, content: LodControlContent) -> Option<&str> {
        self.profiles[content.slot()].as_deref()
    }

    /// 固定九个诊断槽，不随场景像素或实例规模增长。
    pub fn iter(&self) -> impl Iterator<Item = (LodControlContent, &str)> {
        LodControlContent::ALL.into_iter().filter_map(|content| {
            self.profile(content).map(|profile| (content, profile))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_project_ids_are_explicit_and_independent() {
        let mut bindings = LodControlPresetBindings::default();
        assert_eq!(bindings.iter().count(), 0);
        bindings.bind(LodControlContent::OrdinaryGrass, "project/meadow");
        bindings.bind(LodControlContent::Trees, "project/crystal");
        assert_eq!(bindings.profile(LodControlContent::OrdinaryGrass), Some("project/meadow"));
        assert_eq!(bindings.profile(LodControlContent::Trees), Some("project/crystal"));
        assert_eq!(bindings.profile(LodControlContent::Wheat), None);
    }
}
