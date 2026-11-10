# feathrise_vegetation

`feathrise_vegetation` 是 Bevy 0.19 的内容无关种植与空间竞争库。它不区分草、麦、
树、灌木或岩石，而是把内容拆成候选分布、密度、竞争、足迹、实现策略和渲染后端。

核心特性：

- 无限制的层 ID 和竞争组 ID，与 GPU 叶片批次的四个外观槽完全分离。
- `Additive`、`WeightedPartition`、`PriorityOverride` 三种明确竞争语义。
- 实际密度与优先覆盖所有权正交；所有权可选择最近点或源采样单元相交，不扩大可见密度。
- 数值优先级，以及会报告完整循环链的关系式优先级辅助器。
- 圆、椭圆、矩形和 Mesh AABB 足迹，支持实例缩放或固定世界尺寸、额外排斥边距与软恢复宽度。
- 固定世界网格和稳定哈希，相机移动、窗口重定位或局部重算不会重新洗牌。
- 64×64 稀疏脏块、暂存布局、整批原子版本切换和与活动版本对齐的提交快照。
- 可选 GPU 叶片后端、通用多部件 PBR Mesh 后端和无物理依赖的近场代理消息。

默认 feature 为空。`blade` 保留既有 208 字节叶片窗口、16 字节紧凑实例、四档 LOD
和每批四个外观槽，三份 WGSL 直接随本包内嵌；层数超过四种外观时拆分多个渲染
批次即可。叶片专业 API 统一使用 `GpuBlade*` 命名；P2.4已经退役独立
`bevy_learn5_grass`包装包，P5.3b真实场景画面复验通过后又删除
`feathrise_environment::grass`、`grass-compat`及全部`GpuGrass*`公开别名。

`mesh` 提供通用多部件 PBR 与距离 LOD。固定公告板继续使用原材质和作者姿态；动态
`Cylindrical / Spherical` 模式需同时安装 `VegetationMeshBillboardPlugin`，并绑定由
`create_vegetation_mesh_billboard_material` 创建的共享扩展材质。朝向在主画面、阴影与
预通道中按各自视图于 GPU 顶点阶段计算，稳定帧不会遍历相机、扫描公告板节点或写回
ECS `Transform`。动态平面可通过`culling_aabb`提供覆盖全部朝向的保守边界；省略时会
自动关闭不可靠的静态薄平面视锥剔除。`feathrise_environment` 的 `mesh` feature 会
自动组合这两个 Mesh 插件。

给叶片窗口挂载 `VegetationBladeLayer` 后，后端会在活动布局完成原子换版时把对应
无限层解析为所有 LOD 共享的一张完整 Mip 密度图。RenderApp只在布局加载或换版后
接收未决的实际密度、所有权密度、组参数、稳定选择值与足迹影响，并在GPU上执行三种
竞争模式；连续版本且图集范围不变时，候选先复制上一已验证Mip0，再只重写
`VegetationLayoutCommit`记录的64×64基础块，随后把完整Mip链重新生成到1×1。
局部路径与整图路径使用相同的八位量化和整数box average，并在采用前异步回读全部
RGBA字节对照CPU参考。布局处理中、Shader或资源等待、字节差异、超时及安全尺寸
校验失败时，窗口继续使用上一有效版本；布局稳定时不会重建图片或重复执行层竞争。
`VegetationBladeAdoptionStatus`在Update延迟组件写入后的PostUpdate发布活动revision、
可见绑定层数及已采用/绑定窗口分子分母，并通过`VegetationBladeAdoptionPhase`区分
未绑定、安全托底、候选校验中、当前布局全部采用与拒绝托底；该资源只提供观察证据，
不会替工程改变迁移状态或画面验收结论。

同组多个目标层共享一次未决输入编码和同票据GPU缓冲。当前叶片后端固定单竞争组
最多32层、每个输入存储缓冲不超过128 MiB，并继续核对设备实际绑定上限；公共核心
的总层数没有因此受限。`VegetationBladeValidationLayers`可在不创建可见窗口的情况
下请求指定层走同一候选、Mip与逐字节采用门，便于工程切换前做后台对等。

crates.io 版本发布后可使用下面的配置；发布前源码接入及配套补丁见
[SDK 接入说明](../../docs/crates-io.md)。

```toml
[dependencies.feathrise_vegetation]
version = "=0.1.0"
features = ["blade", "mesh"]
```

竞争示例：

```rust
use feathrise_vegetation::{
    VegetationCompetitionGroup, VegetationCompetitionGroupId,
    VegetationCompetitionMode,
};

let ground_cover = VegetationCompetitionGroup {
    id: VegetationCompetitionGroupId(1),
    mode: VegetationCompetitionMode::WeightedPartition { capacity: 1.0 },
    selection_cell_size: 1.0,
    selection_seed: 42,
};
```

`100%` 密度始终表示“本层候选容量全部启用”，候选间距单独配置。默认竞争模式为
`Additive`，因此增加新层不会静默删除已有内容。

`VegetationClaimSampling::SourceCellOverlap`只改变`PriorityOverride`的空间所有权：
非零源采样单元与输出单元相交时可以清除低层，但被选中的高层仍输出自己的逐点
`density`。`VegetationFootprintScale::WorldSpace`则让净空保持固定米制，不消费仅用于
外观变化的实例缩放；默认值分别为`Point`和`InheritTransform`，保持既有调用方语义。

跨组净空按经过验证的无环图级联解析：若 A 排斥 B、B 排斥 C，已经被 A 清除的 B
不会继续向 C 输出“幽灵足迹”。源字段和 GPU 叶片统一使用世界端点映射及
`round(uv × (size - 1))` 最近采样，正负坐标和局部脏区重算保持一致。

仓库中的公共示例都只使用程序化输入：

```powershell
cargo run -p feathrise_vegetation --example competition
cargo run -p feathrise_vegetation --example dense_forest --features mesh
cargo run -p feathrise_vegetation --example procedural_blades --features blade
```

`dense_forest` 使用同一个抖动网格、实例 ID 和 Transform 驱动解析后的近档实体 Mesh、
远档 GPU 圆柱公告板与带 `16m / 20m` 滞回的近场代理；橙色根部标记是示例消息消费者，
替换为物理或玩法适配器不需要修改公共核心。设置
`VEGETATION_DENSE_FOREST_SMOKE_SECONDS` 可隐藏窗口并在指定秒数后成功退出。

`procedural_blades` 会在启动时创建叶片 Mesh、综合色纹理、高度图和密度图，不读取
外部工程的 GLB、PNG 或仓库全局 shader。设置
`VEGETATION_BLADE_PUBLIC_SMOKE_SECONDS` 可让隐藏窗口在指定秒数后自动成功退出。

当前正式支持 Windows DX12/Vulkan；Linux 和 macOS 只保证编译并视为实验性，不支持
WASM。当前 Cargo 清单允许发布到 crates.io；本地源码和发布版本使用相同的功能边界。
