//! 树木公共值类型。
//! 本文件只声明场景层装配树位和四档现有资产所需的数据结构。
//! 【模块所有权】纯值契约随树木子域内收，字段与公开路径保持兼容。

use bevy::camera::visibility::VisibilityRange; // 复用Bevy 0.18自带带抖动过渡的距离LOD。
use bevy::prelude::*; // 引入Handle、Mesh、StandardMaterial和基础数学类型。

use crate::mesh_wind::{MeshWindLodLevel, MeshWindPart}; // 树木内容身份只在本层转换为共享Mesh风质量参数。

/// 树木几何 LOD 身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)] // 默认高档便于零值诊断保持合法。
pub enum TreeLodLevel {
    #[default] // 零值对应玩家近处的最高几何档。
    High, // 高模树干与树冠。
    Medium,    // 中模树干与树冠。
    Low,       // 低模树干与树冠。
    Billboard, // 最远完整树木公告板。
} // 完成四档树木LOD枚举。

/// 树木渲染节点部件身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] // 部件身份只用于诊断和批次审计。
pub enum TreeMeshPart {
    Trunk,     // 不透明树干网格。
    Canopy,    // AlphaMask树冠网格。
    Billboard, // 同时表达树干与树冠的最远公告板。
} // 完成树木部件枚举。

impl From<TreeLodLevel> for MeshWindLodLevel {
    // 共享风模块不反向依赖树木领域，由树木所有者负责一一映射现有四档身份。
    fn from(level: TreeLodLevel) -> Self {
        match level {
            TreeLodLevel::High => Self::High, // 近景实体树保留完整叶簇细摆。
            TreeLodLevel::Medium => Self::Medium, // 中景实体树使用减弱细摆。
            TreeLodLevel::Low => Self::Low,   // 远景实体树只保留低成本宏观风。
            TreeLodLevel::Billboard => Self::Billboard, // 公告板保持静止安全回退。
        }
    }
}

impl From<TreeMeshPart> for MeshWindPart {
    // 树木专有部件名在进入共享材质前转换为通用茎干、叶簇和公告板语义。
    fn from(part: TreeMeshPart) -> Self {
        match part {
            TreeMeshPart::Trunk => Self::Stem, // 木质树干只参与根部固定的宏观弯曲。
            TreeMeshPart::Canopy => Self::Foliage, // 树冠和灌木叶簇可叠加LOD分级细摆。
            TreeMeshPart::Billboard => Self::Billboard, // 最远公告板不安装具体风材质。
        }
    }
}

/// 单档树木现有资产及其可见范围。
#[derive(Clone)] // 场景生成时为每棵树复用同一组轻量句柄和范围。
pub struct TreeLodPrototype {
    pub level: TreeLodLevel,               // 当前原型所属四档身份。
    pub trunk_mesh: Option<Handle<Mesh>>,  // 公告板没有独立树干，实体LOD保存对应树干primitive。
    pub canopy_mesh: Handle<Mesh>, // 实体LOD保存树冠primitive，公告板保存完整impostor primitive。
    pub visibility_range: VisibilityRange, // 相邻档共享一致交叉区，避免距离边界突然跳变。
    pub casts_shadow: bool,        // 场景质量预算决定当前档是否进入阴影图，树干与树冠必须保持一致。
} // 完成单档树木原型定义。

/// 一种树木的完整现有渲染资产句柄。
#[derive(Clone)] // HN-01所有受控与全图树位共享同一原型，不复制纹理或材质资产。
pub struct TreePrototype {
    pub lods: [TreeLodPrototype; 4], // 固定高、中、低、公告板四档，便于固定批次预算。
    pub trunk_material: Handle<StandardMaterial>, // 三个实体LOD共享唯一树干材质。
    pub canopy_material: Handle<StandardMaterial>, // 三个实体LOD共享唯一树冠材质。
    pub billboard_material: Handle<StandardMaterial>, // 最远公告板使用原GLB独立材质。
    pub ground_embed_depth: f32,     // 树根原点吸附地表后继续下扎的米制深度，避免坡面露缝。
} // 完成共享树木原型定义。

/// 场景内容层离线确定的一棵树。
#[derive(Clone, Copy, Debug, PartialEq)] // 常量表可直接复制，不持有运行时资产或实体。
pub struct TreePlacement {
    pub id: u16,            // 场景内稳定编号。
    pub position: Vec3,     // 世界X/Z由内容表固定，Y交给统一地面吸附解算。
    pub yaw_radians: f32,   // 稳定朝向用于打散重复资产轮廓。
    pub uniform_scale: f32, // 统一缩放保持树木比例并提供有限体量变化。
} // 完成离线树位记录定义。
