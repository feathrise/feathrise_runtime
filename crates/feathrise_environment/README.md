# feathrise_environment

`feathrise_environment` 是工程环境能力的聚合入口。内容无关的种植分布、空间竞争、
优先级、足迹净空和近场代理由 `feathrise_vegetation` 统一提供；GPU 叶片、
通用多部件 Mesh、共享 Mesh 顶点风、八层地形地表材质，以及树木和固定景观领域入口按 feature 组合。

公共层没有草、麦、树、灌木或岩石类别枚举。工程通过以下正交能力描述内容：

- 候选源：规则密网格、抖动网格或显式稳定落点。
- 竞争：完全叠加、加权容量分区或数值优先覆盖。
- 足迹：实际轮廓、额外排斥边距和软恢复宽度。
- 实现：纯 GPU、实体或带滞回的近场混合代理。
- 后端：GPU 叶片或通用多部件 PBR Mesh。

## Features

| Feature | 功能 |
| --- | --- |
| `blade` | 正式 GPU 叶片后端与 `GpuBlade*` 公共 API |
| `mesh` | 通用多部件 PBR、阴影、距离LOD、固定公告板与逐视图GPU动态公告板后端 |
| `mesh-wind` | 树木与灌木共用的具体 PBR 顶点风材质、噪声资源和保守 AABB 接口 |
| `tree` | 树木领域入口；组件、旧根生成与 RenderApp 统计已内收，并依赖 `mesh-wind` |
| `landscape` | 固定景观领域入口；实现已内收并按需启用Bevy PBR与Grounding |
| `diagnostics` | 叶片异步 GPU 诊断 |
| `terrain-surface` | 最多八层、两张RGBA权重图与二维PBR数组的通用地形地表材质及RenderApp预热证据 |
| `full` | 启用以上全部能力 |

默认 feature 为空，但始终包含轻量的通用布局核心和近场消息接口。

```rust,no_run
use bevy::prelude::*;
use feathrise_environment::{EnvironmentPlugin, vegetation};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EnvironmentPlugin)
        .run();
}
```

正式场景、F3 与 F1 已改从 `vegetation::blade::GpuBlade*` 读取叶片 API；P5.3b真实地图
画面复验通过后，旧 `environment::grass / GpuGrass*` 兼容入口已经退役。树木与景观的
专项验收也已完成。P5.5已把旧景观和树木package的职责分别完整内收至
`environment::{landscape, tree}`并删除两个旧package；树木与灌木共用的具体风材质也已上提至
`environment::mesh_wind`。底层材质/噪声类型、Shader、AABB公式、公开函数行为、Plugin顺序、
旧根恢复和树木RenderApp统计均保持不变。默认构建与Devtools仅叶片构建不会引入树木、景观、
Grounding或Feronia；`mesh-wind`可独立编译，`tree`显式依赖它。新增内容继续使用`vegetation`的
层、竞争、足迹和通用后端API。地形地表则统一使用`environment::terrain_surface`；它不读取、
统计或拥有草、麦、树等植被密度，地图适配层只负责提供权重纹理、层参数和失败时的白模策略。
