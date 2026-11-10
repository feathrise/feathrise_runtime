//! 原生编辑器插件的唯一二进制边界；所有Rust和Bevy对象都留在各自模块内。
mod ffi;
pub mod metadata;
pub mod scene_domain;
mod protocol;
pub use ffi::*;
pub use metadata::*;
pub use protocol::*;
pub use serde_json;
#[cfg(test)]
mod tests;
