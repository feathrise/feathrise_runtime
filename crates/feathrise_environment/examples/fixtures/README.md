# 网格风中性夹具

`wind_tree.glb` 是程序生成的中性夹具，包含八面柱、两张叶冠卡片、两份 PBR 材质及内嵌小 PNG。示例直接读取这份夹具。

该夹具验证 GLB 双部件异步加载、透明裁切、风材质、LOD 淡出和多实例渲染。

`MESH_WIND_SMOKE_REAL_TREE=1` 保留旧示例启动开关，现选择此中性 GLB；未设置时仍使用 Rust 程序网格。

本目录程序生成的中性夹具与配套源码采用 [MIT](../../LICENSE-MIT) 或 [Apache-2.0](../../LICENSE-APACHE)，由使用者任选其一。
