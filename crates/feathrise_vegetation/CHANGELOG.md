# Changelog

## Unreleased

- P2.4将严格GPU验证入口迁为`rpg_vegetation::blade::GpuBlade*`并退役独立
  `bevy_learn5_grass`包装package；P5.3b真实地图画面复验通过后继续删除
  `rpg_environment::grass`、`grass-compat`及全部`GpuGrass*`公开别名。
- 增加`VegetationClaimSampling`与独立`claim_density`，允许优先覆盖按源采样单元
  相交声明空间所有权，同时保持本层实际密度逐点采样。
- 增加`VegetationFootprintScale`，区分继承实例Transform的实际轮廓与固定世界米制
  净空；旋转矩形、椭圆和Mesh AABB搜索范围使用半对角线保守覆盖。
- 增加`VegetationLayoutCommit`，只在整批布局成功切换时发布与活动版本一致、按
  Y/X稳定排序的完整脏块快照；失败批次继续保留上一提交。
- 叶片布局桥接改为紧凑R8基础图集和完整Mip缓存；连续版本只重建提交脏块及受影响
  父Mip，图集范围变化时安全回退整图，局部与整图最终RGBA上传字节完全一致。
- 增加正式RenderApp候选Mip、带票据/revision/层身份的非阻塞完整链回读，以及全部
  绑定层通过后才原子采用的版本门；失败、超时、换版和迟到回调保留上一有效图片。
- 增加只在加载或换版后运行的GPU竞争输入与WGSL，直接处理实际/所有权密度、
  `Additive`、`WeightedPartition`、`PriorityOverride`、稳定选择值和稀疏足迹影响。
- 局部GPU候选先复制上一已验证Mip0，再只调度`VegetationLayoutCommit`提交块；同组
  层共享输入与GPU缓冲，并增加不可见`VegetationBladeValidationLayers`对等入口。
- 增加`VegetationBladeAdoptionPhase / VegetationBladeAdoptionStatus`只读快照，以
  活动revision、绑定层数和已采用/绑定窗口分子分母观察可见窗口的托底、校验、采用
  与拒绝状态，不把后台验证层或空世界误报为正式采用。

## 0.1.0

- 增加无限种植层、竞争组、三种竞争模式和确定性世界分配。
- 增加候选分布、密度字段、通用足迹、排斥边距与软恢复带。
- 增加关系式优先级和跨组排斥循环校验。
- 增加 64×64 事件脏块、暂存布局与原子版本切换。
- 增加近场混合代理消息和通用多部件 Mesh 后端。
- 将既有 GPU 叶片实现及三份内嵌 WGSL 迁入 `blade` feature，主 API 改用
  `GpuBlade*`，旧草地包通过隔离 feature 保留 `GpuGrass*` 兼容别名。
- 增加 `VegetationBladeLayer` 活动布局桥接；只在完整布局换版时更新同层共享密度图，
  失败或上传等待期间保留上一有效 GPU 输出。
- 增加程序化稠密树林示例，以及通用 Mesh 实例与活动分配场共享候选的生成入口。
- 跨组排斥按无环关系级联解析，禁止已被上游清除的候选继续输出下游幽灵足迹。
- CPU 源字段与 GPU 叶片统一使用端点式世界映射和最近整数纹素采样。
