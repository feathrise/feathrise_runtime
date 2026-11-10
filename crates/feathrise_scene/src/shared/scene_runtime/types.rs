//! 数据驱动场景公共场景环境配置。

use bevy::prelude::*;

/// 场景环境配置（天空与灯光参数）
pub struct SceneEnvironment {
    pub clear_color: Color,
    pub light_illuminance: f32,
    pub light_color: Color,
    pub light_rotation: (f32, f32), // (y_rad, z_rad) for EulerRot::ZYX
}
