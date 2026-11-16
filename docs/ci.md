# SDK Windows CI

推送、Pull Request 或手动触发时，[Windows 工作流](../.github/workflows/windows-check.yml)使用 Rust/MSVC 检查默认与 validation 两种组合。

默认只检出 SDK 并按本仓的 Cargo 清单和锁使用 Bevy 0.20.1 的公开补丁。在 SDK 根目录即可运行以下检查。需要联调其他补丁提交时，设置仓库变量 `FEATHRISE_PATCH_REV` 为完整 40 位提交并手动运行。工作流会核对实际提交，经其 Cargo 配置解析联调锁后编译。

```powershell
cargo check --locked -p feathrise_runtime -p feathrise_scene_core -p feathrise_scene_content -p feathrise_scene -p feathrise_environment -p feathrise_fps_display -p feathrise_plugin_api -p feathrise_sdk_workspace_tests --tests --examples
cargo test --locked -p feathrise_plugin_api -p feathrise_editor_protocol -p feathrise_scene_content -p feathrise_scene_core --all-features --lib
cargo build --manifest-path examples/native_plugin/Cargo.toml --target-dir target --lib
```

复现 validation 检查时，在第一条命令追加 `--features feathrise_runtime/validation`。本地 FSR 构建需先提供 [Windows 原生包](../crates/feathrise_fsr3_frame_generation/prebuilt/README.md)。
