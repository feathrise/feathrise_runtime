use super::*; // 验证无GPU环境下的80字节ABI解析与多圈汇总。

/// 向测试状态的指定字节偏移写入小端u32。
fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes()); // 与生产`encode_draw_state`使用相同端序。
}

#[test]
fn decode_aggregates_true_gpu_survivors_and_overflow() {
    let mut bytes = vec![0_u8; GPU_GRASS_DRAW_STATE_BYTES as usize * 2]; // 构造两个连续草窗状态。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET, 39); // 第一窗使用普通草近景真实索引数。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET, 70); // 第一圈实绘七十根。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_OVERFLOW_INSTANCES_OFFSET,
        2,
    ); // 第一圈有两根超出容量。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_DISTANCE_SURVIVORS_OFFSET,
        90,
    ); // 第一圈距离阶段存活九十根。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_DENSITY_SURVIVORS_OFFSET,
        80,
    ); // 第一圈密度阶段存活八十根。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_SURFACE_SURVIVORS_OFFSET,
        72,
    ); // 第一圈地表阶段最终存活七十二根。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET, 75); // 第一圈紧凑实例缓冲容量七十五根。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_CANDIDATE_COUNT_OFFSET, 100); // 第一圈候选一百根。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET, 0); // 第一圈标记为近景几何LOD。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_VEGETATION_PROFILE_OFFSET,
        0,
    ); // 第一圈归属普通草profile。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_VERTEX_COUNT_OFFSET, 17); // 第一圈使用普通草近景真实顶点数。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET, 10); // 第一圈共有十个粗瓦片。
    write_u32(&mut bytes, GPU_GRASS_DRAW_STATE_ACTIVE_TILES_OFFSET, 4); // 第一圈有四个活跃瓦片。
    write_u32(
        &mut bytes,
        GPU_GRASS_DRAW_STATE_SCANNED_CANDIDATES_OFFSET,
        100,
    ); // 第一圈扫描一百候选线程。
    let second = GPU_GRASS_DRAW_STATE_BYTES as usize; // 定位第二圈起始字节。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET,
        72,
    ); // 第二窗使用麦田远景真实索引数。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET,
        40,
    ); // 第二圈实绘四十根。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_DISTANCE_SURVIVORS_OFFSET,
        60,
    ); // 第二圈距离阶段存活六十根。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_DENSITY_SURVIVORS_OFFSET,
        50,
    ); // 第二圈密度阶段存活五十根。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_SURFACE_SURVIVORS_OFFSET,
        40,
    ); // 第二圈地表阶段存活四十根。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET,
        50,
    ); // 第二圈紧凑实例缓冲容量五十根。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_CANDIDATE_COUNT_OFFSET,
        100,
    ); // 第二圈候选一百根。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET,
        2,
    ); // 第二圈标记为远景几何LOD。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_VEGETATION_PROFILE_OFFSET,
        1,
    ); // 第二圈归属麦田profile。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_VERTEX_COUNT_OFFSET,
        32,
    ); // 第二圈使用麦田远景真实顶点数。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET,
        10,
    ); // 第二圈共有十个粗瓦片。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_ACTIVE_TILES_OFFSET,
        2,
    ); // 第二圈有两个活跃瓦片。
    write_u32(
        &mut bytes,
        second + GPU_GRASS_DRAW_STATE_SCANNED_CANDIDATES_OFFSET,
        80,
    ); // 第二圈扫描八十候选线程。

    let stats = decode_gpu_grass_stats(&bytes, 2, 1, 9); // 解析两圈并声明一圈被诊断容量截断。
    assert_eq!(stats.sample_sequence, 9); // 样本序号必须完整保留。
    assert_eq!(stats.sampled_windows, 2); // 实际解析两个草窗。
    assert_eq!(stats.truncated_windows, 1); // 诊断截断数不得被汇总丢失。
    assert_eq!(stats.candidate_count, 200); // 两圈候选总数为二百。
    assert_eq!(stats.distance_survivors, 150); // 距离存活量按圈汇总。
    assert_eq!(stats.density_survivors, 130); // 密度存活量按圈汇总。
    assert_eq!(stats.surface_survivors, 112); // 地表最终存活包含未实绘的溢出根数。
    assert_eq!(stats.compact_capacity, 125); // 两圈紧凑输出总容量按状态汇总。
    assert_eq!(stats.rendered_instances, 110); // 实绘数是间接参数中的钳制值。
    assert_eq!(stats.rendered_instances_by_lod, [70, 0, 40, 0, 0, 0, 0, 0]); // 近远两档必须按状态LOD分别汇总。
    assert_eq!(stats.total_tiles, 20); // 两圈粗瓦片总数按状态汇总。
    assert_eq!(stats.active_tiles, 6); // 两圈活跃瓦片数按状态汇总。
    assert_eq!(stats.scanned_candidates, 180); // 两圈实际细分类线程数按状态汇总。
    assert_eq!(stats.overflow_instances, 2); // 容量溢出需独立暴露。
    assert_eq!(stats.by_profile[0].sampled_windows, 1); // 普通草只收到第一窗。
    assert_eq!(
        stats.by_profile[0].rendered_instances_by_lod,
        [70, 0, 0, 0, 0, 0, 0, 0]
    ); // 普通草近景身份完整保留。
    assert_eq!(
        stats.by_profile[0].submitted_vertices_by_lod,
        [70 * 17, 0, 0, 0, 0, 0, 0, 0]
    ); // 普通草几何预算来自第一窗真实顶点数。
    assert_eq!(
        stats.by_profile[0].submitted_indices_by_lod,
        [70 * 39, 0, 0, 0, 0, 0, 0, 0]
    ); // 普通草几何预算来自第一窗真实索引数。
    assert_eq!(stats.by_profile[1].sampled_windows, 1); // 麦田只收到第二窗。
    assert_eq!(
        stats.by_profile[1].rendered_instances_by_lod,
        [0, 0, 40, 0, 0, 0, 0, 0]
    ); // 麦田远景身份不会被并入普通草。
    assert_eq!(
        stats.by_profile[1].submitted_vertices_by_lod,
        [0, 0, 40 * 32, 0, 0, 0, 0, 0]
    ); // 麦田使用自己的远景顶点数而非普通草常量。
    assert_eq!(
        stats.by_profile[1].submitted_indices_by_lod,
        [0, 0, 40 * 72, 0, 0, 0, 0, 0]
    ); // 麦田使用自己的远景索引数而非普通草常量。
    assert_eq!(
        stats.by_profile[0].dedicated_window_buffer_bytes()
            + stats.by_profile[1].dedicated_window_buffer_bytes(),
        stats.dedicated_window_buffer_bytes()
    ); // 分内容窗口显存之和必须严格等于聚合窗口显存。
}
