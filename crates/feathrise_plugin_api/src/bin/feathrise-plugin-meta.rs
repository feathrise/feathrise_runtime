//! 给打包/安装工具复用的静态检查命令；解析文件但不加载插件代码。
use feathrise_plugin_api::read_plugin_metadata;
use std::path::Path;

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 3 || arguments[0] != "inspect" || arguments[2] != "--json" {
        return Err("用法：feathrise-plugin-meta inspect <file> --json".into());
    }
    let metadata = read_plugin_metadata(Path::new(&arguments[1]))?;
    println!(
        "{}",
        serde_json::to_string(&metadata).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
