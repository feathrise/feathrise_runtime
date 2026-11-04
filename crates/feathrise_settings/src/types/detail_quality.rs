//! 画质设置中的内容细节档位：GPU 叶片、场景物件与 GPU 粒子。

/// GPU 叶片植被的最终密度、作者范围与屏幕 LOD 质量档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VegetationQuality {
    Low,    // 55% 最终密度、60% 作者范围，并更早切换粗模。
    Medium, // 75% 最终密度、80% 作者范围，并略早切换粗模。
    #[default]
    High, // 100% 作者密度、范围与 LOD 阈值，严格保持当前基线。
    Ultra,  // 保持作者密度与范围，把精细 LOD 的有效距离提高约 25%。
}

impl VegetationQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Ultra => "极高",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }

    /// 从持久化枚举名读取档位，未知值保持当前高画质植被基线。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "Medium" => Self::Medium,
            "Ultra" => Self::Ultra,
            _ => Self::High,
        }
    }

    /// 返回最终密度、屏幕 LOD 阈值与作者最远范围三个 GPU 倍率。
    pub const fn runtime_multipliers(self) -> (f32, f32, f32) {
        match self {
            Self::Low => (0.55, 1.35, 0.60), // 显著降低实绘量、范围并提前使用粗模。
            Self::Medium => (0.75, 1.15, 0.80), // 在画面连续性与 GPU 成本间取平衡。
            Self::High => (1.0, 1.0, 1.0),   // 单位倍率保证旧玩家升级后画面完全不变。
            Self::Ultra => (1.0, 0.8, 1.0),  // 不突破作者范围或容量，只延伸精细 LOD。
        }
    }
}

/// 树木、灌木与岩石共享的视觉 LOD 和最远显示距离质量档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SceneDetailQuality {
    Low,    // LOD 边界与最远范围都缩到作者值的 60%。
    Medium, // LOD 边界与最远范围都缩到作者值的 80%。
    #[default]
    High, // 完整保留作者 LOD 与最远范围，严格保持当前画面基线。
    Ultra,  // 不扩大实例或最远范围，只把精细 LOD 边界延长 10%。
}

impl SceneDetailQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Ultra => "极高",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }

    /// 从持久化枚举名读取档位，未知值保持当前高画质场景物件基线。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "Medium" => Self::Medium,
            "Ultra" => Self::Ultra,
            _ => Self::High,
        }
    }

    /// 返回普通 LOD 交接边界与最远显示边界两个运行时倍率。
    pub const fn runtime_multipliers(self) -> (f32, f32) {
        match self {
            Self::Low => (0.60, 0.60),    // 成比例提前降模并缩短最远范围。
            Self::Medium => (0.80, 0.80), // 保留主要远景轮廓并降低主视图与阴影成本。
            Self::High => (1.0, 1.0),     // 单位倍率严格恢复全部作者范围。
            Self::Ultra => (1.10, 1.0),   // 只延后交给粗模的边界，不突破作者最远距离。
        }
    }
}

/// 技能与天气 GPU 粒子的数量质量档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum EffectsQuality {
    Low,    // 使用作者粒子数量的 50%，优先降低低端 GPU 透明粒子成本。
    Medium, // 使用作者粒子数量的 75%，保留主要轮廓与密度。
    #[default]
    High, // 使用作者粒子数量的 100%，严格保持功能加入前的画面基线。
    Ultra,  // 使用作者粒子数量的 125%，增强高端硬件上的粒子密度。
}

impl EffectsQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Ultra => "极高",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }

    /// 从持久化枚举名读取档位，未知值保持当前作者粒子基线。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "Medium" => Self::Medium,
            "Ultra" => Self::Ultra,
            _ => Self::High,
        }
    }

    /// 返回技能与天气 GPU 粒子数量倍率。
    pub const fn particle_multiplier(self) -> f32 {
        match self {
            Self::Low => 0.50,    // 低档把透明粒子负载减半。
            Self::Medium => 0.75, // 中档保留四分之三作者密度。
            Self::High => 1.0,    // 高档必须精确保持现有粒子数量。
            Self::Ultra => 1.25,  // 极高档在固定容量预算内增加四分之一密度。
        }
    }
}
