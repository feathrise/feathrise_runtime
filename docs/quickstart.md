# 运行第一个 SDK 示例

先将 SDK 与补丁仓克隆到同一目录，并将补丁固定到当前 SDK 配套的提交：

```powershell
git clone https://github.com/hwyrq/feathrise_runtime.git
git clone https://github.com/hwyrq/feathrise_patches.git
git -C feathrise_patches checkout 3c251d3530980b66e49b4748769db36ae9c6bd5a
cd feathrise_runtime
```

SDK 自己的 `.cargo/config.toml` 会加载同级补丁配置，不需要维护者的上级仓库。在 SDK 目录执行：

```powershell
cargo run --locked --example minimal_project
```

窗口显示地面、旋转方块和中文提示。按空格暂停／继续旋转，按 Esc 或关闭窗口退出。示例只依赖本 SDK 和 Bevy，使用程序生成的几何。首次构建需要现有 Rust/MSVC 工具链及 Cargo 依赖；图形运行需要适用驱动。

入口位于 [examples/minimal_project.rs](../examples/minimal_project.rs)。它做两件事：明确提供工程身份与资产根的 `ProjectContext`，然后调用 `feathrise_runtime::run(project, MinimalProjectPlugin { ... })`。`run` 安装 Bevy、场景、资源与渲染运行时；工程的 `Plugin` 注册自己的启动、旋转和输入系统。此例的空地图目录不会创建主相机，所以示例自行创建一个 `PrimarySceneView`；加载已保存地图的工程应初始化地图已有的主相机，避免叠加第二台窗口相机。

`examples/minimal_project/assets/` 只有 7 份小 JSON：空地图、建筑、院落、植被、地表、水体目录和中性世界环境。它们满足当前公共场景启动时的读取合同，没有模型或纹理；空地表目录里的派生路径不会触发纹理数组加载。示例只读这些文件。它们是示例数据，不会被复制进编辑器新建工程。

## 默认字体与特性

默认特性是 `raytracing` 与 `default_font`。前者编入光追能力，实际启用仍受设置与设备支持约束；后者提供内嵌的中文默认字体。示例没有设置 `TextFont.font`，中文来自 `run` 安装的 `DefaultFontPlugin`。工程自带的默认字体或显式字体句柄仍优先。

关闭默认光追能力、保留中文字体：

```powershell
cargo run --locked --example minimal_project --no-default-features --features default_font
```

完全关闭 SDK 默认特性：

```powershell
cargo run --locked --example minimal_project --no-default-features
```

第二种使用 Bevy 自带字体，示例相应显示英文。`--no-default-features` 只关闭本 SDK 默认功能，不关闭整个 Bevy。FSR 与 DLSS 组合及其构建前提见 [README 功能特性](../README.md#功能特性)。

需要一次可重复的实际窗口截图时，可显式传入已有目录中的新 PNG 文件名：

```powershell
cargo run --locked --example minimal_project -- --capture "$env:TEMP/feathrise-minimal.png"
```

截图模式预热后请求真实窗口 GPU 回读，再保存并正常退出；45 秒内未收到回读会失败。普通运行保持交互。输出路径在切换工程工作目录前解析，已有文件不会被覆盖。

## 在自己的工程使用

推荐由 Feathrise 新建工程：它会生成根依赖、补丁、空资产目录及工程描述，工程只编译 SDK 和自身代码。编辑器通过 `sdk-source.toml` 选择 registry 版本、本地路径或固定 Git 提交。

手动接入已发布版本时，在**工程根** `Cargo.toml` 配置精确 SDK 版本与公开 Git 补丁：

```toml
[dependencies]
bevy = "=0.19.1"
feathrise_runtime = { version = "=0.1.0", default-features = false, features = ["default_font"] }

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

这只是已有工程清单中的依赖部分；工程仍须提供下面的入口和公共资产目录。Cargo 不会继承依赖包自己的 `[patch]`。首次 `cargo check` 生成工程自己的锁文件后，再用 `cargo check --locked` 复验。需要联调源码时见[本地修改](crates-io.md#本地修改)。

编辑器创建的工程入口使用实际 `ProjectContext::load`，例如：

```rust
use bevy::prelude::*;
use feathrise_runtime::ProjectContext;

struct MyGame;
impl Plugin for MyGame {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, || info!("我的工程已启动"));
        // 在这里注册自己的玩法、相机控制和界面系统。
    }
}

fn main() -> Result<(), String> {
    let project = ProjectContext::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
        env!("CARGO_PKG_NAME"),
    )?;
    feathrise_runtime::run(project, MyGame)
}
```

`load` 从工程的 Cargo 清单目录向上最多查找 16 层 `editor-project.json`，验证 `format=2`、`game_entry={"kind":"saved-world","version":1}`、匹配的 `package` 和 `cargo_manifest`，以及 1～16 个真实资产目录与独立 `editor_data` 路径。路径不能跳出工程或经过符号链接；`name`、`id` 也必须有效。`run` 随后读取主资产根的 `maps/catalog.map_catalog.json`：空目录允许空世界；已保存地图按工程自己的启动选择读取。

仓内 Cargo example 没有独立的工程清单，故显式构造 `ProjectContext` 并固定到自己的只读示例目录；它没有演示编辑器工程描述的加载。实际工程入口中的 `env!` 必须在工程源码展开，不能改成 SDK 编译目录。未使用编辑器描述格式的工程也可显式构造上下文，但必须自行提供有效、规范化的工程根、非空资产根及完整公共目录文件。
