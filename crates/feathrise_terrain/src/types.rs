//! 动态场景地形派生使用的纯数据类型与有界计算保护。

/// 高度工具共享米制位移预算；平滑和整平朝冻结目标移动且不越过目标。
#[derive(Clone, Copy)]
pub enum TerrainHeightBrushKind { RaiseLower, Smooth, Flatten }
#[derive(Clone, Copy)]
pub struct TerrainHeightBrush {
    pub kind: TerrainHeightBrushKind, pub center: bevy::math::Vec3, pub radius: f32,
    pub strength: f32, pub delta_seconds: f32,
    /// 已含速度、强度与帧时间；升降带方向，平滑和压平取绝对值作为最大步长。
    pub delta_height: f32,
    pub flatten_target: Option<f32>, pub minimum: f32, pub maximum: f32,
    pub hit_triangle: Option<u32>,
}
/// 一个属性顶点的精确f32差分；调用者可合并为完整拖动历史。
pub struct TerrainVertexHeightChange { pub index: usize, pub before: f32, pub after: f32 }

/// 空间分桶最多使用的单轴桶数，避免细分白模在固定高度采样中超出候选测试预算。
pub const MAX_TERRAIN_RASTER_BINS_PER_AXIS: u32 = 128; // 16384桶只约束非规则三角网格的通用栅格化；规则平地走直接采样。
/// 栅格化允许的三角候选测试硬上限，损坏重叠网格不得无限消耗后台CPU。
pub const MAX_TERRAIN_RASTER_TESTS: usize = 64 * 1024 * 1024; // 最坏任务仍具有明确停止条件。
/// 三角形写入空间桶的总引用硬上限，防止大量全图覆盖面形成异常内存峰值。
pub const MAX_TERRAIN_BIN_REFERENCES: usize = 8 * 1024 * 1024; // U32引用峰值约32MiB。

/// 从 Bevy Mesh 与完整节点变换冻结出的世界空间三角网格。
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainMeshInput {
    /// 世界空间顶点；项目坐标为 X/Z 水平、Y 向上。
    pub positions: Vec<[f32; 3]>, // F12修改时保持顺序不变。
    /// TriangleList 的显式 U32 索引；每三个值组成一个三角形。
    pub indices: Vec<u32>, // 保存时保持三角形顺序和连接关系不变。
}

/// 把完整三维地形的最高室外包络采样为规则高度网格所需的清单参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainSampleSpec {
    /// 顶点高度网格宽高，采样点包含场景四条边界。
    pub resolution: [u32; 2], // 动态场景当前限制为`2..=513`。
    /// 世界水平范围下界 `[x, z]`。
    pub world_min: [f32; 2], // 允许非方形和非居中场景。
    /// 世界水平范围上界 `[x, z]`。
    pub world_max: [f32; 2], // 必须逐轴严格大于下界。
    /// `u16 = 0` 对应的世界 Y。
    pub base_height: f32, // Blender顶点不得超出完整编码量程。
    /// `u16::MAX` 相对最低高度的世界米制跨度。
    pub height_scale: f32, // 必须为有限正数。
}

/// 后台完整派生成功后可被画面、碰撞、水域与植被原子采用的数据。
#[derive(Clone, Debug)]
pub struct TerrainDerivedData {
    /// 保留源顶点顺序的世界空间位置。
    pub positions: Vec<[f32; 3]>, // 运行时与F12预览共享实际Blender拓扑。
    /// 按共享拓扑与作者绕序重算的面积加权平滑法线。
    pub normals: Vec<[f32; 3]>, // 不拆点，并允许洞顶、洞壁和倒扣方向。
    /// 保留源三角顺序和连接关系的显式U32索引。
    pub indices: Vec<u32>, // Collider、可见Mesh与GLB保存使用同一表。
    /// 以 Z 行、X 列排列的量化最高表面高度。
    pub heights: Vec<u16>, // 室外水域与植被共享该CPU包络，完整洞穴仍读取真实Mesh/Trimesh。
    /// `(width-1) × (height-1)` 的 RGBA16_UNORM 小端基础级字节。
    pub height_cells_rgba16: Vec<u8>, // 通道固定为NW、NE、SW、SE。
    /// 实际世界包围盒下界。
    pub min: [f32; 3], // GLB accessor与运行时诊断复用。
    /// 实际世界包围盒上界。
    pub max: [f32; 3], // GLB accessor与运行时诊断复用。
}
