//! 静态网格LOD完成值；误差使用源局部米制，实例缩放由运行时保守换算。
use bevy::prelude::Mesh;

pub struct StaticMeshLod {
    pub mesh: Mesh,
    pub max_error: f32,
}
