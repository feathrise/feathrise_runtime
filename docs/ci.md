# SDK Windows CI

推送、Pull Request 或手动触发时，[Windows 工作流](../.github/workflows/windows-check.yml)使用 Rust/MSVC 检查默认与 validation 两种组合。

默认只检出 SDK 并按本仓的 Cargo 清单和锁使用 Bevy 0.20.1 的公开补丁。在 SDK 根目录即可运行以下检查。需要联调其他补丁提交时，设置仓库变量 `FEATHRISE_PATCH_REV` 为完整 40 位提交并手动运行。工作流会核对实际提交，经其 Cargo 配置解析联调锁后编译。

两组作业共享 Cargo 下载缓存。编译产物缓存按 Rust 工具链、补丁来源、功能组合及依赖清单区分，源码修改时由 Cargo 判定需要重编译的目标。CI 关闭增量编译、调试符号和第三方 Rust 依赖的高优化，普通开发与发布继续使用各自的 Cargo profile。

```powershell
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$ciProfile = @('--config', 'profile.dev.package."*".opt-level=0')
cargo @ciProfile check --locked -p feathrise_runtime -p feathrise_scene_core -p feathrise_scene_content -p feathrise_scene -p feathrise_environment -p feathrise_fps_display -p feathrise_plugin_api -p feathrise_sdk_workspace_tests --tests --examples
cargo @ciProfile test --locked -p feathrise_plugin_api -p feathrise_editor_protocol -p feathrise_scene_content -p feathrise_scene_core --all-features --lib
cargo build --manifest-path examples/native_plugin/Cargo.toml --target-dir target --lib
```

复现 validation 检查时，在 `cargo check` 命令追加 `--features feathrise_runtime/validation`。契约测试和独立原生插件构建由 default 作业各运行一次。本地 FSR 构建需先提供 [Windows 原生包](../crates/feathrise_fsr3_frame_generation/prebuilt/README.md)。
