//! 稀疏GPU查询映射字节到分内容草地计时快照的纯解码逻辑。

use crate::{
    resources::{GpuBladeGpuTimingStats, GpuBladeVegetationTimingStats},
    types::GPU_GRASS_PROFILE_CAPACITY,
}; // 解码结果同时发布草麦独立统计与既有近中远聚合字段。

use super::{
    super::{
        CANDIDATE_CLASSIFY_END_TIMESTAMP, COMPUTE_BEGIN_TIMESTAMP, COMPUTE_FINALIZE_END_TIMESTAMP,
        DRAW_LOD_COUNT, DRAW_TIMESTAMP_BASE, PIPELINE_STATISTICS_MASK_SHIFT,
        TILE_CULL_END_TIMESTAMP, compute_timestamp_index,
    },
    resolve_plan::build_query_resolve_plan,
}; // 解码必须复用GPU录制使用的固定查询ABI与同一稀疏解析计划。

/// 把一份已经完成映射的稀疏查询结果解码为可跨世界发布的诊断快照。
pub(super) fn decode_gpu_grass_timing(
    bytes: &[u8],                        // staging槽完整映射字节。
    write_mask: u64,                     // 本样本实际写入的查询槽掩码。
    sequence: u64,                       // 样本提交序号。
    timestamp_period_ns: f32,            // GPU tick对应纳秒数。
    timestamp_supported: bool,           // 当前后端时间戳能力。
    pipeline_statistics_supported: bool, // 当前后端管线统计能力。
) -> GpuBladeGpuTimingStats {
    let mut stats = GpuBladeGpuTimingStats {
        sample_sequence: sequence,     // 成功解析即发布非零递增序号。
        timestamp_supported,           // 保留设备能力供F3选择文本。
        pipeline_statistics_supported, // 保留统计能力供F3选择文本。
        ..Default::default()           // 各阶段只在对应查询位完整时写入真实数值。
    };
    let resolve_plan = build_query_resolve_plan(write_mask); // 按与GPU录制完全相同的规则恢复本样本稀疏布局。
    let duration_ms = |begin_index: u32, end_index: u32| -> f64 {
        let required_mask = (1_u64 << begin_index) | (1_u64 << end_index); // 一对起止槽必须都真实写入。
        if !timestamp_supported || write_mask & required_mask != required_mask {
            return 0.0; // 不完整查询对不读取解析缓冲残留数据。
        }
        let Some(begin_offset) = resolve_plan.timestamp_offset(begin_index) else {
            return 0.0; // 计划与掩码意外失配时宁可丢诊断，也不得读取错误位置。
        };
        let Some(end_offset) = resolve_plan.timestamp_offset(end_index) else {
            return 0.0; // 起止任一查询缺少解析结果时保持零耗时。
        };
        let begin = read_u64(bytes, begin_offset); // 从当前连续段内偏移读取开始设备tick。
        let end = read_u64(bytes, end_offset); // 从当前连续段内偏移读取结束设备tick。
        end.saturating_sub(begin) as f64 * f64::from(timestamp_period_ns) / 1_000_000.0 // tick乘纳秒周期后换算毫秒。
    };
    for profile_index in 0..GPU_GRASS_PROFILE_CAPACITY {
        let profile = profile_index as u32; // 固定四槽内容编号转换为查询ABI使用的u32。
        let compute_begin =
            compute_timestamp_index(profile, COMPUTE_BEGIN_TIMESTAMP).expect("合法内容有开始槽"); // 当前内容四阶段首槽。
        let tile_cull_end =
            compute_timestamp_index(profile, TILE_CULL_END_TIMESTAMP).expect("合法内容有瓦片槽"); // 当前内容瓦片阶段末槽。
        let candidate_classify_end =
            compute_timestamp_index(profile, CANDIDATE_CLASSIFY_END_TIMESTAMP)
                .expect("合法内容有分类槽"); // 当前内容分类阶段末槽。
        let compute_finalize_end = compute_timestamp_index(profile, COMPUTE_FINALIZE_END_TIMESTAMP)
            .expect("合法内容有收尾槽"); // 当前内容Compute最终槽。
        let mut profile_stats = GpuBladeVegetationTimingStats {
            tile_cull_ms: duration_ms(compute_begin, tile_cull_end), // 当前内容粗瓦片与派发参数阶段。
            candidate_classify_ms: duration_ms(tile_cull_end, candidate_classify_end), // 当前内容逐株分类压缩阶段。
            compute_finalize_ms: duration_ms(candidate_classify_end, compute_finalize_end), // 当前内容实例与间接参数收尾。
            compute_total_ms: duration_ms(compute_begin, compute_finalize_end), // 当前内容四个边界组成的总Compute耗时。
            ..Default::default() // 绘制和着色器调用量在下方逐LOD填写。
        };
        for lod_index in 0..DRAW_LOD_COUNT as usize {
            let measurement_index = profile * DRAW_LOD_COUNT + lod_index as u32; // 内容乘四加近中远超远LOD唯一定位测量。
            let begin_index = DRAW_TIMESTAMP_BASE + measurement_index * 2; // 十二组draw各占连续起止时间戳。
            profile_stats.draw_lod_ms[lod_index] = duration_ms(begin_index, begin_index + 1); // 解码当前内容LOD全部八个深度桶间接命令耗时。
            let statistics_bit = 1_u64 << (PIPELINE_STATISTICS_MASK_SHIFT + measurement_index); // 定位当前内容LOD统计完成标记。
            if pipeline_statistics_supported
                && write_mask & statistics_bit != 0
                && let Some(base) = resolve_plan.pipeline_statistics_offset(measurement_index)
            {
                profile_stats.vertex_shader_invocations_by_lod[lod_index] = read_u64(bytes, base); // 第一项是当前内容顶点着色器调用量。
                profile_stats.fragment_shader_invocations_by_lod[lod_index] =
                    read_u64(bytes, base + 24); // 第四项是当前内容片元着色器调用量。
            }
        }
        profile_stats.draw_total_ms = profile_stats.draw_lod_ms.iter().sum(); // 当前内容近中远绘制时间相加。
        stats.by_profile[profile_index] = profile_stats; // 发布当前内容完整独立计时快照。
    }
    stats.compute_total_ms = stats
        .by_profile
        .iter()
        .map(|profile| profile.compute_total_ms)
        .sum(); // 旧聚合字段继续表示所有内容Compute之和。
    stats.tile_cull_ms = stats
        .by_profile
        .iter()
        .map(|profile| profile.tile_cull_ms)
        .sum(); // 聚合普通草、麦田等全部瓦片阶段。
    stats.candidate_classify_ms = stats
        .by_profile
        .iter()
        .map(|profile| profile.candidate_classify_ms)
        .sum(); // 聚合全部内容候选分类阶段。
    stats.compute_finalize_ms = stats
        .by_profile
        .iter()
        .map(|profile| profile.compute_finalize_ms)
        .sum(); // 聚合全部内容Compute收尾阶段。
    stats.draw_total_ms = stats
        .by_profile
        .iter()
        .map(|profile| profile.draw_total_ms)
        .sum(); // 聚合全部内容近中远间接绘制时间。
    for lod_index in 0..DRAW_LOD_COUNT as usize {
        stats.draw_lod_ms[lod_index] = stats
            .by_profile
            .iter()
            .map(|profile| profile.draw_lod_ms[lod_index])
            .sum(); // 旧近中远耗时继续按LOD聚合所有内容。
        stats.vertex_shader_invocations_by_lod[lod_index] = stats.by_profile.iter().fold(
            0_u64, // 使用饱和加法防止损坏驱动统计造成诊断回绕。
            |total, profile| {
                total.saturating_add(profile.vertex_shader_invocations_by_lod[lod_index])
            },
        );
        stats.fragment_shader_invocations_by_lod[lod_index] = stats.by_profile.iter().fold(
            0_u64, // 片元统计同样只做安全聚合，不影响正式渲染。
            |total, profile| {
                total.saturating_add(profile.fragment_shader_invocations_by_lod[lod_index])
            },
        );
    }
    stats // 返回可跨线程和跨世界复制的完整诊断样本。
}

/// 从固定映射布局按小端读取一个u64查询值。
fn read_u64(bytes: &[u8], offset: u64) -> u64 {
    let offset = offset as usize; // 固定六千六百五十六字节范围可安全转换为平台usize。
    let Some(raw) = bytes.get(offset..offset + 8) else {
        return 0; // 损坏或短映射不允许诊断回调panic。
    };
    u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6],
        raw[7], // WebGPU查询结果按本机小端平台直接解析。
    ])
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            super::{PIPELINE_STATISTICS_QUERY_COUNT, TIMESTAMP_QUERY_COUNT},
            resolve_plan::TIMING_RESOLVE_BUFFER_BYTES,
        },
        *,
    }; // 验证分内容解码、设备tick换算和不完整查询对保护。

    /// 向测试查询布局写入一个小端u64。
    fn write_u64(bytes: &mut [u8], offset: u64, value: u64) {
        let offset = offset as usize; // 测试固定偏移均小于稀疏解析缓冲最坏容量。
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes()); // 与wgpu查询解析结果使用相同端序。
    }

    /// 允许多种内容浮点求和产生一个机器精度内的舍入差。
    fn assert_ms(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1.0e-12, // 纳秒换算和四项求和不应产生可观察毫秒误差。
            "actual={actual}, expected={expected}"  // 失败时直接指出具体计时值。
        );
    }

    #[test]
    fn decode_reports_compute_draw_and_shader_invocations() {
        let mut bytes = vec![0_u8; TIMING_RESOLVE_BUFFER_BYTES as usize]; // 构造完整固定查询回读布局。
        let write_mask = ((1_u64 << TIMESTAMP_QUERY_COUNT) - 1)
            | (((1_u64 << PIPELINE_STATISTICS_QUERY_COUNT) - 1) << PIPELINE_STATISTICS_MASK_SHIFT); // 声明四十个时间戳与十二项统计全部有效。
        let resolve_plan = build_query_resolve_plan(write_mask); // 全写掩码形成时间戳与统计各一个连续段。
        for profile in 0..GPU_GRASS_PROFILE_CAPACITY as u32 {
            let first_tick = 100_u64 + u64::from(profile) * 1_000; // 各内容使用明显不同的原始区间以发现串槽。
            for (stage, tick_offset) in [0_u64, 100, 400, 450].into_iter().enumerate() {
                let query_index =
                    compute_timestamp_index(profile, stage as u32).expect("合法内容阶段必须有槽"); // 取得当前内容四阶段固定槽。
                write_u64(
                    &mut bytes, // 固定最坏容量测试布局。
                    resolve_plan
                        .timestamp_offset(query_index)
                        .expect("全写时间戳必须存在解析偏移"), // 按正式稀疏计划定位时间戳结果。
                    first_tick + tick_offset, // 每种内容得到0.1、0.3、0.05和0.45毫秒分项。
                );
            }
            for lod in 0..DRAW_LOD_COUNT {
                let measurement = profile * DRAW_LOD_COUNT + lod; // 当前内容近中远测量编号。
                let begin_query = DRAW_TIMESTAMP_BASE + measurement * 2; // 绘制时间戳连续起止槽。
                let begin_tick = 10_000_u64 + u64::from(measurement) * 100; // 每组使用独立起始值。
                let duration_tick = u64::from((profile + 1) * 10 + lod * 5); // 内容和LOD共同决定可断言耗时。
                write_u64(
                    &mut bytes, // 写入正式解码使用的稀疏布局。
                    resolve_plan
                        .timestamp_offset(begin_query)
                        .expect("全写draw开始必须存在解析偏移"), // 定位当前内容LOD开始tick。
                    begin_tick, // 保存当前内容LOD独立开始tick。
                );
                write_u64(
                    &mut bytes, // 使用同一稀疏测试布局。
                    resolve_plan
                        .timestamp_offset(begin_query + 1)
                        .expect("全写draw结束必须存在解析偏移"), // 相邻查询保存结束tick。
                    begin_tick + duration_tick, // 产生普通草0.01起、麦田0.02起的独立绘制耗时。
                );
                let base = resolve_plan
                    .pipeline_statistics_offset(measurement)
                    .expect("全写统计必须存在解析偏移"); // 定位当前内容LOD统计记录。
                write_u64(&mut bytes, base, 1_000 + u64::from(measurement)); // 第一项写入顶点调用量。
                write_u64(&mut bytes, base + 24, 2_000 + u64::from(measurement)); // 第四项写入片元调用量。
            }
        }
        let stats = decode_gpu_grass_timing(
            &bytes,     // 使用完整测试布局。
            write_mask, // 所有固定查询均已写入。
            7,          // 验证样本序号保留。
            1_000.0,    // 每tick一微秒，便于精确断言毫秒。
            true,       // 启用时间戳解码。
            true,       // 启用管线统计解码。
        );
        assert_eq!(stats.sample_sequence, 7); // 样本序号必须原样发布。
        assert_ms(stats.by_profile[0].compute_total_ms, 0.45); // 普通草四阶段独立解码。
        assert_ms(stats.by_profile[1].compute_total_ms, 0.45); // 麦田不再与普通草共用Compute槽。
        for (actual, expected) in stats.by_profile[1]
            .draw_lod_ms
            .into_iter()
            .zip([0.02, 0.025, 0.03, 0.035])
        {
            assert_ms(actual, expected); // 麦田近中远独立绘制耗时。
        }
        assert_eq!(
            stats.by_profile[1].vertex_shader_invocations_by_lod,
            [1_004, 1_005, 1_006, 1_007, 0, 0, 0, 0]
        ); // 完整查询压力测试中profile一读取测量四至七而非普通草统计。
        assert_ms(stats.compute_total_ms, 1.8); // 旧聚合总耗时为四种内容各零点四五毫秒之和。
        assert_ms(stats.tile_cull_ms, 0.4); // 四种内容粗剔除各零点一毫秒。
        assert_ms(stats.candidate_classify_ms, 1.2); // 四种内容分类各零点三毫秒。
        assert_ms(stats.compute_finalize_ms, 0.2); // 四种内容收尾各零点零五毫秒。
        for (actual, expected) in stats.draw_lod_ms.into_iter().zip([0.1, 0.12, 0.14, 0.16]) {
            assert_ms(actual, expected); // 近中远超远分别聚合四种内容。
        }
        assert_ms(stats.draw_total_ms, 0.52); // 四档聚合绘制耗时总计零点五二毫秒。
        assert_eq!(
            stats.vertex_shader_invocations_by_lod,
            [4_024, 4_028, 4_032, 4_036, 0, 0, 0, 0]
        ); // 四档顶点调用量按四种内容求和。
        assert_eq!(
            stats.fragment_shader_invocations_by_lod,
            [8_024, 8_028, 8_032, 8_036, 0, 0, 0, 0]
        ); // 四档片元调用量按四种内容求和。
    }

    #[test]
    fn four_lod_layout_round_trips_grass_wheat_and_mixed_grain_profiles() {
        let write_mask = ((1_u64 << 8) - 1) // 普通草与麦田Compute写入时间戳零至七。
            | (((1_u64 << 4) - 1) << 12) // 杂粮profile三写入时间戳十二至十五，中间保留芦苇空洞。
            | (((1_u64 << 16) - 1) << DRAW_TIMESTAMP_BASE) // 普通草与麦田各四档Draw写入十六个起止时间戳。
            | (((1_u64 << 8) - 1) << (DRAW_TIMESTAMP_BASE + 24)) // 杂粮四档Draw占测量十二至十五对应的八个时间戳。
            | (((1_u64 << 8) - 1) << PIPELINE_STATISTICS_MASK_SHIFT) // 普通草与麦田统计占测量零至七。
            | (((1_u64 << 4) - 1) << (PIPELINE_STATISTICS_MASK_SHIFT + 12)); // 杂粮统计写到掩码最高四位，覆盖第64位边界。
        let resolve_plan = build_query_resolve_plan(write_mask); // 真实十二窗布局必须跨过芦苇Compute、Draw和统计查询空洞。
        let mut bytes = vec![0_u8; resolve_plan.resolved_bytes as usize]; // 只分配正式节点实际复制和映射的稀疏前缀。
        for profile in [0_u32, 1, 3] {
            let compute_base = 1_000_u64 + u64::from(profile) * 1_000; // 三类内容使用不同tick区间以发现串槽。
            for (stage, tick_offset) in [0_u64, 10, 30, 40].into_iter().enumerate() {
                let query_index =
                    compute_timestamp_index(profile, stage as u32).expect("三类内容Compute槽存在");
                write_u64(
                    &mut bytes,
                    resolve_plan
                        .timestamp_offset(query_index)
                        .expect("三类内容Compute查询必须在稀疏计划中"),
                    compute_base + tick_offset,
                ); // 写入当前内容完整四阶段边界。
            }
            for lod in 0..DRAW_LOD_COUNT {
                let measurement = profile * DRAW_LOD_COUNT + lod; // 草、麦、杂粮分别占统计零至三、四至七、十二至十五。
                let begin_query = DRAW_TIMESTAMP_BASE + measurement * 2; // 每档Draw使用相邻起止时间戳。
                let draw_base = 10_000_u64 + u64::from(measurement) * 100;
                write_u64(
                    &mut bytes,
                    resolve_plan
                        .timestamp_offset(begin_query)
                        .expect("三类内容Draw开始查询必须在稀疏计划中"),
                    draw_base,
                );
                write_u64(
                    &mut bytes,
                    resolve_plan
                        .timestamp_offset(begin_query + 1)
                        .expect("三类内容Draw结束查询必须在稀疏计划中"),
                    draw_base + u64::from(5 + profile + lod),
                ); // 每种内容与LOD产生独立正耗时。
                let statistics_offset = resolve_plan
                    .pipeline_statistics_offset(measurement)
                    .expect("三类内容统计查询必须在稀疏计划中");
                write_u64(
                    &mut bytes,
                    statistics_offset,
                    100_000 + u64::from(measurement),
                ); // 第一项写入可辨识顶点调用量。
                write_u64(
                    &mut bytes,
                    statistics_offset + 24,
                    200_000 + u64::from(measurement),
                ); // 第四项写入可辨识片元调用量。
            }
        }
        let stats = decode_gpu_grass_timing(&bytes, write_mask, 9, 1_000.0, true, true); // 使用三类内容各四档的真实稀疏掩码完成编码到解码闭环。
        assert_eq!(stats.sample_sequence, 9);
        for profile_index in [0_usize, 1, 3] {
            let profile = &stats.by_profile[profile_index]; // 逐一核对普通草、麦田与杂粮四级统计。
            assert!(profile.compute_total_ms > 0.0); // 三类内容Compute都必须跨段正确解码。
            assert!(profile.draw_total_ms > 0.0); // 三类内容近中远超远Draw都必须取得正耗时。
            assert!(
                !profile.vertex_shader_invocations_by_lod[..DRAW_LOD_COUNT as usize].contains(&0)
            ); // 十二组真实统计槽均回到正确内容和四级身份。
            assert!(
                profile.vertex_shader_invocations_by_lod[DRAW_LOD_COUNT as usize..]
                    .iter()
                    .all(|value| *value == 0)
            ); // 固定查询槽之外不伪造扩展档测量。
        }
        assert_eq!(
            stats.by_profile[2],
            GpuBladeVegetationTimingStats::default()
        ); // 时间戳空洞不得伪造芦苇profile数据。
    }

    #[test]
    fn incomplete_timestamp_pair_never_reads_stale_bytes() {
        let mut bytes = vec![0_u8; TIMING_RESOLVE_BUFFER_BYTES as usize]; // 构造可能残留旧查询值的staging布局。
        let wheat_compute_begin =
            compute_timestamp_index(1, COMPUTE_BEGIN_TIMESTAMP).expect("麦田开始槽存在"); // 麦田Compute起始槽。
        let wheat_draw_begin = DRAW_TIMESTAMP_BASE + DRAW_LOD_COUNT * 2; // profile一近景是第四组draw。
        let write_mask = (1_u64 << wheat_compute_begin) | (1_u64 << wheat_draw_begin); // 两组都只声明开始槽真实写入。
        let resolve_plan = build_query_resolve_plan(write_mask); // 两个孤立开始槽各自占一个对齐段。
        let compute_begin_offset = resolve_plan
            .timestamp_offset(wheat_compute_begin)
            .expect("麦田Compute开始必须存在解析偏移"); // 取得首个真实单查询段。
        write_u64(&mut bytes, compute_begin_offset, 10); // 写入麦田Compute开始当前值。
        write_u64(&mut bytes, compute_begin_offset + 8, 999_999); // 在相邻未解析字节模拟旧结束值残留。
        let draw_begin_offset = resolve_plan
            .timestamp_offset(wheat_draw_begin)
            .expect("麦田draw开始必须存在解析偏移"); // 取得第二个真实单查询段。
        write_u64(&mut bytes, draw_begin_offset, 20); // 写入麦田近景draw开始当前值。
        write_u64(&mut bytes, draw_begin_offset + 8, 888_888); // 在相邻未解析字节模拟旧draw结束值。
        let stats = decode_gpu_grass_timing(
            &bytes,     // 字节本身看似含麦田有效巨大耗时。
            write_mask, // 掩码声明两组都只有开始槽由本样本真实写入。
            1,          // 任意非零样本序号。
            1.0,        // tick周期不影响零结果。
            true,       // 时间戳能力存在。
            false,      // 本测试不启用管线统计。
        );
        assert_eq!(stats.by_profile[1].tile_cull_ms, 0.0); // 麦田缺少结束位时必须忽略解析缓冲残留。
        assert_eq!(stats.by_profile[1].draw_lod_ms[0], 0.0); // 麦田draw同样要求完整起止对。
        assert_eq!(stats.compute_total_ms, 0.0); // 总Compute同样要求完整起止对。
        assert_eq!(stats.draw_total_ms, 0.0); // 聚合绘制不得吸收任何旧结束值。
    }
}
