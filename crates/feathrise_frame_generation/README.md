# feathrise_frame_generation

Windows x64 Vulkan 的 Streamline／DLSS 帧生成接入，提供原生桥、呈现事务、运行库身份检查及运行状态。SDK 默认构建不启用本包；根包的 `dlss` 或 `dlss_frame_generation` 会启用它，并同时启用 SR／RR 所需的 DLSS 依赖。直接构建本包也会执行以下原生前置检查。

本包自有源码采用 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE)。工程依赖与八项公开 Git 补丁见 [SDK 接入说明](../../docs/crates-io.md)。

## Windows 构建前提

DLSS SR／RR 与 Streamline 帧生成使用两个独立 SDK，不能只配置 `STREAMLINE_SDK`。

| 输入 | 实际要求 |
| --- | --- |
| Rust／MSVC | `x86_64-pc-windows-msvc` 工具链、Visual C++ x64 构建工具和 Windows SDK；原生桥使用 C++17，在配置好 MSVC 环境的 x64 开发者终端中运行 |
| `DLSS_SDK` | [NVIDIA DLSS SDK v310.9.1](https://github.com/NVIDIA/DLSS/tree/v310.9.1) 根目录；需要 `include/`、`lib/Windows_x86_64/x64/` 中的 NGX 静态库，以及 `lib/Windows_x86_64/rel/` 中的 `nvngx_dlss.dll` 和 `nvngx_dlssd.dll` |
| `VULKAN_SDK` | [Vulkan SDK](https://vulkan.lunarg.com/sdk/home) 根目录，必须包含 `Include/vulkan/vulkan.h`；NGX 绑定与 Streamline 桥均读取其头文件 |
| LLVM／Clang | `bindgen` 使用 `libclang` 解析头文件，`LIBCLANG_PATH` 指向包含 `libclang.dll` 的目录；让匹配版本的 `clang.exe` 可通过 `PATH` 找到，或用 `CLANG_PATH` 指定其完整路径，以发现 C／C++ 头文件搜索目录。参见 [bindgen 要求](https://rust-lang.github.io/rust-bindgen/requirements.html)与 [clang-sys 查找规则](https://github.com/KyleMayes/clang-sys/blob/v1.8.1/src/support.rs) |
| `STREAMLINE_SDK` | [Streamline 2.12.0](https://github.com/NVIDIA-RTX/Streamline/releases/tag/v2.12.0) 根目录；目录名必须为 `Streamline-v2.12.0`，包含公共 `include/` 与 `bin/x64/` 生产 DLL，文件还须通过固定 SHA-256 校验 |

`dlss_wgpu` 根据 CRT 选项链接 `nvsdk_ngx_d.lib` 或 `nvsdk_ngx_s.lib`，生成 NGX 绑定与 C 包装；本包再通过 MSVC 编译 Streamline C++ 桥。LLVM／Clang 不替代这套 MSVC 工具。正常构建不选择 DLSS 的开发覆盖层 DLL。

以下路径仅作示例，须替换为本机实际 SDK 根目录；在 SDK 仓库根执行检查：

```powershell
$env:DLSS_SDK = 'C:\SDK\DLSS-v310.9.1'
$env:VULKAN_SDK = 'C:\SDK\VulkanSDK'
$env:STREAMLINE_SDK = 'C:\SDK\Streamline-v2.12.0'
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
$env:CLANG_PATH = 'C:\Program Files\LLVM\bin\clang.exe'
cargo check --locked -p feathrise_frame_generation
```

工程侧启用根 SDK 的 `dlss_frame_generation` 后，可在配置好相同前置的工程根执行 `cargo check`。只启用 `rtx_rendering` 的 SR／RR 路径需要 DLSS SDK、Vulkan 与 LLVM／Clang，不需要 Streamline 帧生成桥。

## 应用身份与运行文件

`STREAMLINE_APPLICATION_ID` 是可选覆盖；未设置时使用库内固定 Project ID 路径。设置时必须是 NVIDIA 分配的非零 `u32`，空值、零、越界或非数字均会拒绝，不需要为构建填入一个示例 ID。

构建将 SR／RR 的两份 NGX DLL 放入独立目录；Streamline 则只准备 `sl.interposer.dll`、`sl.common.dll`、`sl.dlss_g.dll`、`sl.reflex.dll`、`sl.pcl.dll`、`NvLowLatencyVk.dll` 和 `nvngx_dlssg.dll` 七份固定生产文件。运行前还会核对身份、签名和设备能力；完整 SDK 不进入本 crate 归档，构建不会自动下载厂商 SDK。

发布程序时按实际使用的运行文件保留 NVIDIA 原条件与所需声明。编译成功只证明工具链和源码可构建，`Active2x` 还需要实际 Vulkan 设备、驱动、完整输入和呈现结果验证。
