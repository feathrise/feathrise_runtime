# 运行 SDK 示例

检出 SDK 后进入源码目录。Cargo 按清单和锁获取固定版本的 Bevy 补丁。

```powershell
git clone https://github.com/feathrise/feathrise_runtime.git
cd feathrise_runtime
```

进入下面任一示例工程目录，执行 `cargo run --locked`。

| 工程目录 | 启动入口 | 装配方式 |
| --- | --- | --- |
| `examples/minimal_project/` | [`src/main.rs`](../examples/minimal_project/src/main.rs) | 调用 `feathrise_runtime::run`，由 SDK 装配 Bevy App，工程提供玩法插件。 |
| `examples/minimal_project_plugin/` | [`src/main.rs`](../examples/minimal_project_plugin/src/main.rs) | 自建 `App`，调用 `prepare_project`，配置窗口后添加 `ProjectRuntimePlugin`。 |

两个示例都显示地面、旋转方块和中文提示。按空格暂停或继续旋转，按 Esc 或关闭窗口退出。它们只依赖本 SDK 和 Bevy，使用程序生成的几何。首次构建需要 Rust 1.97.1 或更新版本、MSVC 工具链及 Cargo 依赖。图形运行需要适用驱动。

共用的演示插件在空场景里创建一个 `PrimarySceneView`。加载已保存场景的工程应初始化场景已有的主相机，避免叠加第二台窗口相机。[共用场景](../examples/support/mod.rs)

每个示例都有自己的 `Cargo.toml`、`editor-project.json`、`src/` 和 `assets/`。各自的 `assets/` 包含空场景目录及建筑、院落、植被、地表、水体和世界环境的中性配置。两个示例共用 SDK 工作区的 `Cargo.lock` 和 `target/`。

## 默认字体与特性

默认特性是 `raytracing` 与 `default_font`。前者编入光追能力，实际启用仍受设置与设备支持约束。后者提供运行时安装的 `DefaultFontPlugin` 与内嵌中文字体。工程自带的默认字体或显式字体句柄仍优先。

关闭默认光追能力、保留中文字体：

```powershell
cargo run --locked --no-default-features --features default_font
```

完全关闭 SDK 默认特性：

```powershell
cargo run --locked --no-default-features
```

第二种使用 Bevy 自带字体，示例相应显示英文。`--no-default-features` 只关闭本 SDK 默认功能，不关闭整个 Bevy。FSR 与 DLSS 组合及其构建前提见 [README 功能特性](../README.md#功能特性)。

两个示例都接受 `--capture`，输出到已有目录中的新 PNG 文件。

```powershell
cargo run --locked -- --capture "$env:TEMP/feathrise-minimal.png"
```

截图模式预热后请求真实窗口 GPU 回读，再保存并正常退出。45 秒内未收到回读会失败。输出路径以启动目录为基准，并要求新的文件名。

## 在自己的工程使用

用 Feathrise 创建场景工程时，将 `sdk-source.toml` 指向配套 SDK 与补丁源码。编辑器会生成根依赖、补丁、空资产目录及工程描述，工程只编译 SDK 和自身代码。

联调当前 Bevy 0.20.1 源码时，在工程根的 `Cargo.toml` 使用路径依赖，并按[本地修改](crates-io.md#本地修改)载入同级补丁配置。

```toml
[dependencies]
bevy = "=0.20.1"
feathrise_runtime = { version = "0.2.0", path = "../feathrise_runtime", default-features = false, features = ["default_font"] }
```

Cargo 补丁配置写入消费工程根清单。首次生成锁文件后，再用 `cargo check --locked` 复验。

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
    let directory = std::env::current_dir().map_err(|error| error.to_string())?;
    let project = ProjectContext::load(&directory, env!("CARGO_PKG_NAME"))?;
    feathrise_runtime::run(project, MyGame)
}
```

`load` 从工程的 Cargo 清单目录向上最多查找 16 层 `editor-project.json`，验证 `format=2`、`game_entry={"kind":"saved-world","version":1}`、匹配的 `package` 和 `cargo_manifest`，以及 1～16 个真实资产目录与独立 `editor_data` 路径。路径不能跳出工程或经过符号链接；`name`、`id` 也必须有效。`run` 随后读取主资产根的 `scenes/catalog.scene_catalog.json`：空目录允许空世界；工程根的 startup.scene.json 保存可选启动场景。未设置时采用首个已发布场景。

两个仓内示例都通过 `ProjectContext::load` 从启动目录读取各自的工程清单。`CARGO_PKG_NAME` 在工程源码中展开，用于校验清单的 `package`。显式构造上下文的工程需要自行提供有效、规范化的工程根、非空资产根及公共目录文件。

### 工程自行装配 Bevy App

独立运行的工程可以在 `main` 中用下面的代码替换 `run(project, MyGame)`。`prepare_project` 校验启动场景和资产根，在 Bevy 初始化 `AssetPlugin` 前把资产源注册到这个 `App`。宿主可以修改返回的 `AssetPlugin`，也可以配置 `WindowPlugin`、`RenderPlugin` 和任务池。

```rust
use feathrise_runtime::{ProjectRuntimePlugin, prepare_project};

let mut app = App::new();
let assets = prepare_project(&mut app, &project)?;
app.add_plugins(DefaultPlugins.set(assets).set(WindowPlugin {
    primary_window: Some(Window {
        title: "我的工程".into(),
        ..default()
    }),
    ..default()
}))
.add_plugins((ProjectRuntimePlugin, MyGame));
match app.run() {
    AppExit::Success => Ok(()),
    exit => Err(format!("工程退出失败：{exit:?}")),
}
```

运行时插件复用宿主已安装的 Avian 物理和 Hanabi 粒子插件，需要时安装 SDK 的默认组合。自定义物理组合用于场景热重载时，应保留对 `ColliderCachePlugin` 的禁用，避免用旧碰撞形状替代重新加载的模型。此入口的主窗口呈现模式由宿主管理。编辑器管理的预览进程继续使用 `run(project, plugin)`，由它处理启动参数和窗口交接。
