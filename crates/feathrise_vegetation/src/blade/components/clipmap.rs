//! Blade 接收的已采用 Clipmap 高度快照；不持有场景 ECS 或整图高度数据。
use bevy::ecs::query::QueryItem; // 提取主世界只读组件。
use bevy::prelude::*; // 轻量句柄与数学类型。
use bevy::render::extract_component::ExtractComponent; // 将快照同步到渲染世界。
use bevy::render::sync_component::SyncComponent; // 保持目标组件生命周期一致。

/// GPU 草根和 CPU 作者查询共同支持的显示环带硬上限；倍率从1到2^15最多十六层。
pub const GPU_BLADE_CLIPMAP_MAX_BANDS: usize = 16;

/// 一个已经采用的矩形显示环带；局部格坐标与地形组合 Mesh 完全一致。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuBladeClipmapBand {
    pub inner_half_cells: UVec2, // 当前几何Mip格中的矩形洞口半径；首层为零。
    pub outer_half_cells: UVec2, // 当前几何Mip格中的矩形外边界半径。
    pub geometry_mip: u32,       // 相对作者基础格距的二次幂指数。
}

/// 地形和植被共同使用的完整已采用分页高度；三个修订依次为地图、高度、视图身份。
#[derive(Component, Clone, Debug, PartialEq)] // 每帧只复制两个资产句柄与固定参数。
pub struct GpuBladeClipmapHeight {
    pub stitches: std::sync::Arc<super::GpuBladeClipmapStitches>,
    /// 与地形所有pass相同的固定矩形裁切；洞内不会生成悬空草根。
    pub holes: [Vec4; 32],
    pub hole_count: u32,
    // Scene 负责原子发布；库不反向依赖具体地图实现。
    pub atlas: Handle<Image>, // R16Unorm 二维数组，各层是一张独立高度页。
    pub page_table: Handle<Image>, // RGBA32Uint 紧凑页表，编码 layer+1、minY、spanY。
    pub world_min: Vec2,      // 有限地图世界 X/Z 最小角。
    pub world_max: Vec2,      // 有限地图世界 X/Z 最大角。
    pub cell_size: Vec2,      // 最细层作者格距，地形显示使用同一格距。
    pub logical_cells: UVec2, // 两轴完整逻辑单元数，仅用于页寻址。
    pub max_mip: u32,         // 最高持久父页级别，与草叶 LOD 无关。
    pub fine_center: Vec2,    // 当前已采用视图最细层吸附中心。
    pub bands: [GpuBladeClipmapBand; GPU_BLADE_CLIPMAP_MAX_BANDS], // 从内向外的实际可变环带。
    pub band_count: u32,      // 有效前缀长度；不得超过固定十六槽。
    pub height_min: f32,      // 全图保守高度下界，用于草窗和瓦片剔除。
    pub height_max: f32,      // 全图保守高度上界，用于草窗和瓦片剔除。
    pub revision: [u64; 3],   // 地图代次、高度采用修订、视图修订共同拒绝旧输出。
    pub missing_pages: [UVec4; 16], // 各实际采样Mip至多四个待换入页，key=(x|z<<16)+1，零为空槽。
    pub ready: bool,          // 仅完整已采用快照允许 Compute，未就绪不得回退平地。
} // 完成与场景无关的公共高度契约。

impl ExtractComponent for GpuBladeClipmapHeight {
    // 强句柄保证地形和草地使用同一资源生命期。
    type QueryData = &'static Self; // 主世界只读完整快照。
    type QueryFilter = (); // 所有分页草窗都需同步移除与更新。
    type Out = Self; // 渲染世界保留同型轻量数据。
    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        // 不复制页像素。
        Some(item.clone()) // 只克隆句柄和固定数量参数。
    } // 完成快照提取。
} // 完成 ExtractComponent 实现。
impl SyncComponent for GpuBladeClipmapHeight {
    // 组件卸载时同步清理渲染副本。
    type Target = Self; // 不派生额外实体或资源。
} // 完成同步契约。
