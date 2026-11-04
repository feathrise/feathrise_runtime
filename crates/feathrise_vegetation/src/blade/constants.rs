//! GPU 草地计算压缩与间接绘制共享常量。

/// 每个紧凑草实例只保存世界根位置和量化后的旋转、缩放、稳定外观与材质级别。
pub(crate) const GPU_GRASS_COMPACT_INSTANCE_BYTES: u64 = 20; // 三维表面额外携带四字节压缩法线，外观仍位于字节12。

/// 间接绘制参数、压缩分配器、瓦片统计和阶段统计共用一个八十字节状态块。
pub(crate) const GPU_GRASS_DRAW_STATE_BYTES: u64 = 80; // 状态保留聚合诊断，实际绘制参数由独立深度桶缓冲承载。

/// 聚合状态中的原型索引数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET: usize = 0; // 对应WGSL `GrassDrawState.index_count`。

/// 聚合状态中的最终实绘实例数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET: usize = 4; // 对应WGSL `GrassDrawState.instance_count`。

/// 聚合状态中的共享索引切片起点字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_FIRST_INDEX_OFFSET: usize = 8; // 对应WGSL `GrassDrawState.first_index`。

/// 聚合状态中的共享顶点切片起点字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_BASE_VERTEX_OFFSET: usize = 12; // 对应WGSL `GrassDrawState.base_vertex`。

/// 聚合状态中的首实例保留字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_FIRST_INSTANCE_OFFSET: usize = 16; // 聚合状态不直接绘制，当前固定写零。

/// 聚合状态中的原始预留实例数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_RESERVED_INSTANCES_OFFSET: usize = 20; // Compute收尾阶段汇总八个深度桶的预留量。

/// 聚合状态中的容量溢出实例数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_OVERFLOW_INSTANCES_OFFSET: usize = 24; // F3回读使用该字段诊断输出缓冲是否不足。

/// 聚合状态中的距离阶段存活数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_DISTANCE_SURVIVORS_OFFSET: usize = 28; // 对应第一级候选剔除统计。

/// 聚合状态中的密度阶段存活数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_DENSITY_SURVIVORS_OFFSET: usize = 32; // 对应第二级候选剔除统计。

/// 聚合状态中的合法地表存活数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_SURFACE_SURVIVORS_OFFSET: usize = 36; // 对应最终地表采样统计。

/// 聚合状态中的紧凑实例缓冲容量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET: usize = 40; // Compute写入前使用该字段执行严格越界保护。

/// 聚合状态中的候选实例数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_CANDIDATE_COUNT_OFFSET: usize = 44; // 候选线程使用该字段裁掉边缘工作组尾线程。

/// 聚合状态中的细分类工作组数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_DISPATCH_GROUP_COUNT_OFFSET: usize = 48; // GPU粗剔除收尾阶段写入真实派发量。

/// 聚合状态中的近中远LOD编号字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET: usize = 52; // F3按该字段把真实实绘量归入四级LOD。

/// 聚合状态中的缓冲代次字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_BUFFER_GENERATION_OFFSET: usize = 56; // 异步诊断可据此识别扩容前的旧状态。

/// 聚合状态中的粗瓦片总数字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET: usize = 60; // F3使用该字段作为粗剔除比例分母。

/// 聚合状态中的活跃粗瓦片数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_ACTIVE_TILES_OFFSET: usize = 64; // GPU粗距离和视锥判断完成后写入。

/// 聚合状态中的实际扫描候选数量字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_SCANNED_CANDIDATES_OFFSET: usize = 68; // 记录细分类真实启动的候选线程量。

/// 聚合状态中的叶片植被内容profile编号字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_VEGETATION_PROFILE_OFFSET: usize = 72; // F3按零普通草、一麦田等稳定编号拆分真实GPU统计。

/// 聚合状态中的当前原型实际顶点数字段字节偏移。
pub(crate) const GPU_GRASS_DRAW_STATE_VERTEX_COUNT_OFFSET: usize = 76; // F3由真实网格切片预算顶点，不再把麦田套用普通草常量。

/// 每个LOD环带固定切成八个等面积径向深度桶，由近到远提交以提高早期深度测试命中率。
pub const GPU_GRASS_DEPTH_BUCKET_COUNT: u32 = 8; // 每种内容的近中远LOD各固定八桶、合计二十四条命令；增加内容无需逐草排序。

/// 单个深度桶记录以二十字节索引间接参数开头，尾部保存预留容量与溢出计数。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_STATE_BYTES: u64 = 32; // 三十二字节步长兼顾间接偏移对齐和Compute原子字段。

/// 深度桶状态中的原型索引数量字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_INDEX_COUNT_OFFSET: usize = 0; // 对应`DrawIndexedIndirectArgs.index_count`。

/// 深度桶状态中的安全实例数量字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_INSTANCE_COUNT_OFFSET: usize = 4; // 对应`DrawIndexedIndirectArgs.instance_count`。

/// 深度桶状态中的共享索引切片起点字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_FIRST_INDEX_OFFSET: usize = 8; // 对应`DrawIndexedIndirectArgs.first_index`。

/// 深度桶状态中的共享顶点切片起点字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_BASE_VERTEX_OFFSET: usize = 12; // 对应`DrawIndexedIndirectArgs.base_vertex`。

/// 深度桶状态中的实例分区起点字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_FIRST_INSTANCE_OFFSET: usize = 16; // 对应`DrawIndexedIndirectArgs.first_instance`。

/// 深度桶状态中的全局预留计数字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_RESERVED_INSTANCES_OFFSET: usize = 20; // Compute工作组使用原子加法预留输出槽。

/// 深度桶状态中的实例分区容量字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_CAPACITY_OFFSET: usize = 24; // 八个桶的容量总和严格等于完整紧凑缓冲容量。

/// 深度桶状态中的容量溢出计数字段字节偏移。
pub(crate) const GPU_GRASS_DEPTH_BUCKET_OVERFLOW_INSTANCES_OFFSET: usize = 28; // 收尾阶段统计超过当前桶分区的候选量。

/// 单个LOD环带全部深度桶间接状态的固定字节数。
pub(crate) const GPU_GRASS_DEPTH_BUCKETS_BYTES: u64 =
    GPU_GRASS_DEPTH_BUCKET_STATE_BYTES * GPU_GRASS_DEPTH_BUCKET_COUNT as u64; // 八桶共二百五十六字节，每帧只重置这份小模板。

/// 每个粗剔除瓦片覆盖十六乘十六个候选，恰好对应一个二百五十六线程细分类工作组。
pub(crate) const GPU_GRASS_TILE_SIDE: u32 = 16; // 瓦片通过后无需二次拆分即可间接派发一个工作组。

/// 瓦片间接派发状态只保存XYZ工作组数与GPU原子追加计数。
pub(crate) const GPU_GRASS_TILE_DISPATCH_BYTES: u64 = 16; // 前十二字节可直接交给dispatch_workgroups_indirect。

/// 粗瓦片列表中的每个元素只保存原始二维瓦片的线性编号。
pub(crate) const GPU_GRASS_TILE_INDEX_BYTES: u64 = 4; // u32足以覆盖当前乃至一亿候选的瓦片数量。

/// 手工草窗AABB相对候选窗口每个水平边界额外保留的统一安全距离。
pub const GPU_GRASS_PATCH_AABB_HORIZONTAL_MARGIN: f32 = 2.0; // 与Compute逐株保守半径统一，完整覆盖远圈邻格重映射、98%格内抖动、最宽LOD草片和最多48厘米合成形变。
