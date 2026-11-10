//! 绘制静水的实际三角片面采样；复用规则网格和透明度，不缓存或复制Mesh。
use super::PaintedWaterFootprint; // 沿用后台完整发布的只读足迹。
use bevy::prelude::{Vec2, Vec3}; // 水平候选与变形后三维位置。

impl PaintedWaterFootprint {
    // 规则网格可常数次重建实际命中三角。
    /// 候选定义在静止网格重心坐标中，三个GPU变形顶点插值同时恢复真实水平偏移与高度。
    pub(in super::super) fn sample_displaced_position(
        &self,                                // 当前完整足迹。
        point: Vec2,                          // 静止水面候选XZ。
        water_level: f32,                     // 与渲染网格相同的局部静止高度。
        displace: impl Fn(Vec3, f32) -> Vec3, // 调用方提供同源GPU顶点变形，透明度逐顶点传入。
    ) -> Option<Vec3> {
        // 无有效真实三角时拒绝发射。
        let alpha = self.sample_visible_alpha(point)?; // 复用完整边界、损坏数据与单元合法性检查。
        if alpha <= 0.95 || !water_level.is_finite() {
            return None;
        } // 仅完全可见水面核心允许离水花。
        let minimum = Vec2::from_array(self.local_min); // 网格实际起点。
        let size = Vec2::from_array(self.local_max) - minimum; // 网格实际跨度。
        let cells = Vec2::new(self.cells[0] as f32, self.cells[1] as f32); // 实际单元数。
        let grid = (point - minimum) / size * cells; // 与生成器同一规则坐标。
        let cell = grid.floor().clamp(Vec2::ZERO, cells - Vec2::ONE); // 最大边界归入最后一个单元。
        let fraction = (grid - cell).clamp(Vec2::ZERO, Vec2::ONE); // 当前单元内部重心坐标来源。
        let (corners, weights) = if fraction.y >= fraction.x {
            // 生成器采用左上至右下对角线。
            (
                [Vec2::ZERO, Vec2::Y, Vec2::ONE],
                [1.0 - fraction.y, fraction.y - fraction.x, fraction.x],
            ) // 左下三角与Mesh索引一致。
        } else {
            // 另一半单元采用另一片实际三角。
            (
                [Vec2::ZERO, Vec2::ONE, Vec2::X],
                [1.0 - fraction.x, fraction.y, fraction.x - fraction.y],
            ) // 右上三角与Mesh索引一致。
        }; // 完成唯一三角选择。
        let mut result = Vec3::ZERO; // 按真实三角重心插值整个三维位置。
        for (corner, weight) in corners.into_iter().zip(weights) {
            // 固定三个顶点，不遍历整张水面。
            let vertex = cell + corner; // 实际整数网格顶点坐标。
            let index = vertex.y as usize * (self.cells[0] as usize + 1) + vertex.x as usize; // 与生成器行主序相同。
            let vertex_alpha = *self.vertex_alpha.get(index)?; // 逐顶点岸带衰减不能用片元插值Alpha代替。
            let xz = minimum + size * (vertex / cells); // 与Mesh生成器完全相同的静止角点。
            let position = displace(Vec3::new(xz.x, water_level, xz.y), vertex_alpha); // 先执行真实GPU角点变形。
            if !position.is_finite() {
                return None;
            } // 非有限变形不得污染粒子坐标。
            result += position * weight; // 再执行光栅化三角的线性位置插值。
        } // 完成三个顶点采样。
        result.is_finite().then_some(result) // 水平Gerstner位移和竖向高度都落在真实片面。
    } // 结束无分配固定预算查询。
} // 结束绘制水足迹扩展。

#[cfg(test)] // 无窗口验证实际三角与非线性变形的顺序。
mod tests {
    // 固定几何回归无需游戏或GPU。
    use super::*; // 使用生产采样器。
    #[test] // 稀疏网格中心必须由角点而非解析中心求高度。
    fn sparse_surface_interpolates_displaced_vertices() {
        // 同时验证水平偏移被插值。
        let footprint = PaintedWaterFootprint::from_visible_mesh(
            [1, 1],
            [0.0, 0.0],
            [10.0, 10.0],
            vec![1.0; 4],
        ); // 刻意使用极稀疏单格。
        let position = footprint
            .sample_displaced_position(Vec2::new(2.0, 8.0), 3.0, |p, _| {
                p + Vec3::new(p.x * p.x, p.z * p.z, 0.0)
            })
            .unwrap(); // 非线性角点形变让错误解析采样明显可见。
        assert!((position - Vec3::new(22.0, 83.0, 8.0)).length() < 0.0001); // 权重0.2、0.6、0.2插值实际角点。
        let other = footprint
            .sample_displaced_position(Vec2::new(8.0, 2.0), 3.0, |p, _| {
                p + Vec3::new(0.0, p.x * p.x, 0.0)
            })
            .unwrap(); // 覆盖另一个三角分支。
        assert!((other.y - 83.0).abs() < 0.0001); // 另一半单元保持同一Mesh对角线。
    } // 完成稀疏水面误差回归。
    #[test] // 边界缺水、坏坐标和非法角点必须拒绝。
    fn rejects_transparent_and_invalid_samples() {
        // 不从缺失数据推测水位。
        let footprint = PaintedWaterFootprint::from_visible_mesh(
            [1, 1],
            [0.0, 0.0],
            [1.0, 1.0],
            vec![1.0, 0.0, 1.0, 1.0],
        ); // 右上角为完全透明岸外。
        assert!(
            footprint
                .sample_displaced_position(Vec2::X, 0.0, |p, _| p)
                .is_none()
        ); // 透明片面不发射。
        assert!(
            footprint
                .sample_displaced_position(Vec2::splat(-1.0), 0.0, |p, _| p)
                .is_none()
        ); // 网格外不发射。
        assert!(
            footprint
                .sample_displaced_position(Vec2::NAN, 0.0, |p, _| p)
                .is_none()
        ); // 非有限候选在整数索引前拒绝。
        assert!(
            footprint
                .sample_displaced_position(Vec2::ZERO, f32::NAN, |p, _| p)
                .is_none()
        ); // 非有限静止水位拒绝。
        assert!(
            footprint
                .sample_displaced_position(Vec2::ZERO, 0.0, |_, _| Vec3::NAN)
                .is_none()
        ); // 坏GPU输入不发布NaN。
        assert_eq!(
            footprint.sample_displaced_position(Vec2::ONE, 2.0, |p, _| p),
            Some(Vec3::new(1.0, 2.0, 1.0))
        ); // 最大端点使用最后一个真实三角。
    } // 完成坏数据与岸边回归。
    #[test] // 岸带非线性波幅只能先在顶点求值，不能把插值Alpha传给整片。
    fn shore_alpha_is_evaluated_per_vertex() {
        // 几乎完整水面仍含一个较浅角点。
        let footprint = PaintedWaterFootprint::from_visible_mesh(
            [1, 1],
            [0.0, 0.0],
            [1.0, 1.0],
            vec![1.0, 1.0, 0.9, 1.0],
        ); // 左下顶点有独立岸带Alpha。
        let sampled = footprint
            .sample_displaced_position(Vec2::new(0.2, 0.4), 0.0, |p, alpha| {
                p + Vec3::Y * alpha * alpha
            })
            .unwrap(); // 实际三角权重0.6、0.2、0.2。
        assert!((sampled.y - 0.962).abs() < 0.000001); // 0.6×1²+0.2×0.9²+0.2×1²不同于0.98²。
    } // 完成逐顶点Alpha回归。
} // 结束自动测试。
