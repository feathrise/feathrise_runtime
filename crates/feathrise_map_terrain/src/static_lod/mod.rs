//! 普通静态网格的纯CPU派生入口；调用方先有界捕获，全部计算只在后台执行。
mod attributes;
mod capture;
mod build;
mod types;
mod view;

pub use attributes::{copy_static_mesh_attribute_range, empty_static_mesh_attribute};
pub use capture::{StaticMeshCapture, static_mesh_bytes};
pub use build::build_static_mesh_lods;
pub use types::StaticMeshLod;
pub use view::StaticMeshLodView;

#[cfg(test)]
mod tests;
