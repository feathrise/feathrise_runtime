# crates.io 接入与本地开发

自有源码及各子包采用 MIT OR Apache-2.0，第三方组件保留原许可。

SDK 当前先通过 [GitHub 源码仓](https://github.com/hwyrq/feathrise_runtime) 提供，下面的 registry 版本配置用于 crates.io 发布后。现在可按[最小工程示例](quickstart.md)克隆 SDK 与配套补丁仓，直接使用源码。

编辑器的原生运行库部署身份由无依赖的 `feathrise_runtime_manifest` 提供；帧生成包的构建与启动也消费同一清单。它需先于实际依赖它的包发布，单独使用清单不会编译 Bevy 或厂商桥。SDK 源码仓包含 23 个发布包：根包与 22 个独立子库，另有一个外部字体总包及其三个分片。发布顺序应从当前 Cargo 普通／构建依赖图生成。

`feathrise_plugin_api` 是本仓独立的作者插件接口包，可单独依赖。它没有编辑器或 Bevy 依赖，普通插件只需该包，无需下面的渲染补丁或 SDK 仓库检出；开发示例、可选功能和独立检查器见[插件开发](native-plugins.md)。启用其他运行能力时，才按其依赖另行接入。

根包直接包含 `project` 工程启动模块和 `app` 运行装配模块。场景基础契约统一由 `feathrise_scene_core` 的 `constants`、`view`、`state`、`input`、`residency` 模块提供；这些模块按同一版本发布，不再分别维护小包。`feathrise_plugin_api`、作者工具合同、进程通信协议和原生运行库身份仍保持各自独立边界。

## 外部工程

在工程根声明版本依赖及八项配套补丁。以下配置固定 SDK 0.1.0 配套的公开补丁提交。

```toml
[dependencies]
feathrise_runtime = "=0.1.0"

[patch.crates-io]
avian3d = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
bevy_anti_alias = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
bevy_render = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
bevy_core_pipeline = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
wgpu-hal = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
parley = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
dlss_wgpu = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
wgpu-ffx = { git = "https://github.com/hwyrq/feathrise_patches.git", rev = "3c251d3530980b66e49b4748769db36ae9c6bd5a" }
```

所有补丁采用同一个实际公开 URL 和与 SDK 版本配套的完整提交。Cargo 自动取得源码，不要求手工维护本地 SDK。库内 `[patch]` 不会向上传递；使用 SDK 的工程应把它放在工作区根。

## 本地修改

SDK 和 `feathrise_patches` 同级检出后，在 SDK 目录直接运行 Cargo。SDK 自己的 `.cargo/config.toml` 会加载同级补丁配置。补丁仓在其他位置时，用 `cargo --config <补丁配置路径>` 指定。

自己的工程可使用 `feathrise_runtime = { version = "0.1.0", path = "../feathrise_runtime" }`，并配置同级 `feathrise_patches/.cargo/config.toml` 中的八项本地补丁。SDK 内部的子库也使用 `version + path`；打包时 Cargo 移除路径、采用 registry 版本。这不是路径失效后的自动回退。

编辑器的 Cargo 清单声明 SDK 版本。联调本地源码时，将 SDK 自有包和独立补丁仓的八项补丁一并配置到本机 Cargo 的 `[patch.crates-io]`，再通过 `cargo --config .cargo/local-sdk.toml` 使用。公开 CI 分别检出固定版本的 SDK 与补丁仓，并生成本地路径覆盖。
