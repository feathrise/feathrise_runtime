//! 全局静态网格简化偏好；两条通道独立，损坏参数回退原始质量而不禁用物理。

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MeshLodSettings {
    pub display_enabled: bool,
    pub max_error_pixels: f32,
    pub hysteresis_ratio: f32,
    pub collision_enabled: bool,
    pub collision_safe_near_meters: f32,
    pub collision_max_error_meters: f32,
}

impl Default for MeshLodSettings {
    fn default() -> Self {
        Self {
            display_enabled: true,
            max_error_pixels: 1.0,
            hysteresis_ratio: 0.15,
            collision_enabled: false,
            collision_safe_near_meters: 32.0,
            collision_max_error_meters: 0.02,
        }
    }
}

impl MeshLodSettings {
    /// 设置文件允许保留其他有效偏好，但坏通道不能触发简化；原始几何/碰撞继续使用。
    pub fn sanitized(self) -> Self {
        let defaults = Self::default();
        let display_valid = self.max_error_pixels.is_finite()
            && (0.1..=16.0).contains(&self.max_error_pixels)
            && self.hysteresis_ratio.is_finite()
            && (0.0..=0.5).contains(&self.hysteresis_ratio);
        let collision_valid = self.collision_safe_near_meters.is_finite()
            && (32.0..=4096.0).contains(&self.collision_safe_near_meters)
            && self.collision_max_error_meters.is_finite()
            && (0.0001..=0.25).contains(&self.collision_max_error_meters);
        // 边界与共享作者MeshLodPolicy一致；偏好类型不依赖地图资产，避免渲染设置反向耦合。
        Self {
            display_enabled: self.display_enabled && display_valid,
            max_error_pixels: if display_valid { self.max_error_pixels } else { defaults.max_error_pixels },
            hysteresis_ratio: if display_valid { self.hysteresis_ratio } else { defaults.hysteresis_ratio },
            collision_enabled: self.collision_enabled && collision_valid,
            collision_safe_near_meters: if collision_valid { self.collision_safe_near_meters } else { defaults.collision_safe_near_meters },
            collision_max_error_meters: if collision_valid { self.collision_max_error_meters } else { defaults.collision_max_error_meters },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_mesh_lod_defaults_enable_display_and_preserve_original_collision() {
        let value = MeshLodSettings::default();
        assert!(value.display_enabled);
        assert!(!value.collision_enabled);
        assert_eq!(value.sanitized(), value);
    }

    #[test]
    fn invalid_channel_disables_only_its_simplification_and_preserves_valid_custom_values() {
        let custom = MeshLodSettings {
            max_error_pixels: 0.5, hysteresis_ratio: 0.3,
            collision_enabled: true, collision_safe_near_meters: 64.0, collision_max_error_meters: 0.005,
            ..Default::default()
        };
        assert_eq!(custom.sanitized(), custom);
        for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0, 17.0] {
            let result = MeshLodSettings { max_error_pixels: bad, ..custom }.sanitized();
            assert!(!result.display_enabled);
            assert!(result.collision_enabled);
            assert_eq!(result.collision_safe_near_meters, custom.collision_safe_near_meters);
            assert_eq!(result.max_error_pixels, 1.0);
        }
        let result = MeshLodSettings { collision_safe_near_meters: 31.0, ..custom }.sanitized();
        assert!(result.display_enabled);
        assert!(!result.collision_enabled);
        assert_eq!(result.max_error_pixels, custom.max_error_pixels);
        assert_eq!(result.collision_safe_near_meters, 32.0);
        assert!(!MeshLodSettings { collision_max_error_meters: f32::NAN, ..custom }.sanitized().collision_enabled);
        assert!(!MeshLodSettings { hysteresis_ratio: 0.51, ..custom }.sanitized().display_enabled);
    }
}
