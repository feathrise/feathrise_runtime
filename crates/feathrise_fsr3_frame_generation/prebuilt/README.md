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

修订DLL不带AMD Authenticode签名，不能标成AMD官方签名DLL。官方SDK来源锚点、902项输入、MIT版权以及修订内容继续见[独立补丁仓](https://github.com/feathrise/feathrise_patches)中的 `fidelityfx-sdk-patches/README.md`。该来源说明属于六文件身份输入，不能为了更新本分发说明随意修改。

固定来源原文中的 `scripts/构建FSR修订运行库.ps1` 是当时的历史路径；当前入口为所属 crate 的 [scripts/构建FSR修订运行库.ps1](../scripts/构建FSR修订运行库.ps1)。保留原文字节以维持既有包身份，维护命令以本说明为准。

## 显式源码准备与构建

当前锁只固定原生包身份，不配置下载 URL；构建不自动下载。已有匹配 ZIP 时使用前面的 `FEATHRISE_FSR3_PACKAGE`。没有该 ZIP 时，可以从官方 SDK 与公开补丁源码重建，不需要取得私有主仓，也不需要完整 Feathrise SDK 源码仓。

先从 [AMD FidelityFX SDK v1.1.4 官方发布页](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases/tag/v1.1.4)取得 **Minimal package** 并解压。它须包含 `PrebuiltSignedDLL/amd_fidelityfx_vk.dll`、`sdk/` 与 `ffx-api/`；GitHub 自动生成的源码压缩包不能替代带签名原件的官方包。脚本会校验官方 DLL 的 SHA-256／AMD 签名，以及固定清单中的全部 902 项源码和工具。

安装 PowerShell 7、Git、CMake >=3.23、Visual Studio 2022 C++ x64 工具链，以及 Vulkan SDK >=1.3.250。这些工具仅用于显式源码重建，普通 ZIP 消费不需要它们。随后准备公开补丁和构建输入：

```powershell
git clone https://github.com/feathrise/feathrise_patches.git
git -C feathrise_patches checkout --detach 3c251d3530980b66e49b4748769db36ae9c6bd5a
$patchRepository = (Resolve-Path './feathrise_patches').Path
$env:FIDELITYFX_SDK = (Resolve-Path 'C:/SDK/FidelityFX-SDK-v1.1.4').Path
$env:VULKAN_SDK = (Resolve-Path 'C:/VulkanSDK/1.4.357.0').Path
$env:FEATHRISE_FSR3_PATCH_ROOT = Join-Path $patchRepository 'fidelityfx-sdk-patches'
$env:FEATHRISE_FSR3_BUILD_FROM_SOURCE = '1'
```

示例中的两个官方 SDK 安装路径应改为自己的实际绝对路径。补丁目录必须来自上述配套提交，保留其中的许可、补丁、`source-manifest.json` 与来源说明原件；不从 SDK 包或私有父目录寻找副本。CMake 来自 `BL5_FSR3_CMAKE` 或 PATH，PowerShell 可用 `BL5_FSR3_PWSH` 指定；不搜索维护者的私有工具缓存。

对已解包的本 crate 直接构建。首次检查允许 Cargo 按当前依赖和补丁来源更新锁文件，成功后再添加 `--locked` 复验：

```powershell
cargo --config (Join-Path $patchRepository '.cargo/config.toml') check --manifest-path 'C:/src/feathrise_fsr3_frame_generation-0.1.0/Cargo.toml'
```

包内构建脚本按 `CARGO_MANIFEST_DIR/scripts/构建FSR修订运行库.ps1` 定位。源码副本、CMake 缓存和修订运行库默认写入本次 Cargo 私有 `OUT_DIR/fsr3-source/`，不改写 registry 包的源码输入。直接检查解包包时，Cargo 默认的 `target/.../build/<包哈希>/out/fsr3-source/`，以及包内自定义 `--target-dir` 对应的输出都允许使用；脚本核对继承的绝对 `OUT_DIR` 与当前包 `CARGO_MANIFEST_DIR` 身份，不依赖目标目录名。需要复用已有缓存时，可显式将 `FEATHRISE_FSR3_SOURCE_BUILD_ROOT` 设为独立绝对目录，例如现有 `.local/fsr3-sdk-patched`；显式覆盖的目录应放在包源码外，并与官方 SDK 和补丁材料目录分离；包内只允许上述 Cargo 专用生成目录。已有完整 DLL／manifest 仍须通过同一来源校验；半包或未知内容保持拒绝覆盖。

在完整 SDK 检出中也可运行 `cargo check --locked -p feathrise_runtime --features fsr`，使用相同环境变量。各变量仅为本次显式源码命令设置，用完恢复；源码失败不会静默改用预编译包。

### Windows 源码路径长度

默认 `OUT_DIR/fsr3-source` 继续可用；但首次构建会生成名称较长的 shader 头文件。当前 MSVC 可能对已经生成、但完整路径过长的文件报 `C1083`。脚本在复制 SDK、探测 CMake 和编译前，从官方 accessor 的 `_permutations.h` 引用推导 32 位摘要头文件名；最长预计路径达到 260 个字符时，直接报告具体长度和路径。

遇到该诊断时，显式选择较短的绝对缓存目录后重试，例如：

```powershell
$env:FEATHRISE_FSR3_SOURCE_BUILD_ROOT = 'C:/cache/feathrise-fsr3'
```

手动脚本对应使用 `-BuildRoot 'C:/cache/feathrise-fsr3'`。目录仍须满足上面的源码与输入分离规则；构建不会自动改用其他缓存、修改系统长路径配置或映射驱动器。已有包、锁文件和 ABI 不因选择较短目录而改变。

## 维护者手动重建

也可直接调用随包脚本，并显式指定可写缓存：

```powershell
pwsh -NoProfile -File 'C:/src/feathrise_fsr3_frame_generation-0.1.0/scripts/构建FSR修订运行库.ps1' -SdkRoot $env:FIDELITYFX_SDK -PatchRoot $env:FEATHRISE_FSR3_PATCH_ROOT -VulkanSdk $env:VULKAN_SDK -BuildRoot 'C:/cache/feathrise-fsr3-source'
```

结果位于 `BuildRoot/runtime/`；该目录中的 DLL／manifest／许可与显式补丁目录共同组成来源链。此命令重建运行库，不宣称产生与可信锁逐字节相同的八文件预编译 ZIP。不同编译器重新链接可能改变 DLL 身份；不得修改锁来掩盖不匹配，制作新发行原生包仍需重新审核、生成桥接库及固定新身份。

主仓维护者组包同样传入 `-PatchRoot <材料绝对目录>`，并在 `-RebuildRuntime` 时向同一脚本显式提供 `-BuildRoot`。普通原生包消费、运行交付、AMD 原许可和当前 ZIP／DLL 锁定身份均保持。
