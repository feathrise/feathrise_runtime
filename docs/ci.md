# SDK Windows CI

推送、Pull Request 或手动触发时，[Windows 工作流](../.github/workflows/windows-check.yml)使用 Rust/MSVC 检查默认与 validation 两种组合。

公开 CI 已在工作流中固定补丁仓 [hwyrq/feathrise_patches](https://github.com/hwyrq/feathrise_patches)，提交为 `3c251d3530980b66e49b4748769db36ae9c6bd5a`，自动检出并校验修订。本地把 SDK 与补丁仓同级检出后，在 SDK 根目录执行：

```powershell
cargo check --locked -p feathrise_runtime -p feathrise_map_content -p feathrise_scene -p feathrise_environment -p feathrise_fps_display -p feathrise_plugin_api -p feathrise_sdk_workspace_tests --tests --examples
cargo test --locked -p feathrise_plugin_api --all-features --lib
cargo build --manifest-path examples/native_plugin/Cargo.toml --target-dir target --lib
```

复现 validation 检查时，在第一条命令追加 `--features feathrise_runtime/validation`。本地 FSR 构建需先提供 [Windows 原生包](../crates/feathrise_fsr3_frame_generation/prebuilt/README.md)。
