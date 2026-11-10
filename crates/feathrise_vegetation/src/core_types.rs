//! 通用种植核心的正交公开配置。

use std::fmt; // 权重和配置错误需要给出可读诊断。

use bevy::prelude::{IVec2, Rect, Transform, Vec2}; // 世界范围、显式落点和足迹形状的基础数学类型。

/// 不受任何渲染后端容量限制的种植层身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VegetationLayerId(pub u64); // 四个 GPU 外观槽只属于单个叶片批次，与本ID无关。

/// 不受内容类别约束的竞争组身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VegetationCompetitionGroupId(pub u64); // 地被、林冠或任意项目语义只存在于消费侧常量名。

/// 内容原型的稳定身份，用于近场代理及后端实例关联。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VegetationPrototypeId(pub u64); // 原型可指向草叶、树木、岩石或任何消费者定义资产。

/// 经过校验的有限正竞争权重。
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct VegetationWeight(f32); // 私有字段防止 NaN、无穷或非正值进入解析器。

impl VegetationWeight {
    /// 创建一个有限且严格大于零的竞争权重。
    pub fn new(value: f32) -> Result<Self, &'static str> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value)) // 合法值保持原精度供同优先级分配使用。
        } else {
            Err("vegetation weight must be finite and greater than zero") // 非法配置必须原子拒绝。
        }
    }

    /// 返回受检权重原值。
    pub const fn get(self) -> f32 {
        self.0 // 消费者无法通过该入口绕过构造校验。
    }
}

impl Default for VegetationWeight {
    fn default() -> Self {
        Self(1.0) // 等权是最少惊讶的公共默认值。
    }
}

impl fmt::Display for VegetationWeight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter) // 诊断中直接显示项目配置的数值。
    }
}

/// 一个消费者显式提供的稳定候选落点。
#[derive(Clone, Debug, PartialEq)]
pub struct ExplicitVegetationPlacement {
    pub instance_id: u64,     // 场景稳定ID用于存档、代理滞回和局部脏区重算。
    pub transform: Transform, // 完整平移、旋转和缩放由内容生成器决定。
    pub density: f32,         // 零至一的单点需求允许项目侧保留稀疏概率。
}

/// 候选位置的生成方式，不包含草、树或岩石语义。
#[derive(Clone, Debug, PartialEq)]
pub enum VegetationCandidateSource {
    DenseGrid {
        spacing: f32, // 候选间距单独决定本层100%容量，不能与密度含义混淆。
        seed: u64,    // 世界稳定哈希种子区分不同层或不同外观批次。
    },
    JitteredGrid {
        spacing: f32, // 每个世界网格单元最多产生一个候选。
        jitter: f32,  // 零至0.5表示相对网格间距的轴向偏移上限。
        seed: u64,    // 固定种子保证相机移动或脏区重算不重新洗牌。
    },
    ExplicitPlacements(Vec<ExplicitVegetationPlacement>), // 树木岩石等可先保留当前离线稳定落点。
}

impl VegetationCandidateSource {
    /// 返回规则网格源的候选间距；显式落点没有统一间距。
    pub fn spacing(&self) -> Option<f32> {
        match self {
            Self::DenseGrid { spacing, .. } | Self::JitteredGrid { spacing, .. } => {
                Some(*spacing) // 两种程序化网格都用米制候选间距。
            }
            Self::ExplicitPlacements(_) => None, // 离线落点之间可具有任意距离。
        }
    }
}

/// 一层归一化密度输入及其世界映射。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegetationDensityField {
    pub world_bounds: Rect, // 纹理或程序化字段覆盖的世界X/Z范围。
    pub gain: f32,          // 采样后应用的非负有限增益，结果最终钳制到零至一。
}

impl VegetationDensityField {
    /// 对原始归一化采样应用层增益。
    pub fn apply(self, sample: f32) -> f32 {
        (sample * self.gain).clamp(0.0, 1.0) // 100%始终表示本层候选容量全部启用。
    }

    /// 在本层声明的世界范围内应用原始采样；范围外严格为零。
    pub fn apply_at(self, world_xz: Vec2, sample: f32) -> f32 {
        if self.world_bounds.contains(world_xz) {
            self.apply(sample) // 范围内应用受检增益。
        } else {
            0.0 // 源资产范围更大时也不能绕过本层独立栖息地边界。
        }
    }
}

/// 单个竞争组采用的容量与覆盖语义。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum VegetationCompetitionMode {
    #[default]
    Additive, // 各层完全叠加，100/100仍为100/100。
    WeightedPartition {
        capacity: f32, // 组总容量通常为1；不足需求不自动放大。
    },
    PriorityOverride {
        claim_threshold: f32, // 超过阈值的最高优先级层排除较低层。
    },
}

/// 一个竞争组的公开配置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegetationCompetitionGroup {
    pub id: VegetationCompetitionGroupId, // 层引用的稳定组身份。
    pub mode: VegetationCompetitionMode,  // 明确选择叠加、加权分区或优先覆盖。
    pub selection_cell_size: f32,         // 同级覆盖使用的固定世界选择网格大小。
    pub selection_seed: u64,              // 同级空间分配不依赖相机、窗口或帧。
}

/// 一层在优先覆盖组中声明空间所有权时采用的源采样口径。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VegetationClaimSampling {
    #[default]
    Point, // 默认只由输出点处的最近源样本声明所有权，保持既有层行为。
    SourceCellOverlap, // 任意非零源采样单元与输出单元相交时声明所有权，但不扩大本层实际密度。
}

/// 一层的竞争声明。
#[derive(Clone, Debug, PartialEq)]
pub struct VegetationCompetition {
    pub group: VegetationCompetitionGroupId, // 本层自身参与容量竞争的组。
    pub priority: i32,                       // 数值越大越优先，天然避免关系式循环。
    pub weight: VegetationWeight,            // 同优先级分区或覆盖选择的相对权重。
    pub claim_sampling: VegetationClaimSampling, // 所有权采样与本层实际密度采样保持正交。
    pub excludes_groups: Vec<VegetationCompetitionGroupId>, // 本层足迹可向其他组输出净空。
}

/// 一层空间占用的基础几何。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VegetationFootprintShape {
    Circle {
        radius: f32, // 圆形原型的实际占用半径。
    },
    Ellipse {
        half_extents: Vec2, // X/Z两个方向的实际半轴。
    },
    Rectangle {
        half_extents: Vec2, // 旋转前局部X/Z矩形半尺寸。
    },
    MeshAabb {
        half_extents: Vec2, // 未缩放原型Mesh AABB投影的X/Z半尺寸；实例Transform负责得到实际尺寸。
    },
}

/// 足迹尺寸是否继承实例Transform缩放。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VegetationFootprintScale {
    #[default]
    InheritTransform, // 默认使用实例Transform缩放实际轮廓，适合随模型体量变化的实体。
    WorldSpace, // 忽略实例缩放并把形状尺寸解释为固定世界米制，适合独立于外观缩放的净空。
}

/// 实际占用、额外边距与软恢复带的正交配置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegetationFootprint {
    pub shape: VegetationFootprintShape, // 原型自身实际空间占用。
    pub scale: VegetationFootprintScale, // 明确轮廓是否继承实例Transform缩放。
    pub exclusion_margin: f32,           // 在实际轮廓外额外清除目标层的米制边框。
    pub recovery_width: f32,             // 净空外从零平滑恢复到完整密度的宽度。
}

/// 一层由 GPU、实体或近远混合方式实现。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VegetationRealization {
    GpuOnly,  // 只向渲染后端发布稳定候选或分配场。
    Entities, // 全部解析候选由消费者实现为ECS实体。
    HybridNearField {
        enter_radius: f32, // 候选进入内圈时发布稳定代理。
        exit_radius: f32,  // 必须大于进入半径，用滞回避免边界反复生成。
    },
}

/// 一层完整且内容无关的种植声明。
#[derive(Clone, Debug, PartialEq)]
pub struct VegetationLayerConfig {
    pub id: VegetationLayerId, // 全局无限层ID，不复用任何后端外观槽。
    pub prototype: VegetationPrototypeId, // 后端内容原型身份。
    pub source: VegetationCandidateSource, // 候选空间分布。
    pub density: VegetationDensityField, // 归一化密度的世界映射与增益。
    pub competition: VegetationCompetition, // 本组优先级、权重和跨组排斥。
    pub footprint: VegetationFootprint, // 实际轮廓、额外边距和软恢复带。
    pub realization: VegetationRealization, // 渲染或实体实现策略。
}

/// 一个离散世界布局纹素或候选所属的整数坐标。
pub(crate) type VegetationWorldCell = IVec2; // 内部共享别名避免公共API暴露后端网格细节。

#[cfg(test)]
mod tests {
    use super::*; // 权重构造测试锁定公共新类型的非法值保护。

    #[test]
    fn competition_weight_rejects_zero_negative_and_non_finite_values() {
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                VegetationWeight::new(invalid).is_err(),
                "{invalid:?}不得进入竞争分配"
            ); // 所有会破坏权重总和或排序的值都必须在API边界拒绝。
        }
        assert_eq!(VegetationWeight::new(2.5).unwrap().get(), 2.5); // 有限正值保持调用者精度。
    }

    #[test]
    fn density_world_bounds_remain_independent_from_source_asset_bounds() {
        let density = VegetationDensityField {
            world_bounds: Rect::from_corners(Vec2::splat(-2.0), Vec2::splat(2.0)),
            gain: 2.0,
        };
        assert_eq!(density.apply_at(Vec2::ZERO, 0.25), 0.5); // 范围内正常应用项目增益。
        assert_eq!(density.apply_at(Vec2::new(2.1, 0.0), 1.0), 0.0); // 即使更大源字段有值，层栖息地外仍严格禁种。
    }
}
