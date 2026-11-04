//! 屏幕空间遮蔽档位与预通道兼容规则。

use super::{AntiAliasingMode, GraphicsSettings, RenderScaleQuality}; // 复用画质聚合模型，集中维护 SSAO、接触阴影、TAA 与空间缩放互斥约束。

/// 玩法窗口相机使用的屏幕空间环境光遮蔽质量档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum AmbientOcclusionQuality {
    #[default]
    Off, // 不接管玩法相机 SSAO，严格保持功能加入前的画面与 GPU 成本。
    Low,  // 使用 Bevy Low：每像素 4 个基础样本，适合中低端 GPU。
    High, // 使用 Bevy High：每像素 18 个基础样本，强化缝隙与接触区域层次。
}

impl AmbientOcclusionQuality {
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

    /// 从持久化枚举名读取档位，未知值安全关闭 SSAO。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "High" => Self::High,
            _ => Self::Off,
        }
    }

    /// 返回当前档位是否需要接管玩法相机 SSAO。
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// 玩法窗口相机使用的屏幕空间接触阴影质量档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ContactShadowQuality {
    #[default]
    Off, // 不接管玩法相机接触阴影，保持功能加入前的画面与 GPU 成本。
    Low,  // 使用 8 步、0.20 米短射线，优先控制逐光源屏幕空间步进成本。
    High, // 使用 Bevy 默认 16 步、0.30 米射线，强化脚底与小物体接地细节。
}

impl ContactShadowQuality {
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

    /// 从持久化枚举名读取档位，未知值安全关闭接触阴影。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "High" => Self::High,
            _ => Self::Off,
        }
    }

    /// 返回当前档位是否需要接管玩法相机接触阴影。
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

impl AntiAliasingMode {
    /// 返回当前模式是否使用与 Bevy 0.19 SSAO 不兼容的多重采样。
    pub const fn uses_msaa(self) -> bool {
        matches!(self, Self::Msaa2 | Self::Msaa4 | Self::Msaa8)
    }
}

impl GraphicsSettings {
    /// 以“最后选择生效”规则修改抗锯齿：选择 MSAA 时关闭 SSAO，选择 TAA 时恢复原生渲染比例。
    pub fn set_anti_aliasing(&mut self, value: AntiAliasingMode) {
        self.anti_aliasing = value; // 先采用玩家刚选择的抗锯齿模式。
        if value.uses_msaa() {
            self.ambient_occlusion_quality = AmbientOcclusionQuality::Off; // 禁止把非法组合送入 RenderApp。
        }
        if value == AntiAliasingMode::Taa {
            self.render_scale = RenderScaleQuality::Native; // Bevy 0.19 TAA 不支持读取缩小后的深度与运动矢量区域。
        }
    }

    /// 修改 SSAO；在当前为 MSAA 时自动切到官方推荐的 TAA 单采样路径。
    pub fn set_ambient_occlusion_quality(&mut self, value: AmbientOcclusionQuality) {
        self.ambient_occlusion_quality = value; // 保存玩家刚选择的 SSAO 档位。
        self.normalize_ambient_occlusion_compatibility(); // 同帧消除 Bevy 明确拒绝的多采样组合。
    }

    /// 修改接触阴影；启用时恢复完整深度预通道尺寸，抗锯齿保持玩家原选择。
    pub fn set_contact_shadow_quality(&mut self, value: ContactShadowQuality) {
        self.contact_shadow_quality = value; // 保存玩家最后选择的接触阴影档位。
        if value.is_enabled() {
            self.render_scale = RenderScaleQuality::Native; // Bevy 0.19 接触阴影按完整深度纹理尺寸采样，不能读取局部 viewport。
        }
    }

    /// 修改空间渲染比例；低于 100% 时改用兼容的 FXAA 并关闭依赖完整预通道的屏幕空间效果。
    pub fn set_render_scale(&mut self, value: RenderScaleQuality) {
        self.render_scale = value; // 先采用玩家最后选择的固定内部渲染比例。
        if value != RenderScaleQuality::Native {
            if self.anti_aliasing == AntiAliasingMode::Taa {
                self.anti_aliasing = AntiAliasingMode::Fxaa; // 空间放大后再运行 FXAA，避免错误读取低分辨率 TAA 预通道。
            }
            self.ambient_occlusion_quality = AmbientOcclusionQuality::Off; // SSAO 当前依赖与 TAA 相同的全尺寸深度契约。
            self.contact_shadow_quality = ContactShadowQuality::Off; // 接触阴影按全尺寸深度纹理寻址，局部 viewport 会留下清晰矩形边界。
        }
    }

    /// 归一化手工 JSON 等外部来源产生的预通道非法组合，启用的屏幕空间效果优先保留。
    pub fn normalize_ambient_occlusion_compatibility(&mut self) {
        if self.ambient_occlusion_quality.is_enabled() && self.anti_aliasing.uses_msaa() {
            self.anti_aliasing = AntiAliasingMode::Taa; // TAA 使用 Msaa::Off，并能显著降低 SSAO 噪点。
        }
        if self.ambient_occlusion_quality.is_enabled()
            || self.contact_shadow_quality.is_enabled()
            || self.anti_aliasing == AntiAliasingMode::Taa
        {
            self.render_scale = RenderScaleQuality::Native; // 外部 JSON 的冲突组合优先保留显式 TAA/SSAO/接触阴影，并安全关闭空间缩放。
        }
    }
}
