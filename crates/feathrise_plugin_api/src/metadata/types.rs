//! 作者插件安装、发现与动态握手共用的元数据；Rust结构不跨C ABI传递。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginMetadata {
    pub format: u32,
    pub id: String,
    pub tool: String,
    pub version: String,
    pub target_os: String,
    pub target_arch: String,
    pub abi_major: u32,
    pub runtime_dependencies: Vec<String>,
    #[serde(default)]
    pub runtime: Option<RuntimeMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeMetadata {
    pub crate_name: String,
    pub api: u32,
    pub preview_kind: Option<String>,
    pub entry: String,
}
