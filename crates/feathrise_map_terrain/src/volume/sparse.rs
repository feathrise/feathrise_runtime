//! 第二版作者源：世界格点唯一归属、按需体积块、局部重建及有界共享历史。
mod brush;
mod codec;
mod mesh;
pub mod paged;
mod simplify;
pub use simplify::simplify_display_mesh; // 独立体素显示与碰撞LOD复用保留接缝和洞口的减面策略。
mod types;
pub use brush::{
    apply_sparse_brush, apply_sparse_brush_batch, migrate_dense_volume, new_sparse_volume,
};
pub use codec::{decode_author_volume_glb, encode_author_volume_glb};
pub use mesh::build_author_volume_mesh;
pub use types::{
    MAX_AUTHOR_VOLUME_GLB_BYTES, SPARSE_CELL_SIZE, SPARSE_CHUNK_CELLS, SparseVolume,
    VolumeMemoryBudget, VolumeSource,
};
#[cfg(test)]
mod mesh_tests;
#[cfg(test)]
mod tests;
