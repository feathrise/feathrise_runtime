//! 三维密度雕刻保留旧盒子源兼容，分块层自动扩展；数组写入、网格化及编码只由后台调用。
mod batches; // 长笔划的固定大小窗口与全局连续刷印，不把整条路径复制成数组。
mod brush;
mod codec;
mod mesh;
mod path;
pub mod sparse;
mod types;
pub use brush::{apply_volume_brush, volume_from_heights};
pub use codec::{decode_volume_glb, encode_volume_glb};
pub use mesh::build_volume_mesh;
pub use mesh::build_density_mesh; // 独立体素和高度派生体积共用同一接缝稳定的等值面实现。
pub use mesh::build_density_mesh_fractional;
pub use path::{VolumePathBatches, VolumePathParameters, VolumePathPlan, VolumePathSection};
pub use types::*;
#[cfg(test)]
mod shape_tests;
#[cfg(test)]
mod tests;
