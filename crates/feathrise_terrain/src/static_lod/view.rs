//! 正式场景与独立作者视口共用的真实屏幕误差投影；不以固定距离冒充LOD误差。

use bevy::prelude::*;

#[derive(Clone)]
pub struct StaticMeshLodView {
    pub inverse: Mat4,
    pub focal_pixels: f32,
    pub orthographic: bool,
    pub layers: bevy::camera::visibility::RenderLayers,
}

impl StaticMeshLodView {
    /// 最靠近相机的包围点决定误差上界；穿过近面时保留原始精度。
    pub fn error_pixels(&self, global: &GlobalTransform, radius: f32, error: f32) -> f32 {
        let point = self.inverse.transform_point3(global.translation());
        let matrix = global.to_matrix();
        let scale = Vec3::new(matrix.x_axis.truncate().length(), matrix.y_axis.truncate().length(), matrix.z_axis.truncate().length()).length();
        // Frobenius范数是含shear伸缩的上界，最长单轴会低估几何误差。
        if self.orthographic { error * scale * self.focal_pixels } else {
            let depth = -point.z - radius * scale;
            if depth <= 0.01 { f32::INFINITY } else { error * scale * self.focal_pixels / depth }
        }
    }

    /// 已采用层的退回阈值比新层宽；所有实际目标视图共同满足才允许简化。
    pub fn accepts(&self, global: &GlobalTransform, radius: f32, error: f32,
        max_error_pixels: f32, hysteresis_ratio: f32, currently_applied: bool) -> bool {
        let limit = max_error_pixels * if currently_applied { 1.0 + hysteresis_ratio } else { 1.0 - hysteresis_ratio };
        self.error_pixels(global, radius, error) <= limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn near_plane_and_shear_use_conservative_projection_and_hysteresis() {
        let view = StaticMeshLodView { inverse: Mat4::IDENTITY, focal_pixels: 500.0,
            orthographic: false, layers: Default::default() };
        assert!(view.error_pixels(&GlobalTransform::IDENTITY, 1.0, 0.01).is_infinite());
        let far = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, -100.0));
        // 单位矩阵的Frobenius上界仍是sqrt(3)，不是1；真实误差约0.881px。
        // 新层门槛0.85px应拒绝，已采用层的1.15px门槛允许保留，不能为测试放松公式。
        let error = 0.1 * 3.0_f32.sqrt() * 500.0 / (100.0 - 3.0_f32.sqrt());
        assert!((view.error_pixels(&far, 1.0, 0.1) - error).abs() < 0.00001);
        assert!(!view.accepts(&far, 1.0, 0.1, 1.0, 0.15, false));
        assert!(view.accepts(&far, 1.0, 0.1, 1.0, 0.15, true));
        assert!(view.accepts(&far, 1.0, 0.09, 1.0, 0.15, false));
        assert!(!view.accepts(&far, 1.0, 1.0, 1.0, 0.15, true));
        let shear = GlobalTransform::from(Mat4::from_cols(Vec4::X, Vec4::new(1.0, 1.0, 0.0, 0.0),
            Vec4::Z, Vec4::new(0.0, 0.0, -100.0, 1.0)));
        assert!((view.error_pixels(&shear, 1.0, 0.1) - (0.1 * 2.0 * 500.0 / 98.0)).abs() < 0.00001);
    }
}
