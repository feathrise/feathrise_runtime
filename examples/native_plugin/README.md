# 独立作者插件示例

只使用 feathrise_plugin_api，不依赖 Bevy、RPG 或编辑器源码。它增加会话计数工具，不保存数据，也不提供工程运行逻辑。

本仓联调在 SDK 根目录执行，沿用现有 target：

    cargo build --manifest-path examples/native_plugin/Cargo.toml --target-dir target --lib
    cargo run --locked -p feathrise_plugin_api --features reader --bin feathrise-plugin-meta -- inspect target/debug/feathrise_example_author_plugin.dll --json

Linux／macOS 分别使用对应的 .so／.dylib。编译成功不表示各平台加载和真实编辑器交互已经验收。

接口首次发布到 crates.io 后，把两个依赖中的 path 删除、保留 version，即可在自己的工程中构建，无需检出整个 SDK 仓库。普通接口不依赖渲染补丁；插件自己的其他依赖需要时，才按其说明配置补丁。

通过已安装编辑器的插件管理器安装本平台产物。未打开工程时，全局安装在后台事务完成后直接生效，打开工程才加载作者工具；已有工程内的变更需重新打开工程。编辑器程序用于实际加载和交互测试；插件编译不读取编辑器源码或 target。
