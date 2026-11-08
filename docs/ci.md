# SDK Windows CI

推送、Pull Request 或手动触发时，[Windows 工作流](../.github/workflows/windows-check.yml)使用 Rust/MSVC 检查默认与 validation 两种组合。

公开 CI 从仓库变量 `FEATHRISE_PATCH_REV` 取得 [feathrise/feathrise_patches](https://github.com/feathrise/feathrise_patches) 中与 Bevy 0.20.1 配套的完整 40 位提交。工作流检出后核对实际提交。将 SDK 与配套补丁仓同级检出后，可以在 SDK 根目录运行以下本地检查。

```powershell
cargo check --locked -p feathrise_runtime -p feathrise_map_content -p feathrise_scene -p feathrise_environment -p feathrise_fps_display -p feathrise_plugin_api -p feathrise_sdk_workspace_tests --tests --examples
cargo test --locked -p feathrise_plugin_api --all-features --lib
cargo build --manifest-path examples/native_plugin/Cargo.toml --target-dir target --lib
```

复现 validation 检查时，在第一条命令追加 `--features feathrise_runtime/validation`。本地 FSR 构建需先提供 [Windows 原生包](../crates/feathrise_fsr3_frame_generation/prebuilt/README.md)。
