//! 固定景观实例的公共纯值类型。
//! 【功能设计】地图层提供稳定位置与现有资产句柄，本文件不执行随机散布、资源加载或逐帧更新。
//! 【模块所有权】纯值契约随固定景观子域内收，字段与公开路径保持兼容。

use bevy::camera::visibility::VisibilityRange; // 四档景观LOD复用Bevy原生宽域距离过渡。
use bevy::prelude::*; // Handle、Mesh、StandardMaterial与世界变换值类型。

/// 当前固定景观实例的内容类别。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)] // 稳定枚举同时供ECS、F3和地图清单使用。
pub enum LandscapeKind {
    #[default] // 零值使用无碰撞的灌木，避免默认状态意外生成物理代理。
    Shrub, // 读取灌木密度清单并使用统一世界风场的叶簇实例。
    Rock, // 读取岩石密度清单并只为近处中大型实例生成简化碰撞。
} // 完成景观类别枚举。

/// 固定景观的四档几何身份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)] // 高中低加动态超低代理统一表达完整景观LOD链。
pub enum LandscapeLodLevel {
    #[default] // 零值对应玩家近处最高质量档。
    High, // 近景最高质量网格和纹理。
    Medium, // 中景降低几何或纹理成本的现有资产档。
    Low,    // 远景最低现有GLB资产档。
    Ultra,  // 最远景使用加载阶段从最低网格生成的共享超低代理。
} // 完成四档景观LOD枚举。

/// 单档景观现有资产及其可见范围。
#[derive(Clone)] // 全图同类实例共享同一组句柄与距离范围。
pub struct LandscapeLodPrototype {
    pub level: LandscapeLodLevel, // 当前原型所属高、中、低或动态超低档。
    pub mesh: Handle<Mesh>,       // 前三档引用现有GLB，第四档引用本地图加载阶段生成的共享代理。
    pub material: Handle<StandardMaterial>, // 第四档继续复用最低GLB现有PBR材质，不创建图片资产。
    pub visibility_range: VisibilityRange, // 相邻档共享相同交叉范围，避免硬距离瞬切。
    pub casts_shadow: bool,       // 当前质量预算决定该档是否进入方向光阴影视图。
} // 完成单档景观原型定义。

/// 一类景观实例共享的完整现有资产入口。
#[derive(Clone)] // 地图生成期间复用三档源资产与一档共享代理，不复制逐实例纹理或网格数据。
pub struct LandscapePrototype {
    pub kind: LandscapeKind, // 防止灌木清单误绑定岩石原型或反向绑定。
    pub lods: [LandscapeLodPrototype; 4], // 高、中、低、超低四档固定数组锁定最坏渲染节点预算。
    pub ground_embed_depth: f32, // 根原点吸附后继续下扎的米制深度，遮蔽坡面和量化细缝。
} // 完成共享景观原型定义。

/// 地图内容层离线冻结的一个景观落点。
#[derive(Clone, Copy, Debug, PartialEq)] // 记录只保存稳定内容值，不持有运行时资产或实体。
pub struct LandscapePlacement {
    pub id: u16,                 // 当前类别内从零连续递增的稳定编号。
    pub kind: LandscapeKind,     // 明确当前落点属于灌木或岩石清单。
    pub position: Vec3,          // X/Z由离线清单固定，Y交给统一GroundSnap解算。
    pub yaw_radians: f32,        // 稳定绕Y朝向打散重复现有资产轮廓。
    pub uniform_scale: f32,      // 有限统一缩放保持原资产比例并提供体量变化。
    pub collision_enabled: bool, // 只让中大型岩石进入近处碰撞代理管理。
} // 完成稳定景观落点定义。
