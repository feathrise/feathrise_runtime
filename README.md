# feathrise_runtime

Feathrise（羽立）编辑器与 Rust 工程共用的 Bevy 运行库，提供场景、资源、渲染、地形、水域、植被、物理和公共数据格式。工程通过自己的插件提供玩法、角色、内容预设、界面和存档策略。

自有源码采用 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE)，由使用者任选其一。第三方组件继续遵循 [各自原许可](THIRD_PARTY_NOTICES.md)。

## 使用与入口

SDK 0.3.0 使用 Bevy 0.20.1、wgpu 30.0.1 和 Rust 1.97.1 或更新版本。源码接入使用同级补丁仓。0.3.0 的发布步骤及配套 Git 补丁见[接入说明](docs/crates-io.md)，源码示例见[快速开始](docs/quickstart.md)。

作者插件开发见[独立插件接口与示例](docs/native-plugins.md)。`feathrise_plugin_api` 是本仓的独立发布包，默认只提供协议，不依赖编辑器或 Bevy；开发插件不需要编辑器源码。

先运行 [最小工程示例](docs/quickstart.md)：`cargo run --locked --example minimal_project`。它通过公共入口启动自定义 Plugin，显示旋转方块与默认中文文字，不需要编辑器或外部工程素材。

本地开发可以通过路径依赖使用 SDK：

```toml
[dependencies]
feathrise_runtime = { version = "0.3.0", path = "../feathrise_runtime" }
```

外部工程按 SDK 版本配置配套补丁。本仓内部子库同时声明 `version` 与 `path`。本地使用源码，发布归档中的依赖转为 registry 版本。

渲染补丁必须在消费工程的根 `Cargo.toml` 或显式 Cargo 配置中提供，库包自己的 `[patch]` 不会向上传递。0.20.1 源码的补丁包括 `avian3d`、`bevy_anti_alias`、`bevy_render`、`bevy_core_pipeline`、`bevy_extract`、`wgpu-hal`、`dlss_wgpu` 和 `wgpu-ffx`。使用 crates.io SDK 时，从独立补丁仓的配套 Git 提交取得该发布版本的补丁。

Feathrise 新建工程的 `sdk-source.toml` 支持 `path`、`git` 和 `registry`。registry 来源的 `version` 是完整发布版本，`git`／`rev` 仅选择上述补丁；新工程使用精确 SDK 版本，不需要维护者的本地 SDK 目录。配置变化不升级已有工程。

编辑器的源码联调配置见 [SDK 本地开发](docs/crates-io.md#本地修改)。

根入口提供 `run(project, project_plugin)`、`ProjectContext`、`GameView`、`validate_saved_scene` 和可取消的 `validate_saved_scene_with_check`。`project` 模块对应工程入口库，`app` 模块提供运行装配。`game_plugins::SceneRuntimePlugin` 装配场景、环境与渲染，场景生命周期使用 `ScenePhase`。

- `project::ProjectViewOrigin`：本次已保存场景的出生参考点。
- `project::SceneRuntimeObserver`：附近物理与流送使用的观察锚点。
- `project::PrimarySceneView`：场景生命周期创建、供环境和画质系统使用的主相机标记。

这些契约不创建玩家或提供移动、跳跃和跟随相机。当前空工程加载已保存场景、呈现场景视图；第三人称和固定俯视的 `GameView` 都是 3D 元数据。具体控制方式由工程插件实现。编辑器与工程运行在各自的 World 中。

公共设置库通过 `RenderSettings`、`SceneDisplaySettings` 和 `EnvironmentPreviewSettings` 接收运行输入；工程管理自己的配置和持久化。FPS/F3 面板和 LOD 状态可共用，工程可通过报告接入自有统计。

## 场景工程格式

主资产根下的目录是 `scenes/catalog.scene_catalog.json`。目录以 `scenes` 列出场景，各清单位于 `scenes/<SceneId>/scene.scene_manifest.json`。目录、清单和场景私有文件用格式 2，地表用格式 3。清单的 `subscenes` 通过 `source_scene_id` 引用同目录内的场景。

工程根的 `startup.scene.json` 用格式 2 保存工程身份、修订和可选 `startup_scene_id`。未指定启动项时采用首个已发布场景，空目录可以启动空世界。SDK 0.3 读取这一代场景格式。

## 功能特性

| 特性 | 作用 |
| --- | --- |
| `raytracing`（默认） | 编入光追能力，实际启用取决于工程设置和设备支持 |
| `default_font`（默认） | 内嵌 Noto Sans CJK 默认字体与 OFL 文本 |
| `dlss`、`dlss_frame_generation`、`rtx_rendering` | DLSS、Vulkan 与光追集成组合，[构建前提](crates/feathrise_frame_generation/README.md)分别覆盖 NGX 与 Streamline |
| `fsr`、`fsr3_upscaling`、`fsr3_frame_generation` | FSR 超分和帧生成组合；帧生成需显式提供原生 ZIP，或用包内脚本显式源码准备 |
| `test_panel` | 供工程启用的调试特性 |
| `physics_debug`、`initial_items` | 兼容旧特性名 |
| `validation` | 启用工程验收及报告 |

需要验收报告的工程构建启用 `feathrise_runtime/validation`。工程启动和公共运行装配分别位于根包的 `project`、`app` 模块，沿用同一功能开关。

启用 `default_font` 时，运行库通过 crates.io 依赖 `feathrise_font` 总包。总包在编译期拼回原件并校验 SHA-256。OFL 原文随字体包及运行库提供。`DefaultFontPlugin` 在 Bevy `DefaultPlugins` 后安装，只替换 Bevy 自带默认字体，保留工程已提供的默认字体与控件显式字体。字体编入程序，运行时不读取源码目录。需要自定字体的工程可设置 `default-features = false`，再按需开启 `raytracing` 等特性。

## 构建与运行前提

在 SDK 根目录检查默认组合：

```powershell
cargo check --locked -p feathrise_runtime
```

Windows 构建使用 Rust 1.97.1 或更新版本及 MSVC。公开 CI 默认使用固定的配套补丁提交，可通过仓库变量 `FEATHRISE_PATCH_REV` 覆盖。默认和验收特性的编译工作流见 [CI 说明](docs/ci.md)。测试素材使用库内中性输入。

FSR 帧生成使用 `FEATHRISE_FSR3_PACKAGE` 指定已有本地 ZIP 时，只校验、解包并链接修订 DLL 与桥接静态库，无需 AMD/Vulkan SDK、CMake 或为 FSR 编译 C++。没有原生包时，可按 [FSR 准备说明](crates/feathrise_fsr3_frame_generation/prebuilt/README.md)用公开包内脚本显式源码构建；这条路径需要对应 SDK 与原生构建工具。运行机器需要适用的 Vulkan 显卡驱动和 Microsoft Visual C++ 2015–2022 x64 运行库。当前 FSR 仍为候选能力，编译检查不代表完整 GPU 和画面验收。可选 DLSS 的 DLSS SDK、LLVM／Clang、Vulkan、MSVC 以及帧生成所需的 Streamline 前提见 [DLSS 构建说明](crates/feathrise_frame_generation/README.md)。

## 单包与仓库集成测试

发布包自己的测试只读取包内文件。完整仓库中的跨包生产 Shader 契约集中在不发布的 `tests/workspace` 测试包，不为测试增加生产公开接口。从仓库根执行集成测试：

```powershell
cargo test --locked -p feathrise_sdk_workspace_tests --lib
```

该命令默认执行无 GPU 的 Shader 集成契约，真实 GPU 专项仍显式忽略；范围与执行前提见 [仓库集成测试](tests/workspace/README.md)。SDK 作者视图窗口测试默认把截图写到本 SDK 的 `target/validation/author-view-window-msaa/`，`FEATHRISE_AUTHOR_VIEW_GPU_DIR` 可显式覆盖。

## 目录

```text
feathrise_runtime/
  Cargo.toml、Cargo.lock       工作区与统一入口
  src/                        公共 API、工程启动与运行装配
    project/                  工程上下文、启动、预览和编辑器通信
    app/                      公共场景装配、字体和呈现保护
  examples/minimal_project.rs  可直接运行的 SDK 最小工程
  crates/                     22 个独立子库
    feathrise_scene_core/      constants、view、state、input、residency 场景基础模块
    feathrise_fsr3_frame_generation/
      build.rs                Cargo 构建入口
      scripts/                显式源码重建辅助脚本
      prebuilt/               原生包锁与消费说明
  docs/                       最小使用示例与本仓 CI 说明
  THIRD_PARTY_NOTICES.md       随源码使用的第三方许可入口
```

## 升级兼容

升级 SDK 后，仍使用已移除 `PlayerInitData`、`controls`、`player` 等旧接口的工程需要显式迁移自己的源码与依赖；公共入口不再读取旧 `player_start.player_start.json` 来创建角色。固定原生包的 C ABI、文件名和身份协议按对应包说明保持兼容。
