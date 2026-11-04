// 游戏设置 — 3D 材质纹理过滤质量。

/// StandardMaterial 线性采样纹理使用的各向异性过滤档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum TextureFilteringQuality {
    #[default]
    Off, // 不覆盖纹理作者采样器，严格保持功能加入前的三线性 1× 基线。
    Anisotropic4,  // 四倍各向异性过滤，温和改善斜视表面的纹理清晰度。
    Anisotropic8,  // 八倍各向异性过滤，在清晰度和采样成本间取中间值。
    Anisotropic16, // 十六倍各向异性过滤，优先保持远处地面和墙面的细节。
}

impl TextureFilteringQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 4] = [
        Self::Off,
        Self::Anisotropic4,
        Self::Anisotropic8,
        Self::Anisotropic16,
    ];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::Anisotropic4 => "4×",
            Self::Anisotropic8 => "8×",
            Self::Anisotropic16 => "16×",
        }
    }

    /// 返回采样器使用的各向异性倍数；关闭时恢复纹理作者配置。
    pub const fn anisotropy_clamp(self) -> Option<u16> {
        match self {
            Self::Off => None,
            Self::Anisotropic4 => Some(4),
            Self::Anisotropic8 => Some(8),
            Self::Anisotropic16 => Some(16),
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Anisotropic4 => "Anisotropic4",
            Self::Anisotropic8 => "Anisotropic8",
            Self::Anisotropic16 => "Anisotropic16",
        }
    }

    /// 从持久化枚举名读取档位，未知值不接管纹理作者配置。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Anisotropic4" => Self::Anisotropic4,
            "Anisotropic8" => Self::Anisotropic8,
            "Anisotropic16" => Self::Anisotropic16,
            _ => Self::Off,
        }
    }
}
