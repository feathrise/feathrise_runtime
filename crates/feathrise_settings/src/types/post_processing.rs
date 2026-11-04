// 游戏设置 — 独立后处理与体积光质量档位。

/// 窗口目标 3D 相机使用的 Bloom 后处理档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum BloomQuality {
    #[default]
    Off, // 保持当前 LDR、无 Bloom 基线。
    Low,  // Natural 曲线、0.08 强度、最大 mip 256。
    High, // Natural 曲线、0.15 强度、最大 mip 512。
}

impl BloomQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 3] = [Self::Off, Self::Low, Self::High];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关",
            Self::Low => "低",
            Self::High => "高",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::High => "High",
        }
    }

    /// 从持久化枚举名读取档位，未知值安全关闭 Bloom。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "High" => Self::High,
            _ => Self::Off,
        }
    }
}

/// 窗口目标 3D 相机使用的对比度自适应锐化（CAS）档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SharpeningQuality {
    #[default]
    Off, // 不覆盖相机作者锐化状态，保持功能加入前的画面基线。
    Low,  // 使用 0.25 强度，轻度补回后处理抗锯齿损失的细节。
    High, // 使用 0.60 强度，采用 Bevy CAS 的标准清晰度。
}

impl SharpeningQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 3] = [Self::Off, Self::Low, Self::High];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关",
            Self::Low => "低",
            Self::High => "高",
        }
    }

    /// 返回启用档位对应的 CAS 强度，关闭时不构造全局覆盖参数。
    pub const fn sharpening_strength(self) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Low => Some(0.25),
            Self::High => Some(0.60),
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::High => "High",
        }
    }

    /// 从持久化枚举名读取档位，未知值安全关闭全局锐化。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "High" => Self::High,
            _ => Self::Off,
        }
    }
}

/// 窗口目标 3D 主场景的固定内部渲染比例；UI 与纹理目标相机始终保持原生分辨率。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RenderScaleQuality {
    Performance, // 67%，优先降低主场景像素着色成本。
    Balanced,    // 75%，在性能与清晰度之间取中间值。
    Quality,     // 85%，以较小清晰度损失换取温和性能收益。
    #[default]
    Native, // 100%，保持功能加入前的原生分辨率画面基线。
}

impl RenderScaleQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 4] = [
        Self::Performance,
        Self::Balanced,
        Self::Quality,
        Self::Native,
    ];

    /// 返回面向玩家的实际内部渲染比例。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Performance => "67%",
            Self::Balanced => "75%",
            Self::Quality => "85%",
            Self::Native => "100%",
        }
    }

    /// 返回计算主场景物理像素尺寸使用的比例系数。
    pub const fn scale_factor(self) -> f32 {
        match self {
            Self::Performance => 0.67,
            Self::Balanced => 0.75,
            Self::Quality => 0.85,
            Self::Native => 1.0,
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Performance => "Performance",
            Self::Balanced => "Balanced",
            Self::Quality => "Quality",
            Self::Native => "Native",
        }
    }

    /// 从持久化枚举名读取档位，未知值安全回退到原生分辨率。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Performance" => Self::Performance,
            "Balanced" => Self::Balanced,
            "Quality" => Self::Quality,
            _ => Self::Native,
        }
    }
}

/// 玩法主相机使用的体积雾与天体光束光线步进质量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VolumetricFogQuality {
    #[default]
    Off, // 保持功能加入前只有距离雾、没有体积光计算的画面基线。
    Low,  // 使用 32 个光线步进，优先控制全屏像素成本。
    High, // 使用 Bevy 默认 64 个光线步进，降低雾带分层。
}

impl VolumetricFogQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 3] = [Self::Off, Self::Low, Self::High];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关",
            Self::Low => "低",
            Self::High => "高",
        }
    }

    /// 返回启用档位的光线步进数；关闭时不创建体积雾渲染视图。
    pub const fn raymarch_steps(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::Low => Some(32),
            Self::High => Some(64),
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::High => "High",
        }
    }

    /// 从持久化枚举名读取档位，未知值安全关闭体积雾。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "High" => Self::High,
            _ => Self::Off,
        }
    }
}
