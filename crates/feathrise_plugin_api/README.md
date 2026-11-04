# Feathrise 作者插件 SDK

本包提供原生作者插件 ABI，可作为独立依赖使用。开发流程与中性示例见[独立插件开发](../../docs/native-plugins.md)；自有源码采用包内 MIT OR Apache-2.0 双许可。

作者插件交付当前平台的单个 `.dll`、`.so` 或 `.dylib`，不使用外部清单或压缩包。安装器通过 `reader` feature 的 `read_plugin_metadata` 静态读取文件，不执行插件；`validate_current_platform` 再拒绝当前编辑器不能使用的平台／CPU。读文件不代表插件可信，实际加载仍在宿主同进程执行本机代码。

元数据节为 PE／ELF 的 `.frmeta`、Mach-O 的 `__DATA,__frmeta`。内容依次为16字节 `FEATHRISE_META\0\0`、小端 `u32` 格式1、小端 `u32` JSON长度及不超过64KiB的 UTF-8 JSON。JSON类型为 `PluginMetadata`／`RuntimeMetadata`；后者只是现有源码运行端接入说明，不是通用二进制运行端接口。读取器核对实际共享库格式、架构、后缀和声明，并限制文件／节表／节数据读取预算。

插件的 `build.rs` 调用 `write_plugin_metadata`，由 Cargo 的包版本、目标OS和目标CPU生成 `OUT_DIR/feathrise_plugin_metadata.rs`；这三项不依靠手填。插件声明 `feathrise_plugin_api::export_plugin!(ToolType, embedded)`，同时产生静态节及 `feathrise_plugin_metadata_v1(context, WriteResponse) -> i32`。getter返回**包含固定头的完整字节数组**，和静态节同源，宿主使用 `decode_plugin_metadata` 解码后核对身份。回调只在调用期间借用字节，Rust对象及内存释放不跨库。

原 `feathrise_plugin_api_v1` 函数表和 create／invoke／destroy ABI保持。只使用旧的单参数导出宏无法生成新格式安装所需的元数据，旧目录包需要重新构建。

维护者在 `feathrise_runtime/` 执行，复用 SDK 原 target：

```powershell
cargo build --locked -p feathrise_plugin_api --features reader --bin feathrise-plugin-meta
target/debug/feathrise-plugin-meta inspect <插件单文件> --json
cargo test --locked -p feathrise_plugin_api --features reader-test-support
```

发布后可通过 `cargo install feathrise_plugin_api --version "=0.1.0" --locked --features reader --bin feathrise-plugin-meta` 独立安装检查命令。普通插件只依赖默认接口；地图文件传输显式开启 `blob-io`，复用后台有界读写和 SHA-256 身份，不引入编辑器实现。

CLI静态检查所有已实现的格式；安装是否适用于当前平台由调用方再执行 `validate_current_platform`。`reader-test-support` 只给测试生成带真实共享库头的解析夹具，不能用于证明 DLL 加载或完整作者业务通过。Linux/macOS代码需要各自实机另验。
