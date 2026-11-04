//! 矩形Clipmap完整边的显式拼接；作者关系持久化，姿态改变后重新验证，派生边界不改作者精度。
mod geometry;
mod resolve;
mod types;
pub(crate) fn is_empty(value: &std::sync::Arc<[ClipmapStitch]>) -> bool {
    value.is_empty()
}

pub use geometry::{clipmap_edge_geometry, clipmap_stitch_pair, nearest_clipmap_stitch_pose};
pub use resolve::{resolve_clipmap_stitches, validate_clipmap_stitches};
pub use types::{CLIPMAP_STITCH_SEGMENTS, ClipmapEdge, ClipmapEdgeProfile, ClipmapStitch};

#[cfg(test)]
mod tests;
