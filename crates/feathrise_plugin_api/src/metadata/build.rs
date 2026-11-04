//! build.rs复用入口：版本和目标平台由Cargo提供，不允许手写清单与实际产物分叉。
use super::{PluginMetadata, encode_plugin_metadata};
use std::{fmt::Write, path::PathBuf};

/// 写入OUT_DIR/feathrise_plugin_metadata.rs，供export_plugin!(Tool, embedded)引用。
pub fn write_plugin_metadata(mut metadata: PluginMetadata) -> Result<(), String> {
    for key in [
        "CARGO_PKG_VERSION",
        "CARGO_CFG_TARGET_OS",
        "CARGO_CFG_TARGET_ARCH",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    metadata.version = std::env::var("CARGO_PKG_VERSION").map_err(|error| error.to_string())?;
    metadata.target_os = std::env::var("CARGO_CFG_TARGET_OS").map_err(|error| error.to_string())?;
    metadata.target_arch =
        std::env::var("CARGO_CFG_TARGET_ARCH").map_err(|error| error.to_string())?;
    let bytes = encode_plugin_metadata(&metadata)?;
    let mut source = format!(
        "// 由Cargo版本与目标生成；静态节和导出getter共用。\nconst FEATHRISE_METADATA_BYTES: [u8; {}] = [",
        bytes.len()
    );
    for byte in bytes {
        write!(&mut source, "{byte},").map_err(|error| error.to_string())?;
    }
    source.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("缺少Cargo OUT_DIR")?);
    std::fs::write(out.join("feathrise_plugin_metadata.rs"), source)
        .map_err(|error| error.to_string())
}
