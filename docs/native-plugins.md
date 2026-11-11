# 无编辑器源码的插件开发

feathrise_plugin_api 是 SDK 仓库中的独立 Rust 包，不是新的 Git 仓库。它提供 C 函数表、请求／快照、元数据生成与静态检查工具，不依赖 Bevy 或编辑器实现。

场景版宿主的插件在 `[dependencies]` 和 `[build-dependencies]` 中都通过路径引用配套的 `feathrise_plugin_api` 源码。使用 `NativeTool` 实现自己的逻辑，在 `build.rs` 调用 `write_plugin_metadata`，再调用 `export_plugin!(Tool, embedded)` 导出同源元数据和函数表。 [中性计数示例](../examples/native_plugin/README.md)

开发者使用 Rust 工具链和配套的接口包源码。需要使用共享运行能力的插件，再单独声明对应 crate 和它要求的补丁。编辑器程序用于实际安装、加载和交互测试。

在配套 SDK 根运行元数据检查器。

    cargo run --locked -p feathrise_plugin_api --features reader --bin feathrise-plugin-meta -- inspect <本平台插件文件> --json

该命令复用 SDK 的 `target`。检查器静态读取文件，实际插件在宿主进程中运行。

每个平台交付一个 DLL／SO／DYLIB。Rust 容器、trait、Bevy World 和实体引用不跨二进制边界；会话在插件内部创建和销毁，响应字节只在回调期间借用。场景版 ABI 主版本为 2，消息能力随接口与宿主版本配套维护；发布说明应明确兼容的宿主版本，旧宿主不能默默接受新消息能力。

场景插件可开启 blob-io，复用有界、内容寻址的不可变文件传输。读写只在原有后台任务中执行，完整回执仍由宿主核对身份后采用。启用 reader 才引入二进制静态解析器；reader-test-support 仅提供测试夹具。

本接口解决作者工具接入。通用二进制运行插件、最终工程的运行端收集及闭源双端生命周期仍是另一个功能，不以作者 DLL 编译成功宣称完成。
