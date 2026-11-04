//! GPU查询写入掩码到无空洞、对齐解析区间的纯值规划。
//! Vulkan会等待解析范围内每个查询，因此任何未写槽都必须在录制命令前被切开。

use super::super::{
    PIPELINE_STATISTICS_MASK_SHIFT, PIPELINE_STATISTICS_QUERY_COUNT, TIMESTAMP_QUERY_COUNT,
}; // 解析计划与固定查询ABI使用同一槽位数量和统一掩码位段。

pub(super) const QUERY_RESOLVE_ALIGNMENT: u64 = 256; // WebGPU要求每段查询解析目标偏移按二百五十六字节对齐。
pub(super) const TIMESTAMP_BYTES_PER_QUERY: u64 = 8; // 每个时间戳查询只返回一个u64设备tick。
pub(super) const PIPELINE_STATISTICS_BYTES_PER_QUERY: u64 = 40; // 五个u64标准统计值固定占四十字节。
pub(super) const MAX_TIMESTAMP_QUERY_RUNS: usize = (TIMESTAMP_QUERY_COUNT as usize).div_ceil(2); // 四十位最坏交替写入时形成二十段连续区间。
pub(super) const MAX_PIPELINE_STATISTICS_QUERY_RUNS: usize =
    (PIPELINE_STATISTICS_QUERY_COUNT as usize).div_ceil(2); // 十二位最坏交替写入时形成六段连续区间。
pub(super) const TIMING_RESOLVE_BUFFER_BYTES: u64 =
    (MAX_TIMESTAMP_QUERY_RUNS + MAX_PIPELINE_STATISTICS_QUERY_RUNS) as u64
        * QUERY_RESOLVE_ALIGNMENT; // 最坏二十六个单查询段各占一个对齐块，共固定六千六百五十六字节。

/// 一段只包含本帧真实写入查询的连续解析范围。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct QueryResolveRun {
    pub(super) start_query: u32,        // QuerySet中的首个已写查询编号。
    pub(super) end_query: u32,          // QuerySet中的半开结束编号，区间内部不存在未写空洞。
    pub(super) destination_offset: u64, // 当前连续段在解析缓冲中的二百五十六字节对齐起点。
    bytes_per_query: u64,               // 时间戳为八字节，五项管线统计为四十字节。
}

/// 当前采样帧的固定容量稀疏查询解析计划。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryResolvePlan {
    timestamp_runs: [QueryResolveRun; MAX_TIMESTAMP_QUERY_RUNS], // 时间戳真实写入连续段，禁止把槽位空洞交给Vulkan等待。
    timestamp_run_count: usize, // 时间戳数组前多少项属于本帧有效计划。
    pipeline_statistics_runs: [QueryResolveRun; MAX_PIPELINE_STATISTICS_QUERY_RUNS], // 管线统计同样只解析真实写入连续段。
    pipeline_statistics_run_count: usize, // 管线统计数组前多少项属于本帧有效计划。
    pub(super) resolved_bytes: u64,       // 当前计划实际需要复制和映射的二百五十六字节对齐长度。
}

/// 把字节末端向上对齐到WebGPU查询解析要求。
fn align_query_resolve_bytes(bytes: u64) -> u64 {
    bytes.div_ceil(QUERY_RESOLVE_ALIGNMENT) * QUERY_RESOLVE_ALIGNMENT // 所有输入都远小于u64上限，向上取整不会溢出。
}

/// 从统一写入掩码提取连续真值段，并为每段分配独立对齐目标区域。
fn collect_query_resolve_runs<const RUN_CAPACITY: usize>(
    write_mask: u64,                            // 当前渲染帧真实写入的统一查询位。
    mask_shift: u32,                            // 时间戳从零位、统计从第四十八位开始。
    query_count: u32,                           // 当前QuerySet固定可用槽数。
    bytes_per_query: u64,                       // 决定当前连续段解析结果实际字节数。
    next_destination_offset: &mut u64,          // 两类查询共享同一解析缓冲的下一个对齐起点。
    runs: &mut [QueryResolveRun; RUN_CAPACITY], // 固定容量数组避免每四帧为诊断动态分配。
) -> usize {
    let mut query = 0_u32; // 从当前QuerySet零号槽开始寻找真实写入位。
    let mut run_count = 0_usize; // 记录已经写入固定数组的连续段数量。
    while query < query_count {
        if write_mask & (1_u64 << (mask_shift + query)) == 0 {
            query += 1; // 未写槽直接跨过，绝不把它包含进Vulkan带WAIT的解析命令。
            continue; // 从下一查询位继续寻找连续段起点。
        }
        let start_query = query; // 当前真值位成为本段半开区间起点。
        query += 1; // 至少消费当前一个真实写入查询。
        while query < query_count && write_mask & (1_u64 << (mask_shift + query)) != 0 {
            query += 1; // 连续真值保持在同一次query resolve中，减少命令和时间戳归一化次数。
        }
        let end_query = query; // 首个零位或QuerySet末端成为本段半开终点。
        let run = runs
            .get_mut(run_count)
            .expect("固定查询段容量必须覆盖最坏交替写入掩码"); // 数学上最多ceil(N/2)段，越界代表常量契约被改坏。
        *run = QueryResolveRun {
            start_query,                                  // 解析只从首个真实写入槽开始。
            end_query,                                    // 解析在首个未写槽之前结束。
            destination_offset: *next_destination_offset, // 每段写入独立二百五十六字节对齐区域。
            bytes_per_query,                              // 解码时按查询类型恢复段内相对偏移。
        }; // 完成当前无空洞解析段登记。
        let run_bytes = u64::from(end_query - start_query) * bytes_per_query; // 计算该段真实返回字节数。
        *next_destination_offset = align_query_resolve_bytes(*next_destination_offset + run_bytes); // 下一段从当前结果末端的下一个对齐边界开始。
        run_count += 1; // 固定数组下一项接收后续连续段。
    }
    run_count // 返回有效前缀长度，调用方不得遍历未使用默认项。
}

/// 根据本帧真实写入位构造不会解析任何未写查询的固定容量计划。
pub(super) fn build_query_resolve_plan(write_mask: u64) -> QueryResolvePlan {
    let mut timestamp_runs = [QueryResolveRun::default(); MAX_TIMESTAMP_QUERY_RUNS]; // 预留时间戳最坏二十段。
    let mut pipeline_statistics_runs =
        [QueryResolveRun::default(); MAX_PIPELINE_STATISTICS_QUERY_RUNS]; // 预留统计最坏六段。
    let mut next_destination_offset = 0_u64; // 第一段可以从解析缓冲零偏移开始。
    let timestamp_run_count = collect_query_resolve_runs(
        write_mask,                   // 使用同一帧查询写入掩码。
        0,                            // 时间戳占统一掩码零至三十九位。
        TIMESTAMP_QUERY_COUNT,        // 固定四十个时间戳槽。
        TIMESTAMP_BYTES_PER_QUERY,    // 每个时间戳返回一个u64。
        &mut next_destination_offset, // 统计段会接在时间戳实际占用区域之后。
        &mut timestamp_runs,          // 写入固定时间戳段数组。
    ); // 完成时间戳空洞剔除与对齐布局。
    let pipeline_statistics_run_count = collect_query_resolve_runs(
        write_mask,                          // 继续使用同一帧查询写入掩码。
        PIPELINE_STATISTICS_MASK_SHIFT,      // 管线统计从第四十八位开始。
        PIPELINE_STATISTICS_QUERY_COUNT,     // 固定十二个内容LOD统计槽。
        PIPELINE_STATISTICS_BYTES_PER_QUERY, // 五个u64统计值共四十字节。
        &mut next_destination_offset,        // 从时间戳最后一个对齐段之后继续布局。
        &mut pipeline_statistics_runs,       // 写入固定统计段数组。
    ); // 完成统计空洞剔除与对齐布局。
    debug_assert!(
        next_destination_offset <= TIMING_RESOLVE_BUFFER_BYTES,
        "稀疏查询计划不得越过固定最坏容量"
    ); // 开发构建立即暴露查询数量与容量常量失配。
    QueryResolvePlan {
        timestamp_runs,                          // 保存时间戳真实连续段。
        timestamp_run_count,                     // 限制时间戳有效数组前缀。
        pipeline_statistics_runs,                // 保存统计真实连续段。
        pipeline_statistics_run_count,           // 限制统计有效数组前缀。
        resolved_bytes: next_destination_offset, // 只复制并映射本次计划实际使用的对齐前缀。
    } // 返回节点录制与异步解码必须共同复用的确定性计划。
}

impl QueryResolvePlan {
    /// 返回当前帧真实时间戳解析段。
    pub(super) fn timestamp_runs(&self) -> &[QueryResolveRun] {
        &self.timestamp_runs[..self.timestamp_run_count] // 未使用默认数组尾部绝不生成GPU命令。
    }

    /// 返回当前帧真实管线统计解析段。
    pub(super) fn pipeline_statistics_runs(&self) -> &[QueryResolveRun] {
        &self.pipeline_statistics_runs[..self.pipeline_statistics_run_count] // 未写统计槽不会进入Vulkan等待范围。
    }

    /// 返回指定时间戳查询在本帧稀疏解析布局中的字节偏移。
    pub(super) fn timestamp_offset(&self, query_index: u32) -> Option<u64> {
        resolved_query_offset(self.timestamp_runs(), query_index) // 只有真实写入段内查询才有可读结果。
    }

    /// 返回指定管线统计查询在本帧稀疏解析布局中的字节偏移。
    pub(super) fn pipeline_statistics_offset(&self, query_index: u32) -> Option<u64> {
        resolved_query_offset(self.pipeline_statistics_runs(), query_index) // 每项偏移指向五个u64统计值首字节。
    }
}

/// 在少量固定连续段中查找一个查询的解析结果偏移。
fn resolved_query_offset(runs: &[QueryResolveRun], query_index: u32) -> Option<u64> {
    runs.iter().find_map(|run| {
        (query_index >= run.start_query && query_index < run.end_query).then(|| {
            run.destination_offset + u64::from(query_index - run.start_query) * run.bytes_per_query // 段内保持QuerySet原始顺序紧凑排列。
        })
    }) // 未写查询不会出现在任何段中，调用方必须把它当作无结果。
}

#[cfg(test)]
mod tests {
    use super::*; // 验证稀疏掩码切段、最坏容量和固定查询ABI边界。

    #[test]
    fn sparse_grass_and_wheat_queries_never_resolve_unwritten_timestamp_hole() {
        let write_mask = ((1_u64 << 8) - 1) // 普通草与麦田Compute真实写入时间戳零至七。
            | (((1_u64 << 12) - 1) << 16) // 两类内容近中远draw真实写入时间戳十六至二十七。
            | (((1_u64 << 6) - 1) << PIPELINE_STATISTICS_MASK_SHIFT); // 两类内容近中远统计真实写入零至五。
        let plan = build_query_resolve_plan(write_mask); // 构造六窗正常帧的真实稀疏查询布局。
        assert_eq!(
            plan.timestamp_runs(),
            &[
                QueryResolveRun {
                    start_query: 0,                             // 两类Compute形成首段连续查询。
                    end_query: 8,                               // 八号槽开始属于尚未接入内容。
                    destination_offset: 0,                      // 首段从解析缓冲零偏移开始。
                    bytes_per_query: TIMESTAMP_BYTES_PER_QUERY, // 时间戳每项八字节。
                },
                QueryResolveRun {
                    start_query: 16,                            // Draw时间戳绕过八至十五号空洞。
                    end_query: 28,                              // 普通草与麦田三档draw共十二项。
                    destination_offset: 256,                    // 第二段必须从独立对齐区域开始。
                    bytes_per_query: TIMESTAMP_BYTES_PER_QUERY, // 时间戳每项仍为八字节。
                },
            ]
        ); // Vulkan绝不能再收到包含八至十五号未写查询的零至二十八整段。
        assert_eq!(
            plan.pipeline_statistics_runs(),
            &[QueryResolveRun {
                start_query: 0,                                       // 普通草与麦田统计编号连续。
                end_query: 6,            // 只解析两类内容的六个LOD统计。
                destination_offset: 512, // 接在两个时间戳对齐段之后。
                bytes_per_query: PIPELINE_STATISTICS_BYTES_PER_QUERY, // 每项保留五个u64。
            }]
        ); // 管线统计仍保持一次连续解析。
        assert_eq!(plan.timestamp_offset(8), None); // 首个未写Compute槽不得拥有解析结果偏移。
        assert_eq!(plan.timestamp_offset(15), None); // 整个危险空洞末端同样必须缺席。
        assert_eq!(plan.resolved_bytes, 768); // 正常帧只复制三个二百五十六字节对齐块。
    }

    #[test]
    fn alternating_query_mask_fits_fixed_worst_case_capacity() {
        let timestamp_mask = (0..TIMESTAMP_QUERY_COUNT)
            .step_by(2)
            .fold(0_u64, |mask, query| mask | (1_u64 << query)); // 四十项交替写入制造最多二十个时间戳段。
        let statistics_mask =
            (0..PIPELINE_STATISTICS_QUERY_COUNT)
                .step_by(2)
                .fold(0_u64, |mask, query| {
                    mask | (1_u64 << (PIPELINE_STATISTICS_MASK_SHIFT + query)) // 十二项交替写入制造最多六个统计段。
                });
        let plan = build_query_resolve_plan(timestamp_mask | statistics_mask); // 构造两类查询共同最坏稀疏布局。
        assert_eq!(plan.timestamp_runs().len(), MAX_TIMESTAMP_QUERY_RUNS); // 固定时间戳数组容量恰好覆盖二十段。
        assert_eq!(
            plan.pipeline_statistics_runs().len(),
            MAX_PIPELINE_STATISTICS_QUERY_RUNS
        ); // 固定统计数组容量恰好覆盖六段。
        assert_eq!(plan.resolved_bytes, TIMING_RESOLVE_BUFFER_BYTES); // 最坏布局恰好占满六千六百五十六字节。
        assert!(
            plan.timestamp_runs()
                .iter()
                .chain(plan.pipeline_statistics_runs())
                .all(|run| run.destination_offset % QUERY_RESOLVE_ALIGNMENT == 0)
        ); // 每一段目标偏移都满足WebGPU强制对齐。
    }

    #[test]
    fn highest_sparse_timestamp_and_statistics_slots_remain_resolvable() {
        let write_mask = (1_u64 << (TIMESTAMP_QUERY_COUNT - 1))
            | (1_u64 << (PIPELINE_STATISTICS_MASK_SHIFT + PIPELINE_STATISTICS_QUERY_COUNT - 1)); // 只写最后一种内容的最后时间戳与远景统计。
        let plan = build_query_resolve_plan(write_mask); // 验证稀疏计划不会再依赖从零开始的连续前缀。
        assert_eq!(
            plan.timestamp_runs(),
            &[QueryResolveRun {
                start_query: TIMESTAMP_QUERY_COUNT - 1, // 只解析第三十九号时间戳。
                end_query: TIMESTAMP_QUERY_COUNT,       // 半开终点仍严格位于QuerySet容量内。
                destination_offset: 0,                  // 首个真实段从零偏移开始。
                bytes_per_query: TIMESTAMP_BYTES_PER_QUERY, // 单时间戳返回八字节。
            }]
        ); // 最高时间戳无需连带等待零至三十八号未写查询。
        assert_eq!(
            plan.pipeline_statistics_runs(),
            &[QueryResolveRun {
                start_query: PIPELINE_STATISTICS_QUERY_COUNT - 1, // 只解析第十一号统计。
                end_query: PIPELINE_STATISTICS_QUERY_COUNT,       // 半开终点等于固定十二槽容量。
                destination_offset: QUERY_RESOLVE_ALIGNMENT,      // 第二类查询从下一对齐块开始。
                bytes_per_query: PIPELINE_STATISTICS_BYTES_PER_QUERY, // 单统计返回五个u64。
            }]
        ); // 最高统计槽同样无需解析任何前置空洞。
        assert_eq!(plan.timestamp_offset(TIMESTAMP_QUERY_COUNT - 1), Some(0)); // 第三十九号时间戳映射到首段首字节。
        assert_eq!(
            plan.pipeline_statistics_offset(PIPELINE_STATISTICS_QUERY_COUNT - 1),
            Some(QUERY_RESOLVE_ALIGNMENT)
        ); // 第十一号统计映射到第二个对齐块首字节。
    }
}
