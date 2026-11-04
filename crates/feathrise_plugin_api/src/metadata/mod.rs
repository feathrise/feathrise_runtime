//! 单文件作者插件的静态元数据合同；安装检查不执行动态库中的任何代码。
mod build;
mod codec;
#[cfg(feature = "reader")]
mod reader;
#[cfg(feature = "reader-test-support")]
pub mod test_support;
#[cfg(test)]
mod tests;
mod types;

pub use build::write_plugin_metadata;
pub use codec::{decode_plugin_metadata, encode_plugin_metadata};
#[cfg(feature = "reader")]
pub use reader::read_plugin_metadata;
pub use types::*;

pub const METADATA_FORMAT: u32 = 1;
pub const METADATA_MAGIC: &[u8; 16] = b"FEATHRISE_META\0\0";
pub const METADATA_HEADER_BYTES: usize = 24;
pub const MAX_METADATA_JSON_BYTES: usize = 64 * 1024;
pub const MAX_METADATA_BYTES: usize = METADATA_HEADER_BYTES + MAX_METADATA_JSON_BYTES;
pub const METADATA_SYMBOL: &[u8] = b"feathrise_plugin_metadata_v1\0";

pub type MetadataEntry = unsafe extern "C" fn(*mut std::ffi::c_void, crate::WriteResponse) -> i32;

/// 安装当前宿主无法使用的二进制没有意义；文件格式与实际CPU头由静态读取器另外核对。
pub fn validate_current_platform(metadata: &PluginMetadata) -> Result<(), String> {
    codec::validate(metadata)?;
    if metadata.target_os != std::env::consts::OS || metadata.target_arch != std::env::consts::ARCH
    {
        return Err(format!(
            "插件目标为{}-{}，当前编辑器为{}-{}",
            metadata.target_os,
            metadata.target_arch,
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    Ok(())
}
