# Windows FSR 帧生成预编译包

启用 `fsr3_frame_generation` 或 `fsr` 时，先准备匹配 [package.lock.json](package.lock.json) 的原生 ZIP，再显式传入绝对路径：

```powershell
$env:FEATHRISE_FSR3_PACKAGE = (Resolve-Path 'C:/path/feathrise-fsr3-fg-windows-x86_64-msvc.zip').Path
cargo check --locked -p feathrise_runtime --features fsr
```

目标为 `x86_64-pc-windows-msvc`，桥接使用动态 CRT 和 ABI 1。构建只读取本地 ZIP，校验长度、SHA-256 和接口源码指纹后链接。解包缓存默认位于 `%LOCALAPPDATA%/Feathrise/native`，可通过 `FEATHRISE_NATIVE_CACHE` 指定。

## 包与身份协议

`package.lock.json`为受版本控制的信任入口，固定`schema_version`、`package_id`、目标、`crt`、归档文件名、字节数和SHA-256、`native_manifest_sha256`。当前包身份为`fsr3-1.1.4-bl5-vulkan-lifetime-ui-1-abi1`。升级时同时发布新原生包并更新 SDK 锁文件。

ZIP只允许以下八个根目录文件，不接受路径穿越、子目录、重复名称或额外内容：

| 文件 | 职责 |
| --- | --- |
| `amd_fidelityfx_vk.dll` | 从已校验官方SDK源码应用独立补丁仓修订后构建的运行库 |
| `manifest.json` | 原SDK、补丁、许可及修订产物身份 |
| `LICENSE-FidelityFX-SDK.txt` | AMD MIT许可完整文本 |
| `v1.1.4-vulkan-lifetime.patch` | 独立补丁仓维护的源码修订 |
| `source-manifest.json` | 固定官方源码/API/工具输入清单 |
| `README.md` | 原来源、修订与运行交付说明 |
| `rpg_fsr3_frame_generation_bridge.lib` | 编入Rust程序的预编译桥接静态库 |
| `native-package.json` | 目标、CRT、桥接ABI、逐文件身份与源码输入指纹 |

`rpg_fsr3_frame_generation_bridge.lib`是已经固化的原生包文件名，不是当前Cargo子库名称。此次公共包统一命名保留该文件名、C ABI及修订身份，继续链接同一已验二进制，避免仅为更名重新构建AMD SDK。

`native-package.json`固定包身份、目标、动态CRT与`bridge_abi`，记录其余七文件的长度和SHA-256；清单不对自身递归求哈希。归档SHA-256先由本地锁文件验证，再读取包内清单，所以下载方不能通过同时替换DLL和JSON建立新信任。

源码接口指纹覆盖`src/native/fsr3_frame_generation_bridge.cpp`、`fsr3_present_images.h`、`fsr3_present_submit.h`、`fsr3_runtime_identity.h`、`fsr3_ui_resources.h`以及`src/native.rs`、`src/runtime_identity.rs`。四个头文件均位于`src/native/`。文本按UTF-8读取并将CRLF规范化为LF后计算SHA-256，避免Git换行设置改变接口身份；不得忽略实际代码变更。运行DLL身份继续和`runtime_package.rs`中的SDK、补丁、许可来源核验衔接。

构建获取器须核对归档长度/SHA、八文件集合、逐文件身份、目标、CRT、ABI与当前源码指纹，先解入私有暂存区，全部成功后才原子发布缓存。完整缓存复用前仍需核验，其中`native-package.json`先按本地锁中的`native_manifest_sha256`验证，不能通过修改缓存清单同时替换桥接库。不能采用半包、未知文件或上次未完成的解包目录。桥接库从共享缓存复制到本次Cargo私有`OUT_DIR`后，再按已经核验的桥接SHA-256复查，实际链接该私有副本。任何校验失败均返回具体原因，不静默回退源码编译，也不替换用户未知目录。

## 运行交付

桥接`.lib`只用于Rust程序链接。运行工程时仍只交付、固化并复验原六文件：DLL、`manifest.json`、AMD许可、补丁、源码清单与原来源`README.md`。既有发行白名单、启动前完整身份复验和锁定文件后加载策略保持；包内清单不能取代程序编译时固定的身份。

运行机器需要支持目标GPU与Vulkan的显卡驱动，以及Microsoft Visual C++ 2015–2022 x64运行库；已经安装合适运行库时无需重复安装。这些运行前提与AMD SDK、Vulkan SDK、CMake等维护者构建工具分别处理，预编译分发不会将其移除。

修订DLL不带AMD Authenticode签名，不能标成AMD官方签名DLL。官方SDK来源锚点、902项输入、MIT版权以及修订内容继续见[独立补丁仓](https://github.com/hwyrq/feathrise_patches)中的 `fidelityfx-sdk-patches/README.md`。该来源说明属于六文件身份输入，不能为了更新本分发说明随意修改。

固定来源原文中的 `scripts/构建FSR修订运行库.ps1` 是当时的历史路径；当前入口为所属 crate 的 [scripts/构建FSR修订运行库.ps1](../scripts/构建FSR修订运行库.ps1)。保留原文字节以维持既有包身份，维护命令以本说明为准。

## 维护者与CI重建

普通消费构建默认使用预编译包。只有维护者或CI显式设置`FEATHRISE_FSR3_BUILD_FROM_SOURCE=1`才进入源码模式，安装官方AMD SDK、Vulkan SDK、CMake及MSVC C++构建工具，并沿用[修订运行库构建脚本](../scripts/构建FSR修订运行库.ps1)的来源审计与本工作树原子发布规则。SDK 目录由 `-SdkRoot` 或 `FIDELITYFX_SDK` 明确提供。原生补丁材料目录由 `-PatchRoot` 或 `FEATHRISE_FSR3_PATCH_ROOT` 明确提供，指向独立补丁仓的 `fidelityfx-sdk-patches/` 绝对路径；不从 SDK 或私有父目录寻找副本。CMake 来自 `-CmakePath`、`BL5_FSR3_CMAKE` 或 PATH；不搜索维护者的私有工具缓存。环境变量只在所需命令范围设置，用完恢复，不能作为普通用户前提。

源码模式示例（在 SDK 根目录运行，原生补丁需来自与当前原生包配套的补丁仓检出）：

```powershell
$env:FEATHRISE_FSR3_PATCH_ROOT = (Resolve-Path '../feathrise_patches/fidelityfx-sdk-patches').Path
$env:FEATHRISE_FSR3_BUILD_FROM_SOURCE = '1'
cargo check --locked -p feathrise_runtime --features fsr
```

主仓维护者组包同样传入 `-PatchRoot <材料绝对目录>`，并在 `-RebuildRuntime` 时将其传给同一源码重建入口。原生包消费、运行交付及许可校验均保留现有哈希锁定；本次目录迁移不修改 ZIP、DLL 或来源原文。
