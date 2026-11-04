//! F1空气墙辅助显示的三档策略，不改变碰撞及F12独立编辑显示。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AirWallDisplayMode {
    #[default]
    Hidden, // 游戏画面不显示空气墙辅助网格。
    Full,   // 显示全部空气墙，不受单墙visible影响。
    Editor, // 按编辑器样式画贴地中心线和厚度轮廓，并遵循visible。
}

impl AirWallDisplayMode {
    /// F1按钮使用固定三档循环，不与地图规划或碰撞开关串联。
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Full,
            Self::Full => Self::Editor,
            Self::Editor => Self::Hidden,
        }
    }

    /// 当前档位的用户可见文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "不显示",
            Self::Full => "显示完整",
            Self::Editor => "按照编辑器显示",
        }
    }
}
