//! 跨厂商固定 2× 帧生成请求模式。
//!
//! 【功能设计】设置只保存玩家希望下次启动取得的交换链所有者。DLSS-G 与 FSR 3
//! Frame Generation 都需要在 Vulkan 初始化前安装代理，因此运行中切换不会热替换交换链。

/// 玩家选择的固定 2× 帧生成后端。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FrameGenerationMode {
    #[default]
    Off, // 保持单次原生 Present，不加载任何帧生成交换链代理。
    Dlss2x,  // NVIDIA Streamline DLSS-G 固定生成一张中间帧。
    Fsr3_2x, // AMD FidelityFX FSR 3.1 固定生成一张中间帧。
}

impl FrameGenerationMode {
    /// 设置界面使用的稳定顺序；构建未包含的后端由 UI 过滤。
    pub const ALL: [Self; 3] = [Self::Off, Self::Dlss2x, Self::Fsr3_2x];

    /// 返回面向玩家的后端名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::Dlss2x => "NVIDIA DLSS 2×",
            Self::Fsr3_2x => "AMD FSR 3 2×（候选）",
        }
    }

    /// 返回 `settings.json` 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Dlss2x => "Dlss2x",
            Self::Fsr3_2x => "Fsr3_2x",
        }
    }

    /// 从持久化值解析后端；未知值必须安全回退为关闭。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Dlss2x" => Self::Dlss2x,
            "Fsr3_2x" => Self::Fsr3_2x,
            _ => Self::Off,
        }
    }

    /// 返回当前请求是否会改变交换链呈现节奏。
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_names_round_trip_and_unknown_values_disable_frame_generation() {
        for mode in FrameGenerationMode::ALL {
            assert_eq!(
                FrameGenerationMode::from_storage_name(mode.storage_name()),
                mode
            );
        }
        assert_eq!(
            FrameGenerationMode::from_storage_name("FutureBackend"),
            FrameGenerationMode::Off
        );
    }
}
