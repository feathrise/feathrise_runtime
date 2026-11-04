//! 多层碰撞的纯数据身份；不持有 ECS 或资产世界。
use bevy::prelude::{Vec2, Vec3}; // 采用边界与物理网格坐标。
use std::collections::BTreeMap;
use std::sync::Arc; // 不可变完整单元列表共享给后台候选，不在主线程复制规模相关数据。

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)] // 世界固定块键。
pub(super) struct ChunkKey {
    // level 表示作者格距的二次幂指数。
    pub level: u8,      // 当前采样层级。
    pub band: u32,      // 径向配置带决定独立管理粒度，同倍率远区不能继承近区小块。
    pub cells_log2: u8, // 每边单元数的二次幂指数；配置缩小管理块时禁止误复用旧大块。
    pub x: u32,         // 当前层固定块列。
    pub z: u32,         // 当前层固定块行。
} // 块身份不依赖相机。

#[derive(Clone, Debug)] // 后台内容签名避免主线程深比单元数组。
pub(super) struct ChunkPlan {
    // 一块完整几何输入。
    pub key: ChunkKey,                     // 固定世界块。
    pub finer_edges: u8,                   // 西1、东2、南4、北8：粗边补入细层顶点。
    pub cells: Option<Arc<Vec<CellPlan>>>, // 显式完整单元；None仅用于旧完整块测试夹具。
    pub signature: String,                 // 后台SHA256内容摘要，主线程只比较固定64字符。
} // 接缝身份纳入块复用判定。

impl PartialEq for ChunkPlan {
    // 比较小身份，不遍历几何规模数据。
    fn eq(&self, other: &Self) -> bool {
        // 缓存和发布共用。
        self.key == other.key
            && self.finer_edges == other.finer_edges
            && self.signature == other.signature
            && self.cells.is_some() == other.cells.is_some() // 不比较Arc内部数组。
    } // 摘要在后台生成。
} // 无帧关键路径哈希。

#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 不包含圆弧交点或任意剖分约束。
pub(super) struct CellPlan {
    // 世界固定格子的唯一几何合同。
    pub origin: [u64; 2], // 作者基础格坐标；步长由所属块level决定。
    pub finer_edges: u8,  // 仅邻接细一级的边拆成两段。
} // 普通单元永远两个三角形。

#[derive(Clone, Debug, PartialEq)] // 完整计划参与后台需求比较。
pub(super) struct Plan {
    // 无重叠的平衡四叉树叶集合。
    pub chunks: BTreeMap<ChunkKey, ChunkPlan>, // 包含必要的二倍过渡叶。
    pub outer_boundary: Vec<Vec2>,             // 旧发布字段兼容，新完整格路径恒为空，不再生成凸包。
    pub supported_rows: Option<Arc<feathrise_scene_core::residency::TerrainSupportRows>>, // 最粗完整格覆盖的逐行区间。
} // 所有叶共同原子发布。

pub(super) struct ChunkGeometry {
    // 后台返回的纯网格。
    pub vertices: Vec<Vec3>, // 同边点通过相同采样器取得完全相同高度。
    pub base_grid_points: Vec<[f64; 2]>, // 精确基础格坐标，避免世界f32反投影造成页边界漂移。
    pub triangles: Vec<[u32; 3]>, // 朝上的三角面索引。
} // Collider cooking 由运行时后台调用方负责。
