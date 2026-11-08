//! 遮雨体GPU布局，每体96字节，世界高度和XZ范围提供便宜早退。

use bevy::prelude::*; // 四阶矩阵和四向量。
use bevy::render::render_resource::ShaderType; // 与生产WGSL存储缓冲共享对齐。

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType, bytemuck::Pod, bytemuck::Zeroable)] // 完整批次可以固定大小按值发布。
pub struct RainShelterVolume {
    // 不含任何Mesh、材质或碰撞资源。
    pub world_to_local: Mat4, // 世界向上射线进入作者局部盒子的逆矩阵。
    pub half_extents: Vec4,   // XYZ局部半径，W世界最大Y。
    pub world_bounds: Vec4,   // 世界最小X/Z和最大X/Z，远离屋顶时免除矩阵计算。
} // 完成固定体布局。
