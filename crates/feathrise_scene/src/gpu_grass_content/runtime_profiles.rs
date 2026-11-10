//! 动态场景相机的通用 Blade 视图基线；预设身份与完整参数来自工程目录。

use feathrise_environment::vegetation::blade::GpuBladeViewLodConfig;
use bevy::prelude::{Vec2, Vec3, Vec4};

/// 动态场景相机的启动期屏幕LOD基线；每个数据预设仍携带自己的完整视图参数。
pub fn dynamic_scene_blade_view_lod() -> GpuBladeViewLodConfig {
    GpuBladeViewLodConfig::new_with_ultra(
        Vec2::new(3.5, 5.0),
        Vec2::new(12.0, 20.0),
        Vec2::new(40.0, 56.0),
        Vec2::new(0.18, 0.55),
        0.70,
        0.0875,
        Vec4::new(46.0, 104.0, 220.0, 1220.0),
        2.0,
        Vec3::new(8.0, 16.0, 64.0),
    ) // 相机只需通用启动基线；实际窗口仍读取各预设数据。
}
