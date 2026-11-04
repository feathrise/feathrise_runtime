//! GPU草地压缩容量、二维工作组派发和间接状态字节编码纯函数。

use bevy::math::UVec2; // 返回候选正方形对应的二维瓦片布局。

use crate::components::GpuBladePatch; // 由草窗常驻半径、格距和候选上限推导安全输出容量。
use crate::constants::{
    // 所有字节和派发计算使用统一常量，避免Rust与WGSL漂移。
    GPU_GRASS_COMPACT_INSTANCE_BYTES,          // 单株紧凑实例步长。
    GPU_GRASS_DEPTH_BUCKET_BASE_VERTEX_OFFSET, // 深度桶共享顶点切片起点偏移。
    GPU_GRASS_DEPTH_BUCKET_CAPACITY_OFFSET,    // 深度桶实例分区容量偏移。
    GPU_GRASS_DEPTH_BUCKET_COUNT,              // 每个LOD窗口固定径向深度分桶数。
    GPU_GRASS_DEPTH_BUCKET_FIRST_INDEX_OFFSET, // 深度桶共享索引切片起点偏移。
    GPU_GRASS_DEPTH_BUCKET_FIRST_INSTANCE_OFFSET, // 深度桶实例分区起点偏移。
    GPU_GRASS_DEPTH_BUCKET_INDEX_COUNT_OFFSET, // 深度桶原型索引数量偏移。
    GPU_GRASS_DEPTH_BUCKET_INSTANCE_COUNT_OFFSET, // 深度桶安全实例数量偏移。
    GPU_GRASS_DEPTH_BUCKET_OVERFLOW_INSTANCES_OFFSET, // 深度桶容量溢出计数偏移。
    GPU_GRASS_DEPTH_BUCKET_RESERVED_INSTANCES_OFFSET, // 深度桶原始预留计数偏移。
    GPU_GRASS_DEPTH_BUCKET_STATE_BYTES,        // 每个桶的间接参数与原子状态步长。
    GPU_GRASS_DEPTH_BUCKETS_BYTES,             // 每圈全部深度桶模板字节数。
    GPU_GRASS_DRAW_STATE_ACTIVE_TILES_OFFSET,  // 聚合状态活跃瓦片数量偏移。
    GPU_GRASS_DRAW_STATE_BASE_VERTEX_OFFSET,   // 聚合状态共享顶点切片起点偏移。
    GPU_GRASS_DRAW_STATE_BUFFER_GENERATION_OFFSET, // 聚合状态缓冲代次偏移。
    GPU_GRASS_DRAW_STATE_BYTES,                // 间接状态固定字节数。
    GPU_GRASS_DRAW_STATE_CANDIDATE_COUNT_OFFSET, // 聚合状态候选数量偏移。
    GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET,      // 聚合状态紧凑输出容量偏移。
    GPU_GRASS_DRAW_STATE_DENSITY_SURVIVORS_OFFSET, // 聚合状态密度阶段存活数偏移。
    GPU_GRASS_DRAW_STATE_DISPATCH_GROUP_COUNT_OFFSET, // 聚合状态细分类工作组数量偏移。
    GPU_GRASS_DRAW_STATE_DISTANCE_SURVIVORS_OFFSET, // 聚合状态距离阶段存活数偏移。
    GPU_GRASS_DRAW_STATE_FIRST_INDEX_OFFSET,   // 聚合状态共享索引切片起点偏移。
    GPU_GRASS_DRAW_STATE_FIRST_INSTANCE_OFFSET, // 聚合状态首实例保留位偏移。
    GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET,   // 聚合状态原型索引数量偏移。
    GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET, // 聚合状态最终实绘数量偏移。
    GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET,     // 聚合状态近中远LOD编号偏移。
    GPU_GRASS_DRAW_STATE_OVERFLOW_INSTANCES_OFFSET, // 聚合状态容量溢出数量偏移。
    GPU_GRASS_DRAW_STATE_RESERVED_INSTANCES_OFFSET, // 聚合状态原始预留数量偏移。
    GPU_GRASS_DRAW_STATE_SCANNED_CANDIDATES_OFFSET, // 聚合状态实际扫描候选数量偏移。
    GPU_GRASS_DRAW_STATE_SURFACE_SURVIVORS_OFFSET, // 聚合状态合法地表存活数量偏移。
    GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET,   // 聚合状态粗瓦片总数量偏移。
    GPU_GRASS_DRAW_STATE_VEGETATION_PROFILE_OFFSET, // 聚合状态内容profile编号偏移。
    GPU_GRASS_DRAW_STATE_VERTEX_COUNT_OFFSET,  // 聚合状态当前原型真实顶点数偏移。
    GPU_GRASS_TILE_DISPATCH_BYTES,             // GPU生成的间接派发状态字节数。
    GPU_GRASS_TILE_SIDE,                       // 一个瓦片恰好由一个细分类工作组处理。
};
use crate::types::GpuBladeCompactInstance; // 让实际缓冲字节计算直接依赖CPU镜像布局而非重复魔数。

/// 根据圆形可见范围计算绝不会小于所有可能存活根位置的正方形容量。
pub(super) fn compact_output_capacity(patch: &GpuBladePatch) -> u32 {
    if patch.uses_volume_source() {
        return patch.instance_count();
    } // 三维作者点可以集中在洞底；不能套用二维均匀格的面积上界。
    if patch.uses_ultra_lod_budget() {
        let budget_block_side = patch.ultra_lod_budget_block_side().max(1) as u64; // 与WGSL世界预算块单边保持同一高、中、低档。
        let blocks_per_side =
            (patch.grid_side() as u64).saturating_add(budget_block_side - 1) / budget_block_side; // 任意窗口对齐下向上取整给出可被选中超远代理数的严格正方形上界。
        let structurally_selected_upper_bound = blocks_per_side.saturating_mul(blocks_per_side); // 每个固定世界预算块最多写出一个超远代理。
        let capacity_with_bucket_headroom = structurally_selected_upper_bound.saturating_mul(2); // 八个等面积桶共享两倍全局上界，严格验收仍要求溢出为零。
        return capacity_with_bucket_headroom
            .min(patch.instance_count() as u64) // 小型测试窗口不会因安全余量反向扩容。
            .min(u32::MAX as u64) as u32; // 当前storage索引与间接实例数继续使用u32。
    } // 完成第四级超远固定预算容量路径。
    let spacing = patch.grid_spacing().max(f32::EPSILON); // 防止损坏参数造成除零或无穷容量。
    let visible_diameter_cells = (patch.visible_radius() * 2.0 / spacing).ceil() as u64; // 覆盖整个外圈直径需要的格数。
    let safe_side = visible_diameter_cells.saturating_add(4); // 两侧各留至少一个抖动和边界取整安全格。
    let capacity = safe_side
        .saturating_mul(safe_side) // 圆外接安全正方形给出严格容量上界。
        .min(patch.instance_count() as u64) // 小测试块不得反向扩容到超过真实候选数。
        .min(u32::MAX as u64); // GPU间接实例数和数组索引当前仍使用u32。
    capacity as u32 // 返回当前草窗持久缓冲容量。
}

/// 把端点式正方形候选网格切分为十六乘十六瓦片布局。
pub(super) fn tile_dimensions(patch: &GpuBladePatch) -> UVec2 {
    let tile_side = patch.grid_side().saturating_add(GPU_GRASS_TILE_SIDE - 1) / GPU_GRASS_TILE_SIDE; // 向上取整使最外侧不足十六格的候选仍进入一个合法边缘瓦片。
    UVec2::splat(tile_side) // 当前候选窗口始终为正方形，两轴瓦片数保持一致。
}

/// 返回一个草窗全部粗瓦片的线性数量。
pub(super) fn tile_count(tile_dimensions: UVec2) -> u32 {
    tile_dimensions.x.saturating_mul(tile_dimensions.y) // 当前千万候选档只有约三万五千瓦片，不接近u32上限。
}

/// 返回粗剔除入口需要派发的六十四线程工作组数。
pub(super) fn tile_cull_group_count(tile_count: u32) -> u32 {
    tile_count.saturating_add(63) / 64 // 每个线程只判定一个瓦片并把活跃编号追加到GPU列表。
}

/// 创建可被GPU写入并由`dispatch_workgroups_indirect`消费的十六字节初值。
pub(super) fn encode_tile_dispatch() -> [u8; GPU_GRASS_TILE_DISPATCH_BYTES as usize] {
    let mut bytes = [0_u8; GPU_GRASS_TILE_DISPATCH_BYTES as usize]; // X和活跃计数必须每帧清零，避免复用上一帧瓦片列表长度。
    write_u32(&mut bytes, 4, 1); // 间接派发Y固定为一。
    write_u32(&mut bytes, 8, 1); // 间接派发Z固定为一。
    bytes // 字节十二的原子活跃计数保持零并由粗剔除入口追加。
}

/// 返回当前紧凑实例缓冲需要的字节数。
pub(super) fn compact_buffer_bytes(capacity: u32) -> u64 {
    debug_assert_eq!(
        std::mem::size_of::<GpuBladeCompactInstance>() as u64,
        GPU_GRASS_COMPACT_INSTANCE_BYTES,
    ); // 调试构建再次确认CPU镜像与公开WGSL步长常量一致。
    (capacity as u64).saturating_mul(std::mem::size_of::<GpuBladeCompactInstance>() as u64) // 使用u64避免未来压力档乘法溢出。
}

/// 创建计算着色器和异步诊断共同解释的八十字节聚合状态具名输入。
pub(super) struct GpuBladeDrawStateTemplate {
    pub(super) index_count: u32,        // 原始grass.glb primitive索引数量。
    pub(super) first_index: u32,        // MeshAllocator共享索引缓冲中的起始索引。
    pub(super) base_vertex: i32,        // MeshAllocator共享顶点缓冲中的基准顶点。
    pub(super) capacity: u32,           // 输出实例缓冲安全容量。
    pub(super) candidate_count: u32,    // 本草窗需要测试的原始候选数量。
    pub(super) tile_count: u32,         // 本草窗候选正方形切分后的粗瓦片总数。
    pub(super) lod_index: u32,          // 零至三分别标记近中远超远几何档供GPU调试器识别。
    pub(super) buffer_generation: u32,  // 缓冲扩容代次，供后续异步统计识别旧样本。
    pub(super) vegetation_profile: u32, // 零普通草、一麦田等稳定内容编号供F3拆分。
    pub(super) vertex_count: u32,       // 当前原型真实顶点数供F3计算实际几何预算。
} // 完成具名聚合状态模板，避免八个相邻位置参数发生顺序误用。

/// 把具名草窗状态模板编码为 Rust、WGSL 与异步回读共同解释的固定布局。
pub(super) fn encode_draw_state(
    template: GpuBladeDrawStateTemplate, // 调用点必须按字段语义构造，禁止依赖八项位置顺序。
) -> [u8; GPU_GRASS_DRAW_STATE_BYTES as usize] {
    let GpuBladeDrawStateTemplate {
        index_count,        // 解构原型索引数量供间接ABI头部使用。
        first_index,        // 解构共享索引缓冲绝对起点。
        base_vertex,        // 解构共享顶点缓冲有符号起点。
        capacity,           // 解构紧凑输出安全上限。
        candidate_count,    // 解构当前草窗原始候选数量。
        tile_count,         // 解构当前草窗粗瓦片总量。
        lod_index,          // 解构近中远诊断编号。
        buffer_generation,  // 解构持久缓冲分配代次。
        vegetation_profile, // 解构普通草、麦田或后续内容profile编号。
        vertex_count,       // 解构当前GLB primitive真实顶点数。
    } = template; // 后续字节编码继续使用短局部名并保持现有ABI不变。
    let mut bytes = [0_u8; GPU_GRASS_DRAW_STATE_BYTES as usize]; // 所有原子计数器和保留位默认归零。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET,
        index_count,
    ); // 保留原型索引数量供GPU调试器直接核对。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET, 0); // 聚合实绘实例数由finalize计算写入并供F3回读。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_FIRST_INDEX_OFFSET,
        first_index,
    ); // 保留共享索引切片起点供状态诊断。
    write_i32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_BASE_VERTEX_OFFSET,
        base_vertex,
    ); // 保留共享顶点切片起点的有符号布局。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_FIRST_INSTANCE_OFFSET, 0); // 聚合状态不参与分桶绘制，首实例保持零。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_RESERVED_INSTANCES_OFFSET,
        0,
    ); // 汇总预留数由finalize计算写入。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_OVERFLOW_INSTANCES_OFFSET,
        0,
    ); // 汇总溢出数由finalize计算写入。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_DISTANCE_SURVIVORS_OFFSET,
        0,
    ); // 距离阶段计数由Compute原子累计。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_DENSITY_SURVIVORS_OFFSET, 0); // 密度阶段计数由Compute原子累计。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_SURFACE_SURVIVORS_OFFSET, 0); // 地表阶段计数由Compute原子累计。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET, capacity); // 状态capacity供写入越界保护和finalize钳制。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_CANDIDATE_COUNT_OFFSET,
        candidate_count,
    ); // 状态candidate_count供WGSL尾线程判断。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_DISPATCH_GROUP_COUNT_OFFSET,
        0,
    ); // 实际细分类工作组数由GPU瓦片收尾入口写入。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET, lod_index); // 状态与紧凑实例记录使用同一近中远材质级别。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_BUFFER_GENERATION_OFFSET,
        buffer_generation,
    ); // 保存当前缓冲代次。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET,
        tile_count,
    ); // 粗瓦片总数作为F3稳定分母。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_ACTIVE_TILES_OFFSET, 0); // 活跃瓦片数由GPU粗剔除收尾入口写入。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_SCANNED_CANDIDATES_OFFSET,
        0,
    ); // 实际进入细分类的候选数由活跃瓦片线程累计。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_VEGETATION_PROFILE_OFFSET,
        vegetation_profile,
    ); // 尾部第一字段让异步回读按内容拆分而不依赖实体查询顺序。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_VERTEX_COUNT_OFFSET,
        vertex_count,
    ); // 尾部第二字段保存当前真实原型顶点数，麦田不会套用普通草预算。
    bytes // 返回可由RenderQueue直接写入COPY_DST状态缓冲的字节数组。
}

/// 创建八个由近到远的等容量输出分区和索引间接绘制模板。
pub(super) fn encode_volume_depth_bucket_draws(
    index_count: u32,
    first_index: u32,
    base_vertex: i32,
    capacity: u32,
) -> [u8; GPU_GRASS_DEPTH_BUCKETS_BYTES as usize] {
    let mut bytes = encode_depth_bucket_draws(index_count, first_index, base_vertex, capacity);
    for bucket in 0..GPU_GRASS_DEPTH_BUCKET_COUNT {
        let offset = bucket as usize * GPU_GRASS_DEPTH_BUCKET_STATE_BYTES as usize;
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_FIRST_INSTANCE_OFFSET,
            if bucket == 0 { 0 } else { capacity },
        );
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_CAPACITY_OFFSET,
            if bucket == 0 { capacity } else { 0 },
        );
    }
    bytes // 非均匀洞内点全部使用共享输出区，避免等面积八桶导致有空闲缓冲却丢草。
}
pub(super) fn encode_depth_bucket_draws(
    index_count: u32, // 原始草网格索引数量，写入每个桶的间接参数。
    first_index: u32, // MeshAllocator共享索引缓冲中的起始索引。
    base_vertex: i32, // MeshAllocator共享顶点缓冲中的基准顶点。
    capacity: u32,    // 当前完整紧凑实例缓冲容量，由八桶无损瓜分。
) -> [u8; GPU_GRASS_DEPTH_BUCKETS_BYTES as usize] {
    let mut bytes = [0_u8; GPU_GRASS_DEPTH_BUCKETS_BYTES as usize]; // 每帧实例数、预留量和溢出量必须全部归零。
    let capacity_per_bucket = capacity / GPU_GRASS_DEPTH_BUCKET_COUNT; // 先为每个等面积桶分配相同基础容量。
    let remainder = capacity % GPU_GRASS_DEPTH_BUCKET_COUNT; // 不能整除的少量槽依次交给靠近玩家的桶。
    let mut first_instance = 0_u32; // 逐桶前缀和定义同一实例缓冲内互不重叠的区域起点。
    for bucket_index in 0..GPU_GRASS_DEPTH_BUCKET_COUNT {
        let bucket_capacity = capacity_per_bucket + u32::from(bucket_index < remainder); // 所有桶容量相差最多一株且总和严格等于原容量。
        let offset = (bucket_index as usize) * GPU_GRASS_DEPTH_BUCKET_STATE_BYTES as usize; // 显式分组桶索引转换，三十二字节记录满足间接绘制偏移要求。
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_INDEX_COUNT_OFFSET,
            index_count,
        ); // DrawIndexedIndirectArgs.index_count。
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_INSTANCE_COUNT_OFFSET,
            0,
        ); // DrawIndexedIndirectArgs.instance_count等待GPU收尾。
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_FIRST_INDEX_OFFSET,
            first_index,
        ); // DrawIndexedIndirectArgs.first_index。
        write_i32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_BASE_VERTEX_OFFSET,
            base_vertex,
        ); // DrawIndexedIndirectArgs.base_vertex。
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_FIRST_INSTANCE_OFFSET,
            first_instance,
        ); // DrawIndexedIndirectArgs.first_instance直接选择当前桶实例分区。
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_RESERVED_INSTANCES_OFFSET,
            0,
        ); // 当前桶全局预留原子计数每帧归零。
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_CAPACITY_OFFSET,
            bucket_capacity,
        ); // Compute写入严格限制在当前分区容量内。
        write_u32(
            &mut bytes,
            offset + GPU_GRASS_DEPTH_BUCKET_OVERFLOW_INSTANCES_OFFSET,
            0,
        ); // 当前桶溢出原子计数每帧归零。
        first_instance = first_instance.saturating_add(bucket_capacity); // 下一个桶紧接当前分区末尾，不浪费实例缓冲。
    }
    debug_assert_eq!(first_instance, capacity); // 八桶分区必须完整覆盖且不得越过原有实例容量。
    bytes // 返回可同时由RenderQueue、Compute与八次间接绘制消费的模板。
}

/// 在固定状态字节数组中写入一个小端u32。
fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes()); // WebGPU原生间接参数按当前支持平台小端布局写入。
}

/// 在固定状态字节数组中写入一个小端i32。
fn write_i32(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes()); // 保留负base_vertex的二进制补码表示。
}

#[cfg(test)] // 纯函数测试不需要启动RenderApp或创建显卡设备。
mod tests {
    #[test]
    fn volume_density_subcells_and_concentrated_capacity_do_not_use_outdoor_area_bounds() {
        use super::*;
        let patch = GpuBladePatch::square(bevy::math::Vec3::ZERO, 16.0, 257, 17)
            .with_volume_cell_source(64, 0.25);
        assert_eq!(patch.instance_count(), 2304); // 4×4正式格外加两圈抖动候选；按唯一作者范围裁掉多余格。
        assert_eq!(compact_output_capacity(&patch), 2304);
        let bytes = encode_volume_depth_bucket_draws(3, 0, 0, 1024);
        for bucket in 0..GPU_GRASS_DEPTH_BUCKET_COUNT {
            let offset = bucket as usize * GPU_GRASS_DEPTH_BUCKET_STATE_BYTES as usize
                + GPU_GRASS_DEPTH_BUCKET_CAPACITY_OFFSET;
            let capacity = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            assert_eq!(capacity, if bucket == 0 { 1024 } else { 0 });
        }
    }
    use bevy::prelude::Vec3; // 构造不同常驻半径和格距的草窗参数。

    use super::*; // 访问容量、派发和状态编码函数。

    #[test] // 正式三个重叠常驻层的输出缓冲都必须低于常见128MiB单绑定限制。
    fn physical_grass_layers_use_circle_bounded_compact_capacity() {
        let near =
            GpuBladePatch::square(Vec3::ZERO, 117.6, 1345, 1).with_lod_range(0.0, 46.0, 0.01, 1.0); // 最细格窗口描述约一百八十一万候选。
        let middle =
            GpuBladePatch::square(Vec3::ZERO, 235.2, 1345, 2).with_lod_range(0.0, 104.0, 0.01, 1.0); // 二倍格窗口同样保留统一候选维度。
        let far = GpuBladePatch::square(Vec3::ZERO, 470.4, 1345, 3)
            .with_lod_range(0.0, 220.0, 0.01, 60.0); // 四倍格窗口在宽域外沿交给正式地表材质。
        assert!(compact_output_capacity(&near) < near.instance_count()); // 近层正方形安全带不进入输出缓冲。
        assert!(compact_output_capacity(&middle) < middle.instance_count()); // 中层只为玩家常驻圆预留输出容量。
        assert!(compact_output_capacity(&far) < far.instance_count()); // 远层同样排除正方形窗口四角。
        assert!(compact_buffer_bytes(compact_output_capacity(&far)) < 128 * 1024 * 1024); // 远层单storage binding保持常见设备限制内。
    }

    #[test] // 高密度超远窗口必须把一千二百三十九万候选压入约一百一十八点三MiB固定实例池。
    fn ultra_layer_uses_world_stable_structural_budget_capacity() {
        let ultra = GpuBladePatch::square(Vec3::ZERO, 2464.0, 3521, 4)
            .with_lod_range(0.0, 1220.0, 0.01, 220.0)
            .with_lod_level(3)
            .with_hierarchical_lod(true)
            .with_ultra_lod_budget(crate::types::GpuBladeUltraLodBudget::High); // 复现二乘二高密度预算块超远规格。
        assert_eq!(ultra.instance_count(), 12_397_441); // 候选口径保持完整3521平方而不是偷换成CPU生成数量。
        assert_eq!(ultra.ultra_lod_budget_block_side(), 2); // 高密度档必须与WGSL每四父格一代理一致。
        assert_eq!(compact_output_capacity(&ultra), 6_202_242); // 1761平方结构上界乘两倍深度桶余量。
        assert_eq!(
            compact_buffer_bytes(compact_output_capacity(&ultra)),
            124_044_840
        ); // 单窗口固定实例池约一百一十八点三MiB。
        assert!(compact_buffer_bytes(compact_output_capacity(&ultra)) < 128 * 1024 * 1024); // 明确低于常见WebGPU单storage绑定上限。
    }

    #[test] // 当前远层必须把约一百八十一万候选压成固定粗瓦片测试。
    fn far_layer_is_partitioned_into_sixteen_by_sixteen_tiles() {
        let far = GpuBladePatch::square(Vec3::ZERO, 470.4, 1345, 3); // 复现二百二十米远层候选正方形。
        let dimensions = tile_dimensions(&far); // 1345格按十六向上取整为85块。
        assert_eq!(dimensions, UVec2::splat(85)); // 两轴都必须覆盖最外侧单格边缘瓦片。
        assert_eq!(tile_count(dimensions), 7_225); // 粗测试量仍只有原候选量约千分之四。
        assert_eq!(tile_cull_group_count(7_225), 113); // 六十四线程组向上取整且不遗漏尾部瓦片。
    }

    #[test] // 超远候选仍使用完整固定瓦片表，不按原子追加顺序截断导致闪烁。
    fn ultra_layer_keeps_full_fixed_tile_list_capacity() {
        let ultra = GpuBladePatch::square(Vec3::ZERO, 2464.0, 3521, 4); // 复现第四级端点式候选网格。
        let dimensions = tile_dimensions(&ultra); // 3521格按十六向上取整为221块。
        assert_eq!(dimensions, UVec2::splat(221)); // 两轴完整覆盖超远窗口边缘候选。
        assert_eq!(tile_count(dimensions), 48_841); // 活跃列表拥有固定完整容量，不依赖相机顺序。
        assert_eq!(tile_cull_group_count(48_841), 764); // 粗剔除只需七百六十四个六十四线程组。
    }

    #[test] // 间接参数前二十字节必须和wgpu结构布局一致。
    fn encoded_state_starts_with_draw_indexed_indirect_args() {
        let bytes = encode_draw_state(GpuBladeDrawStateTemplate {
            index_count: 39,       // 使用正式近景草相同索引数量。
            first_index: 123,      // 使用非零共享索引起点。
            base_vertex: -7,       // 使用负值覆盖有符号基准顶点编码。
            capacity: 456,         // 使用非零紧凑输出容量。
            candidate_count: 789,  // 使用非零原始候选数量。
            tile_count: 17,        // 使用非零粗瓦片数量。
            lod_index: 1,          // 使用中景编号覆盖LOD字段。
            buffer_generation: 2,  // 使用第二代缓冲覆盖代次字段。
            vegetation_profile: 1, // 使用麦田内容编号覆盖尾部profile字段。
            vertex_count: 40,      // 使用中景麦簇真实顶点数覆盖尾部几何字段。
        }); // 完成全部具名字段编码测试输入。
        assert_eq!(
            u32::from_le_bytes(
                bytes[GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET
                    ..GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            39
        ); // index_count保持三十九索引。
        assert_eq!(
            u32::from_le_bytes(
                bytes[GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET
                    ..GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            0
        ); // instance_count等待GPU收尾。
        assert_eq!(
            u32::from_le_bytes(
                bytes[GPU_GRASS_DRAW_STATE_FIRST_INDEX_OFFSET
                    ..GPU_GRASS_DRAW_STATE_FIRST_INDEX_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            123
        ); // first_index保持共享切片起点。
        assert_eq!(
            i32::from_le_bytes(
                bytes[GPU_GRASS_DRAW_STATE_BASE_VERTEX_OFFSET
                    ..GPU_GRASS_DRAW_STATE_BASE_VERTEX_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            -7
        ); // base_vertex保留符号。
        assert_eq!(
            u32::from_le_bytes(
                bytes[GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET
                    ..GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            456
        ); // capacity位于WGSL约定偏移。
        assert_eq!(
            u32::from_le_bytes(
                bytes[GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET
                    ..GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            1
        ); // 中景材质档必须写入状态诊断。
        assert_eq!(
            u32::from_le_bytes(
                bytes[GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET
                    ..GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            17
        ); // 粗瓦片总数写入扩展统计区。
        assert_eq!(encode_tile_dispatch()[4..12], [1, 0, 0, 0, 1, 0, 0, 0]); // 间接Y/Z固定一且X等待GPU填写。
    }

    #[test] // 八桶必须完整瓜分原容量且每条记录可直接作为索引间接参数消费。
    fn depth_bucket_template_partitions_capacity_without_gaps() {
        let bytes = encode_depth_bucket_draws(39, 123, -7, 101); // 使用不能被八整除的容量覆盖余数分配。
        let mut expected_first_instance = 0_u32; // 从实例缓冲零槽开始逐桶核对前缀。
        let mut total_capacity = 0_u32; // 汇总所有分区确保没有增加或丢失实例内存。
        for bucket_index in 0..GPU_GRASS_DEPTH_BUCKET_COUNT {
            let offset = (bucket_index as usize) * GPU_GRASS_DEPTH_BUCKET_STATE_BYTES as usize; // 显式分组桶索引转换并定位当前三十二字节记录。
            let read_u32 = |relative: usize| {
                u32::from_le_bytes(
                    bytes[offset + relative..offset + relative + 4]
                        .try_into()
                        .unwrap(),
                )
            }; // 按WGSL小端字段读取当前记录。
            assert_eq!(read_u32(GPU_GRASS_DEPTH_BUCKET_INDEX_COUNT_OFFSET), 39); // 每桶复用相同原型索引数量。
            assert_eq!(read_u32(GPU_GRASS_DEPTH_BUCKET_INSTANCE_COUNT_OFFSET), 0); // 首帧实例数等待GPU写入。
            assert_eq!(read_u32(GPU_GRASS_DEPTH_BUCKET_FIRST_INDEX_OFFSET), 123); // 每桶复用相同共享索引起点。
            assert_eq!(
                i32::from_le_bytes(
                    bytes[offset + GPU_GRASS_DEPTH_BUCKET_BASE_VERTEX_OFFSET
                        ..offset + GPU_GRASS_DEPTH_BUCKET_BASE_VERTEX_OFFSET + 4]
                        .try_into()
                        .unwrap(),
                ),
                -7
            ); // 有符号基准顶点保持不变。
            assert_eq!(
                read_u32(GPU_GRASS_DEPTH_BUCKET_FIRST_INSTANCE_OFFSET),
                expected_first_instance
            ); // 当前桶必须紧接上一桶末尾。
            let bucket_capacity = read_u32(GPU_GRASS_DEPTH_BUCKET_CAPACITY_OFFSET); // 读取当前实例分区长度。
            assert!((12..=13).contains(&bucket_capacity)); // 一百零一槽分八桶只能得到十二或十三。
            expected_first_instance += bucket_capacity; // 推进下一桶期望起点。
            total_capacity += bucket_capacity; // 汇总实际分配容量。
        }
        assert_eq!(total_capacity, 101); // 分桶不允许增加显存也不允许浪费任何已有槽位。
    }
}
