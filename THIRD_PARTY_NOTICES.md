# feathrise_runtime 第三方与资源说明

SDK 自有源码采用 `MIT OR Apache-2.0`，完整原文为根目录及各发布子库的 `LICENSE-MIT`／`LICENSE-APACHE`。第三方组件保留原许可；此源码许可不自动改变品牌及全部资产的使用条件。

随 SDK 提供的自有中性示例数据和程序生成的测试夹具采用相同的 `MIT OR Apache-2.0`，包括网格风示例的 `wind_tree.glb`；第三方资源仍按下面的原许可处理。

- 八项第三方 Rust 补丁与兼容层由独立 `feathrise_patches` Git 仓提供，各自保留原 `LICENSE`、`NOTICE`、`PATCHES.md` 或 `UPSTREAM.md`；本地修改不冒充上游原样版本。植被 crate 的已有原许可仍位于该 crate 内。
- 默认中文字体由独立发布的 `feathrise_font` 总包提供，总包随附原 OFL，SDK 通过 `DEFAULT_FONT_LICENSE` 提供相同文本：未修改的 Noto Sans CJK SC，来源为 `notofonts/noto-cjk` 的固定提交 `f8d157532fbfaeda587e826d4cd5b21a49186f7c`，OFL-1.1 原文保留 Adobe 版权及 Reserved Font Name。字体嵌入程序后，发行者仍须附带适用许可。
- FidelityFX 1.1.4 修订源码材料位于独立补丁仓 `feathrise_patches/fidelityfx-sdk-patches/`，独立提供的 Windows 预编译包由 `crates/feathrise_fsr3_frame_generation/prebuilt/package.lock.json` 锁定。原 AMD MIT 许可、项目修改及运行文件身份随原材料保留；修订 DLL 没有 AMD Authenticode 签名。
- `dlss_wgpu` 的开源许可仅覆盖对应源码；NVIDIA DLSS／Streamline 二进制以及 Microsoft 运行库的分发条件须按实际版本另行处理。
- 普通 Cargo 依赖由 Cargo 按清单获取，不在本仓复制其全部源码。发行程序时应根据实际提供的组件保留对应原文、NOTICE 及其他适用材料。

全特性依赖审计、材料采集记录和组包副本由开发仓库维护，不属于 SDK 编译输入。SDK 的源码、测试和示例不依赖这些维护资料。
