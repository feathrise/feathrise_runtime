# crates.io 接入与本地开发

自有源码及各子包采用 MIT OR Apache-2.0，第三方组件保留原许可。

SDK 0.3.0 与 Bevy 0.20.1、wgpu 30.0.1 配套。历史 Bevy 0.19.1 工程可继续使用 [crates.io 上的 SDK 0.1.0](https://crates.io/crates/feathrise_runtime/0.1.0)。需要修改 SDK 或运行仓内示例时，可按[最小工程示例](quickstart.md)克隆源码与配套补丁仓。

编辑器的原生运行库部署身份由无依赖的 `feathrise_runtime_manifest` 提供；帧生成包的构建与启动也消费同一清单。它需先于实际依赖它的包发布，单独使用清单不会编译 Bevy 或厂商桥。SDK 源码仓包含 23 个发布包：根包与 22 个独立子库，另有一个外部字体总包及其三个分片。发布顺序应从当前 Cargo 普通／构建依赖图生成。

`feathrise_plugin_api` 是本仓独立的作者插件接口包，可单独依赖。它没有编辑器或 Bevy 依赖，普通插件只需该包，无需下面的渲染补丁或 SDK 仓库检出；开发示例、可选功能和独立检查器见[插件开发](native-plugins.md)。启用其他运行能力时，才按其依赖另行接入。

根包直接包含 `project` 工程启动模块和 `app` 运行装配模块。场景基础契约统一由 `feathrise_scene_core` 的 `constants`、`view`、`state`、`input`、`residency` 模块提供；这些模块按同一版本发布，不再分别维护小包。`feathrise_plugin_api`、作者工具合同、进程通信协议和原生运行库身份仍保持各自独立边界。

## 发布前检查

当前 Bevy 0.20.1 源码要求 Rust 和 Cargo 1.97.1 或更新版本。以下命令在 SDK 根目录检查整个待发布集合。

```powershell
cargo package --workspace --exclude feathrise_sdk_workspace_tests --registry crates-io --no-verify --locked
```

Cargo 按依赖关系为 23 个公开包生成归档。测试工作区保持 `publish = false`。归档阶段用 `--no-verify` 跳过重复编译，解包后的消费编译单独执行。尚有未提交改动时，预检可追加 `--allow-dirty`。参数见 [Cargo 打包说明](https://doc.rust-lang.org/cargo/commands/cargo-package.html)。

源码编译另行检查，例如默认与验收组合：

```powershell
cargo check --locked -p feathrise_runtime
cargo check --locked -p feathrise_runtime --features validation
```

发布前还需在独立目录检查归档的源码和测试输入，用候选归档及配套 Git 补丁做消费编译。发布后再从 crates.io 下载实际版本进行验收。

## 手动发布 0.3.0

在干净的 SDK 检出中执行。工作区批量发布使用 crates.io 的直接索引，按依赖顺序处理 23 个 SDK 包。字体继续使用已发布的 `feathrise_font = "=0.1.1"`。

```powershell
cargo publish --workspace --exclude feathrise_sdk_workspace_tests --registry crates-io --locked --no-verify
```

预检阶段已单独进行解包编译时可使用 `--no-verify`，省去发布阶段再次构建每个包。正式命令需要对这 23 个包有发布权限的 crates.io 凭据。参见 [Cargo 发布说明](https://doc.rust-lang.org/cargo/commands/cargo-publish.html)。

## 外部工程

在工程根声明 SDK 版本依赖及八项配套补丁。

### SDK 0.3.0 / Bevy 0.20.1

等整组 0.3.0 在 crates.io 可见后，使用以下配置。应用若直接依赖 Bevy，应同时固定到 `=0.20.1`。

```toml
[dependencies]
feathrise_runtime = "=0.3.0"

[patch.crates-io]
avian3d = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
bevy_anti_alias = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
bevy_render = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
bevy_core_pipeline = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
bevy_extract = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
wgpu-hal = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
dlss_wgpu = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
wgpu-ffx = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "019fd1fe13ed9107d17b360b8f5a3df61bfc139a" }
```

### 历史 SDK 0.1.0 / Bevy 0.19.1

```toml
[dependencies]
feathrise_runtime = "=0.1.0"

[patch.crates-io]
avian3d = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
bevy_anti_alias = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
bevy_render = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
bevy_core_pipeline = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
wgpu-hal = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
parley = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
dlss_wgpu = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
wgpu-ffx = { git = "https://github.com/feathrise/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
```

所有补丁采用同一个实际公开 URL 和与 SDK 版本配套的完整提交。Cargo 自动取得源码，不要求手工维护本地 SDK。库内 `[patch]` 不会向上传递；使用 SDK 的工程应把它放在工作区根。

首次配置独立工程时，先执行 `cargo check` 生成锁文件，再以 `cargo check --locked` 复验。用 `cargo metadata --format-version 1` 核对实际来源：SDK 自有包应来自 crates.io，八项约定补丁来自固定 Git 修订；工程自己的源码保留本地路径。默认功能与关闭默认功能应分别检查，可选原生组合另外验证。首次配置尚无锁文件，不直接要求 `--locked`。

## 可选 Windows 原生能力

默认 `raytracing`／`default_font` 不要求 DLSS 或 FSR 的厂商 SDK。启用 `rtx_rendering` 需要 DLSS SDK、Vulkan SDK、LLVM／Clang 和 MSVC；启用 `dlss`／`dlss_frame_generation` 还需要固定 Streamline 2.12.0。`DLSS_SDK`、`LIBCLANG_PATH`／`CLANG_PATH`、`STREAMLINE_SDK` 与 `VULKAN_SDK` 的完整要求和检查命令见 [DLSS 帧生成包](../crates/feathrise_frame_generation/README.md)。

FSR 帧生成可以显式提供 `FEATHRISE_FSR3_PACKAGE` 指向已有固定原生 ZIP；没有预编译包时，也可使用公开单包内的脚本显式从源码准备，配置 `FEATHRISE_FSR3_BUILD_FROM_SOURCE=1`、`FIDELITYFX_SDK`、`VULKAN_SDK` 与 `FEATHRISE_FSR3_PATCH_ROOT`。Windows 源码构建示例还提供独立短目录 `FEATHRISE_FSR3_SOURCE_BUILD_ROOT`，避免 MSVC 生成头文件的长路径限制；具体步骤和固定补丁修订见 [FSR 原生包准备](../crates/feathrise_fsr3_frame_generation/prebuilt/README.md)。普通 Cargo 构建不自动下载原生包。

## 本地修改

将 SDK 和 Bevy 0.20.1 配套的 `feathrise_patches` 同级检出后，在 SDK 目录运行 Cargo。SDK 自己的 `.cargo/config.toml` 会加载同级补丁配置。补丁仓在其他位置时，用 `cargo --config <补丁配置路径>` 指定。

自己的工程可使用 `feathrise_runtime = { version = "0.3.0", path = "../feathrise_runtime" }`，同时锁定 Bevy 0.20.1 并加载同级 `feathrise_patches/.cargo/config.toml` 中的本地补丁。SDK 内部的子库也使用 `version + path`。打包时 Cargo 移除路径、采用 registry 版本。

编辑器的 Cargo 清单声明 SDK 版本。联调本地源码时，将 SDK 自有包和独立补丁仓的八项补丁一并配置到本机 Cargo 的 `[patch.crates-io]`，再通过 `cargo --config .cargo/local-sdk.toml` 使用。公开 CI 分别检出固定版本的 SDK 与补丁仓，并生成本地路径覆盖。
