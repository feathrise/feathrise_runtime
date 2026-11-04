//! NVIDIA DLSS 正式画质模式及其与输出分辨率无关的纯数据规则。

use bevy::prelude::UVec2; // Auto 解析只依赖相机最终输出的物理像素尺寸。

/// 玩家可持久化的 DLSS 模式；`Auto` 会在运行时解析成明确质量档或关闭。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DlssMode {
    /// 完全关闭 DLSS Super Resolution 与 Ray Reconstruction。
    #[default]
    Off,
    /// 按最终输出像素量选择明确档位，低于 1080p 时关闭。
    Auto,
    /// 优先画质的超分档位。
    Quality,
    /// 画质与性能平衡档位。
    Balanced,
    /// 优先性能的超分档位。
    Performance,
    /// 面向 4K 及更高输出的超性能档位。
    UltraPerformance,
    /// 原生分辨率时间重建与抗锯齿，不执行空间放大。
    Dlaa,
}

impl DlssMode {
    /// 设置界面固定使用的正式顺序。
    pub const ALL: [Self; 7] = [
        Self::Off,              // 关闭必须始终位于第一项。
        Self::Auto,             // Auto 紧随关闭，作为支持设备的首次安装默认。
        Self::Quality,          // 质量档优先于其余空间放大档。
        Self::Balanced,         // 平衡档位于质量与性能之间。
        Self::Performance,      // 性能档用于高于 1440p、低于 4K 的 Auto 输出。
        Self::UltraPerformance, // 超性能仅在玩家明确选择或 Auto 解析到 4K 时生效。
        Self::Dlaa,             // DLAA 放在最后，明确它不是空间放大档。
    ];

    /// 返回持久化使用的稳定 PascalCase 名称。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",                           // 关闭的稳定 JSON 名称。
            Self::Auto => "Auto",                         // 自动档的稳定 JSON 名称。
            Self::Quality => "Quality",                   // 质量档的稳定 JSON 名称。
            Self::Balanced => "Balanced",                 // 平衡档的稳定 JSON 名称。
            Self::Performance => "Performance",           // 性能档的稳定 JSON 名称。
            Self::UltraPerformance => "UltraPerformance", // 超性能档的稳定 JSON 名称。
            Self::Dlaa => "Dlaa",                         // DLAA 的稳定 JSON 名称。
        }
    }

    /// 从宽松配置名称读取模式；未知值安全回退为关闭。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Auto" => Self::Auto,                         // 恢复自动档请求。
            "Quality" => Self::Quality,                   // 恢复质量档请求。
            "Balanced" => Self::Balanced,                 // 恢复平衡档请求。
            "Performance" => Self::Performance,           // 恢复性能档请求。
            "UltraPerformance" => Self::UltraPerformance, // 恢复超性能档请求。
            "Dlaa" | "DLAA" => Self::Dlaa,                // 兼容人工配置常见全大写写法。
            _ => Self::Off,                               // 关闭、空值和非法值都采用安全回退。
        }
    }

    /// 返回设置界面使用的中文短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关闭",                // 玩家明确关闭所有 DLSS 路径。
            Self::Auto => "Auto",               // 保留 NVIDIA 通用产品命名。
            Self::Quality => "质量",            // 中文质量档。
            Self::Balanced => "平衡",           // 中文平衡档。
            Self::Performance => "性能",        // 中文性能档。
            Self::UltraPerformance => "超性能", // 中文超性能档。
            Self::Dlaa => "DLAA",               // 厂商正式缩写不翻译。
        }
    }

    /// 判断请求是否允许挂接任一 DLSS 组件。
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off) // 只有明确关闭禁止 SR 与 RR。
    }

    /// 按正式设置界面顺序循环，供 F1 兼容快捷入口复用唯一持久请求。
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::Auto,             // 关闭后首先进入硬件感知自动档。
            Self::Auto => Self::Quality,         // 自动档后进入明确质量档。
            Self::Quality => Self::Balanced,     // 质量后进入平衡档。
            Self::Balanced => Self::Performance, // 平衡后进入性能档。
            Self::Performance => Self::UltraPerformance, // 性能后进入超性能档。
            Self::UltraPerformance => Self::Dlaa, // 超性能后进入原生DLAA。
            Self::Dlaa => Self::Off,             // 最后回到完全关闭。
        }
    }

    /// 把 Auto 按最终输出像素量解析为当前封装真正接收的明确模式。
    pub fn resolve_for_output(self, output: UVec2) -> Self {
        if self != Self::Auto {
            return self; // 手动档位保持玩家选择，不再二次映射。
        }
        let pixels = u64::from(output.x) * u64::from(output.y); // 使用 u64 防止异常大尺寸乘法溢出。
        if pixels < 1_920_u64 * 1_080_u64 {
            Self::Off // 低于标准 1080p 时 Auto 不承担额外时间重建成本。
        } else if pixels <= 2_560_u64 * 1_440_u64 {
            Self::Quality // 1080p 到 1440p（含边界）优先质量。
        } else if pixels < 3_840_u64 * 2_160_u64 {
            Self::Performance // 高于 1440p 且低于 4K 使用性能档。
        } else {
            Self::UltraPerformance // 4K 及以上固定使用超性能档。
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 直接核对公开模式顺序、迁移和 Auto 边界。

    #[test]
    fn auto_resolution_boundaries_are_exact() {
        assert_eq!(
            DlssMode::Auto.resolve_for_output(UVec2::new(1919, 1080)),
            DlssMode::Off
        ); // 低于 1080p 像素阈值关闭。
        assert_eq!(
            DlssMode::Auto.resolve_for_output(UVec2::new(1920, 1080)),
            DlssMode::Quality
        ); // 1080p 边界进入质量档。
        assert_eq!(
            DlssMode::Auto.resolve_for_output(UVec2::new(2560, 1440)),
            DlssMode::Quality
        ); // 1440p 边界仍为质量档。
        assert_eq!(
            DlssMode::Auto.resolve_for_output(UVec2::new(2561, 1440)),
            DlssMode::Performance
        ); // 首个高于 1440p 像素量进入性能档。
        assert_eq!(
            DlssMode::Auto.resolve_for_output(UVec2::new(3840, 2160)),
            DlssMode::UltraPerformance
        ); // 4K 边界进入超性能档。
    }

    #[test]
    fn storage_names_round_trip_and_invalid_values_close() {
        for mode in DlssMode::ALL {
            assert_eq!(DlssMode::from_storage_name(mode.storage_name()), mode); // 每个正式模式必须无损往返。
        }
        assert_eq!(
            DlssMode::from_storage_name("FrameGeneration"),
            DlssMode::Off
        ); // 未支持能力不得从手改配置启用。
    }
}
