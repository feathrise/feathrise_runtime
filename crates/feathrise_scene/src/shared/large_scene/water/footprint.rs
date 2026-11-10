//! 正式大场景动态水面轮廓的纯几何提取与点内判断。
//! 本模块不访问 ECS 或渲染资源；样条水保存实际带状网格的每个横向顶点，保证玩法
//! 高度、作者深度与 GPU 渲染采用完全相同的三角剖分。

use bevy::prelude::Vec2; // 二维坐标索引水平足迹，三维截面同时保存斜河真实水面高度。

use crate::scenes::dynamic_scene::{ClipmapGroundSnapshot, ClipmapWaterSurface}; // 静水互动与渲染共享同一R8及已采用高度修订。

mod painted;
mod ribbon; // 样条三角形索引与动态水花高度保持独立纯几何模块。 // 绘制静水从真实三个顶点变形后插值，避免稀疏网格解析高度误差。
pub(super) use ribbon::{RibbonWaterFootprint, ribbon_splash_position}; // 既有调用继续使用同一footprint入口。

/// 统一描述R8静水、三维河流、Clipmap与禁用互动的瀑布幕布。
#[derive(Clone)] // 每张水体只持有一种完整足迹，运行时不分配临时几何。
pub(super) enum WaterInteractionFootprint {
    Painted(PaintedWaterFootprint), // R8湖泊与池塘继续使用实际规则网格顶点Alpha。
    Ribbon(RibbonWaterFootprint),   // 三维河流与急流按实际带状三角形返回局部表面Y和作者深度。
    Clipmap {
        surface: ClipmapWaterSurface,  // 原始R8、世界范围、水位和最低安全水深。
        ground: ClipmapGroundSnapshot, // 同代分页高度快照按正式三角拓扑执行固定复杂度采样。
    },
    Disabled, // 瀑布幕布和纯视觉水体明确不参与玩家八槽波纹。
} // 结束统一水体互动足迹定义。

/// 统一互动查询返回当前局部XZ的静止表面高度和作者有效深度。
#[derive(Clone, Copy, Debug, PartialEq)] // 小型值只在当前水体玩家接触判定中使用。
pub(super) struct WaterInteractionSample {
    pub(super) surface_local_y: f32, // 当前水体局部空间中的真实静止水面高度。
    pub(super) author_depth: f32,    // 当前作者水深；旧静水使用无穷并沿用既有三米玩法上限。
} // 结束统一互动采样定义。

/// 按局部XZ查询实际水面；瀑布禁用分支始终返回None且不会触碰玩家波源池。
pub(super) fn water_interaction_sample(
    footprint: &WaterInteractionFootprint, // 当前水体初始化或后台构建完成的唯一互动几何。
    rest_water_level: f32,                 // 水平GLB与R8静水继续复用标量局部水位。
    local_xz: Vec2,                        // 已由水体GlobalTransform逆变换得到的玩家局部XZ。
) -> Option<WaterInteractionSample> {
    match footprint {
        WaterInteractionFootprint::Painted(painted) => painted
            .sample_visible_alpha(local_xz)
            .filter(|alpha| *alpha > f32::EPSILON)
            .map(|_| WaterInteractionSample {
                surface_local_y: rest_water_level, // R8静水实体局部水位仍固定为零。
                author_depth: f32::INFINITY,       // 现有静水层尚未保存独立作者深度。
            }),
        WaterInteractionFootprint::Ribbon(ribbon) => ribbon.sample(local_xz).map(|sample| {
            WaterInteractionSample {
                surface_local_y: sample.surface_local_y, // 斜河直接采用同源三角形插值Y。
                author_depth: sample.author_depth,       // 斜河直接采用同源截面插值作者深度。
            }
        }),
        WaterInteractionFootprint::Clipmap { surface, ground } => surface
            .surface_depth_at(ground, local_xz)
            .map(|depth| WaterInteractionSample {
                surface_local_y: surface.water_level, // Clipmap波源直接使用稳定世界坐标与绝对世界Y。
                author_depth: depth, // 真实地形三角水深限制玩家互动，缺页和跨修订已在采样入口拒绝。
            }),
        WaterInteractionFootprint::Disabled => None, // 瀑布持续表现不能授权或淘汰任何玩家波槽。
    }
}

/// 绘制水域在局部X/Z平面上的实际可见网格足迹。
///
/// 【功能设计】`0.20m`稳定深水核心只负责安全生成几何，不能再兼任玩法接触范围；
/// 这里保留渲染网格同一份顶点透明度，让浅滩与画面中的水面严格使用同一水平边界。
#[derive(Clone, Debug)]
pub(super) struct PaintedWaterFootprint {
    pub(super) cells: [u32; 2], // 与实际绘制水面完全相同的局部规则网格单元数。
    pub(super) local_min: [f32; 2], // 活跃水面网格局部X/Z最小端点。
    pub(super) local_max: [f32; 2], // 活跃水面网格局部X/Z最大端点。
    pub(super) vertex_alpha: std::sync::Arc<[f32]>, // 上传块共享完整互动足迹，不能逐块深拷贝。
}

impl PaintedWaterFootprint {
    /// 从后台已经完成的可见水面网格建立只读玩法足迹。
    pub(super) fn from_visible_mesh(
        cells: [u32; 2],        // 实际水面网格X/Z单元数。
        local_min: [f32; 2],    // 活跃网格局部X/Z最小端点。
        local_max: [f32; 2],    // 活跃网格局部X/Z最大端点。
        vertex_alpha: Vec<f32>, // 与Mesh顶点颜色逐顶点相同的透明度。
    ) -> Self {
        let expected_vertices = (cells[0] as usize + 1).saturating_mul(cells[1] as usize + 1); // 使用受检网格尺寸计算完整顶点数。
        debug_assert_eq!(
            vertex_alpha.len(),
            expected_vertices,
            "绘制水互动足迹必须与实际水面网格逐顶点一致"
        ); // 后台构建纯函数内部错误应在测试和调试构建立刻暴露。
        Self {
            cells,        // 保留实际渲染网格分辨率。
            local_min,    // 保留活跃水面最小端点。
            local_max,    // 保留活跃水面最大端点。
            vertex_alpha: vertex_alpha.into(),
        }
    }

    /// 为合法全零水层建立不会命中任何位置的空足迹。
    pub(super) fn empty(local_min: [f32; 2], local_max: [f32; 2]) -> Self {
        Self {
            cells: [0, 0],            // 零单元让运行时在任何除法和索引前直接拒绝。
            local_min,                // 保留作者范围供诊断值结构完整。
            local_max,                // 保留作者范围供诊断值结构完整。
            vertex_alpha: Default::default(), // 空水层不占用额外互动网格内存。
        }
    }

    /// 按水面Mesh相同的对角线，对脚下位置执行一次三角形顶点透明度插值。
    pub(super) fn sample_visible_alpha(&self, local_xz: Vec2) -> Option<f32> {
        let size_x = self.local_max[0] - self.local_min[0]; // 取得活跃网格局部X跨度。
        let size_z = self.local_max[1] - self.local_min[1]; // 取得活跃网格局部Z跨度。
        if self.cells[0] == 0
            || self.cells[1] == 0
            || size_x <= f32::EPSILON
            || size_z <= f32::EPSILON
            || !local_xz.x.is_finite()
            || !local_xz.y.is_finite()
            || local_xz.x < self.local_min[0]
            || local_xz.x > self.local_max[0]
            || local_xz.y < self.local_min[1]
            || local_xz.y > self.local_max[1]
        {
            return None; // 空足迹、坏坐标与活跃网格外位置都不能授权玩家水波。
        }
        let grid_x = (local_xz.x - self.local_min[0]) / size_x * self.cells[0] as f32; // 映射到连续网格X坐标。
        let grid_z = (local_xz.y - self.local_min[1]) / size_z * self.cells[1] as f32; // 映射到连续网格Z坐标。
        let cell_x = grid_x
            .floor()
            .clamp(0.0, self.cells[0].saturating_sub(1) as f32) as u32; // 最大端点归入最后一个单元。
        let cell_z = grid_z
            .floor()
            .clamp(0.0, self.cells[1].saturating_sub(1) as f32) as u32; // 最大端点归入最后一个单元。
        let fraction_x = (grid_x - cell_x as f32).clamp(0.0, 1.0); // 取得单元内横向插值比例。
        let fraction_z = (grid_z - cell_z as f32).clamp(0.0, 1.0); // 取得单元内纵向插值比例。
        let vertex_width = self.cells[0] + 1; // 顶点行宽比单元数多一。
        let top_left = (cell_z * vertex_width + cell_x) as usize; // 当前单元左上顶点下标。
        let top_right = top_left + 1; // 当前单元右上顶点下标。
        let bottom_left = top_left + vertex_width as usize; // 当前单元左下顶点下标。
        let bottom_right = bottom_left + 1; // 当前单元右下顶点下标。
        let top_left_alpha = *self.vertex_alpha.get(top_left)?; // 损坏足迹安全退化为不命中。
        let top_right_alpha = *self.vertex_alpha.get(top_right)?; // 读取实际Mesh右上顶点透明度。
        let bottom_left_alpha = *self.vertex_alpha.get(bottom_left)?; // 读取实际Mesh左下顶点透明度。
        let bottom_right_alpha = *self.vertex_alpha.get(bottom_right)?; // 读取实际Mesh右下顶点透明度。
        let alpha = if fraction_z >= fraction_x {
            top_left_alpha * (1.0 - fraction_z)
                + bottom_left_alpha * (fraction_z - fraction_x)
                + bottom_right_alpha * fraction_x
            // 左下三角与Mesh索引`左上-左下-右下`使用完全相同的重心权重。
        } else {
            top_left_alpha * (1.0 - fraction_x)
                + bottom_right_alpha * fraction_z
                + top_right_alpha * (fraction_x - fraction_z)
            // 右上三角与Mesh索引`左上-右下-右上`使用完全相同的重心权重。
        };
        alpha.is_finite().then_some(alpha) // 非有限完成值不能授权玩家水波。
    }
}

/// 判断局部点是否落在绘制水域实际具有可见透明度的三角形内。
#[cfg(test)]
pub(super) fn painted_water_footprint_contains(footprint: &PaintedWaterFootprint, local_xz: Vec2) -> bool {
    footprint
        .sample_visible_alpha(local_xz) // 常数次读取实际渲染网格的四个相邻顶点。
        .is_some_and(|alpha| alpha > f32::EPSILON) // 透明保护边和网格外区域都不属于互动水面。
}

#[cfg(test)]
mod tests;
