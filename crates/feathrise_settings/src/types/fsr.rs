//! AMD FSR 3 超分持久模式及其纯数据规则。

/// 玩家可持久化的 FSR 3 时间域超分模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FsrMode {
    #[default]
    Off, // 完全关闭 FSR 时间域超分。
    Quality,          // 1.5x 线性放大，约 67% 内部尺寸。
    Balanced,         // 1.7x 线性放大，约 59% 内部尺寸。
    Performance,      // 2.0x 线性放大，50% 内部尺寸。
    UltraPerformance, // 3.0x 线性放大，约 33% 内部尺寸。
}

impl FsrMode {
    /// 设置界面固定顺序；关闭始终在第一项。
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Quality,
        Self::Balanced,
        Self::Performance,
        Self::UltraPerformance,
    ]; // 新模式必须同步补持久化、运行映射与测试。

    /// 返回 settings.json 使用的稳定 PascalCase 名称。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Quality => "Quality",
            Self::Balanced => "Balanced",
            Self::Performance => "Performance",
            Self::UltraPerformance => "UltraPerformance",
        }
    }

    /// 从持久化名称读取模式；未知值安全回退关闭。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Quality" => Self::Quality,
            "Balanced" => Self::Balanced,
            "Performance" => Self::Performance,
            "UltraPerformance" => Self::UltraPerformance,
            _ => Self::Off,
        }
    }

    /// 返回设置界面的中文短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::Quality => "质量",
            Self::Balanced => "平衡",
            Self::Performance => "性能",
            Self::UltraPerformance => "超性能",
        }
    }

    /// 返回设置说明使用的近似内部分辨率百分比。
    pub const fn render_percentage(self) -> u8 {
        match self {
            Self::Off => 100,
            Self::Quality => 67,
            Self::Balanced => 59,
            Self::Performance => 50,
            Self::UltraPerformance => 33,
        }
    }

    /// 判断是否请求任一 FSR 超分档。
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off) // 只有关闭不取得相机和 GPU 历史所有权。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 纯数据测试不初始化 Bevy RenderApp。

    #[test]
    fn storage_names_round_trip_and_invalid_values_close() {
        for mode in FsrMode::ALL {
            assert_eq!(FsrMode::from_storage_name(mode.storage_name()), mode); // 五项必须无损往返。
        }
        assert_eq!(FsrMode::from_storage_name("Auto"), FsrMode::Off); // 未支持的自动档不得被手改配置启用。
    }

    #[test]
    fn display_percentages_follow_fixed_fsr_ratios() {
        assert_eq!(FsrMode::Quality.render_percentage(), 67); // 1/1.5 约等于 67%。
        assert_eq!(FsrMode::Balanced.render_percentage(), 59); // 1/1.7 约等于 59%。
        assert_eq!(FsrMode::Performance.render_percentage(), 50); // 1/2 精确等于 50%。
        assert_eq!(FsrMode::UltraPerformance.render_percentage(), 33); // 1/3 约等于 33%。
    }
}
