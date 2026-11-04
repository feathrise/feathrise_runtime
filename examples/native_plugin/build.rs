//! 从Cargo身份生成插件元数据；示例不声明工程运行端。
use feathrise_plugin_api::{PluginMetadata, write_plugin_metadata};
fn main() {
    write_plugin_metadata(PluginMetadata {
        format: 1, id: "example.counter".into(), tool: "计数示例".into(),
        version: String::new(), target_os: String::new(), target_arch: String::new(),
        abi_major: feathrise_plugin_api::ABI_MAJOR,
        runtime_dependencies: Vec::new(), runtime: None,
    }).expect("示例元数据生成失败");
}
