//! 通用多部件Mesh后端的内容无关值类型。
//! 【功能设计】这里只描述共享几何、类型擦除材质绑定、LOD、公告板能力位和可选风参数，不持有ECS生命周期。

use bevy::asset::UntypedHandle; // 强类型材质句柄擦除后仍保持原强弱语义与运行时类型身份。
use bevy::camera::{primitives::Aabb, visibility::VisibilityRange}; // 复用Bevy原生剔除边界和宽域距离LOD。
use bevy::ecs::system::EntityCommands; // 类型擦除安装函数只在共享生成入口恢复具体材质组件。
use bevy::prelude::{Dir3, Handle, Mesh, Transform}; // 通用原型只组合共享Mesh、材质绑定、公告板模式与局部变换。

use crate::VegetationLayerId; // 渲染分箱身份沿用布局层的稳定技术ID，不引入项目内容枚举。
use crate::VegetationPrototypeId; // 原型身份不包含树、灌木或岩石枚举。

pub(super) type VegetationMeshMaterialInsert = for<'a> fn(&UntypedHandle, &mut EntityCommands<'a>); // 私有函数指针恢复正确的MeshMaterial3d<M>。

/// 通用Mesh后端的LOD身份；数值只用于诊断，不限制档位数量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VegetationMeshLodLevel(pub u8); // 项目通常使用0至3表达高、中、低、公告板。

/// 批量实例顶点阶段需要保留的部件形变能力。
///
/// 【功能设计】批量后端继续复用原部件的具体 Bevy 材质及其片元、预通道和阴影管线；
/// 这里只声明原来由逐实例顶点管线承担的能力，禁止根据材质类型名猜测树木、灌木或岩石。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VegetationMeshBatchVertexMode {
    #[default]
    Static, // 固定 Mesh、固定交叉 impostor 和岩石只应用逐实例世界矩阵。
    Wind,      // TreeWindMaterial 等高度权重风材质在批量顶点阶段继续执行同一风形变。
    Billboard, // 动态公告板按当前主视图或关联阴影视图在 GPU 中逐实例转向。
}

/// RenderApp只读统计使用的通用Mesh目标身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VegetationMeshRenderTarget {
    pub layer: VegetationLayerId, // 精确归属一个布局层，避免共用Mesh句柄时把旧实现计入候选。
    pub prototype: VegetationPrototypeId, // 精确归属一个已登记原型，支持同层热替换后的独立核对。
} // 目标身份只表达技术归属，不承担运行时所有权切换。

/// RenderApp只读统计使用的一档单部件身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VegetationMeshRenderPart {
    pub level: VegetationMeshLodLevel, // 保留任意数量LOD档位，不硬编码树木的四档约定。
    pub part_id: u32,                  // 由原型提供的稳定部件编号，可表示树干、树冠或其他组合部件。
    pub billboard: bool,               // 明确区分普通几何与公告板部件，便于F3核对远景成本。
} // 部件身份与Mesh资产句柄解耦，同资产复用不会合并错误分箱。

/// 单个PBR阶段中由Bevy公开分箱证明的通用Mesh提交量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VegetationMeshRenderPhaseStats {
    pub views: u32,       // 至少包含一个匹配节点的主视图或阴影视图数量。
    pub draw_groups: u32, // 多绘制批集、普通批次或独立实体形成的真实提交组数量。
    pub bins: u32,        // 至少包含一个匹配节点的真实Bevy渲染分箱数量。
    pub instances: u32,   // 分箱中的匹配渲染实体数量，跨视图时按视图累计。
} // 数值来自CPU侧公开排队结果，不触发GPU同步读回。

/// 一档单部件在上一完整渲染帧中的PBR分箱快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VegetationMeshRenderPartStats {
    pub part: VegetationMeshRenderPart, // LOD、部件编号和公告板位共同构成精确分组键。
    pub opaque: VegetationMeshRenderPhaseStats, // 主画面Opaque3d公开分箱。
    pub alpha_mask: VegetationMeshRenderPhaseStats, // 主画面AlphaMask3d公开分箱。
    pub shadow: VegetationMeshRenderPhaseStats, // 灯光Shadow公开分箱。
} // 每次采样原位归零并重建，禁止从上一原型遗留部件数据。

/// 一个布局层与原型组合在上一完整渲染帧中的PBR分箱快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VegetationMeshRenderTargetStats {
    pub target: VegetationMeshRenderTarget, // 层和原型共同隔离旧实现、跨地图复用与热替换。
    pub opaque: VegetationMeshRenderPhaseStats, // 目标全部LOD与部件的Opaque3d合计。
    pub alpha_mask: VegetationMeshRenderPhaseStats, // 目标全部LOD与部件的AlphaMask3d合计。
    pub shadow: VegetationMeshRenderPhaseStats, // 目标全部LOD与部件的Shadow合计。
    pub parts: Vec<VegetationMeshRenderPartStats>, // 按LOD、部件和公告板身份稳定升序保存。
} // 目标合计直接按实体过滤统计，不把各部件视图数或提交组数简单相加。

/// 相机朝向公告板在资产局部空间中的正面轴。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VegetationMeshBillboardFrontAxis {
    PositiveX, // 资产平面正面沿局部正X，适配侧向建模的四边形。
    NegativeX, // 资产平面正面沿局部负X，适配反向侧向建模的四边形。
    PositiveZ, // 资产平面正面沿局部正Z。
    #[default]
    NegativeZ, // Bevy Transform默认前向轴，程序化平面优先使用该约定。
}

impl VegetationMeshBillboardFrontAxis {
    pub fn direction(self) -> Dir3 {
        match self {
            Self::PositiveX => Dir3::X,     // 返回局部正X单位方向。
            Self::NegativeX => Dir3::NEG_X, // 返回局部负X单位方向。
            Self::PositiveZ => Dir3::Z,     // 返回局部正Z单位方向。
            Self::NegativeZ => Dir3::NEG_Z, // 返回Bevy默认前向单位方向。
        } // 完成有限安全轴到Bevy方向的映射。
    } // 完成公告板资产正面方向查询。
}

/// 一个部件的显式公告板朝向模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VegetationMeshBillboard {
    #[default]
    Fixed, // 固定交叉impostor保留作者局部旋转，不读取相机。
    Cylindrical {
        front_axis: VegetationMeshBillboardFrontAxis, // 正面仅绕世界Y轴朝向唯一主动相机。
    },
    Spherical {
        front_axis: VegetationMeshBillboardFrontAxis, // 正面使用完整三维方向朝向唯一主动相机。
    },
}

impl VegetationMeshBillboard {
    pub fn requires_camera(self) -> bool {
        !matches!(self, Self::Fixed) // 固定交叉面不应因缺少主相机进入等待或改变姿态。
    } // 完成公告板相机需求查询。
}

/// 可选高度权重风的材质适配参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegetationHeightWeightedWind {
    pub root_height: f32, // 此世界/局部高度以下权重为零，固定树根或灌木根。
    pub tip_height: f32,  // 此高度及以上达到完整风强。
    pub strength: f32,    // 有限非负的最大米制或材质定义摆幅。
    pub frequency: f32,   // 动画频率由材质适配器解释。
}

/// 一个保留具体Bevy材质类型与原资产句柄语义的内容无关绑定。
#[derive(Clone)]
pub struct VegetationMeshMaterialBinding {
    pub(super) handle: UntypedHandle, // 注册表原样持有调用方句柄；正式内容应传已加载强句柄以维持资产存活。
    pub(super) material_type_name: &'static str, // F3与fixture无需解析内容名称即可识别真实材质类型。
    pub(super) insert: VegetationMeshMaterialInsert, // 生成节点时恢复唯一正确的MeshMaterial3d<M>组件。
}

/// 一档中的一个共享PBR部件。
#[derive(Clone)]
pub struct VegetationMeshPart {
    pub part_id: u32,       // 消费侧可用稳定编号区分树干、树冠或任意部件。
    pub mesh: Handle<Mesh>, // 全部同原型实例共享几何句柄。
    pub material: VegetationMeshMaterialBinding, // 标准PBR与任意Material扩展使用同一类型安全绑定。
    pub local_transform: Transform, // 树冠、树干或组合岩块可在同一原型根内保持独立局部位置。
    pub culling_aabb: Option<Aabb>, // GPU变形可传覆盖全部姿态的保守边界；动态公告板为None时会关闭视锥剔除。
    pub casts_shadow: bool,         // 每档每部件独立控制方向光阴影成本。
    pub billboard: Option<VegetationMeshBillboard>, // None为普通部件，Some显式区分固定或相机朝向模式。
    pub wind: Option<VegetationHeightWeightedWind>, // 高度权重风参数供可选材质适配系统消费。
    pub batch_vertex_mode: VegetationMeshBatchVertexMode, // 海量实例批量路径显式保留静态、风动或动态公告板顶点语义。
}

/// 一个LOD档及其任意数量Mesh/Material部件。
#[derive(Clone)]
pub struct VegetationMeshLod {
    pub level: VegetationMeshLodLevel, // 诊断和后端适配使用的稳定档位。
    pub visibility_range: VisibilityRange, // 相邻档可共享宽域交叉区。
    pub parts: Vec<VegetationMeshPart>, // 树干加树冠或多块岩石都无需特殊结构。
}

/// 一种通用多部件Mesh原型。
#[derive(Clone)]
pub struct VegetationMeshPrototype {
    pub id: VegetationPrototypeId,    // 与布局层引用的稳定原型身份相同。
    pub lods: Vec<VegetationMeshLod>, // 支持任意档数，每档支持任意部件数。
}
