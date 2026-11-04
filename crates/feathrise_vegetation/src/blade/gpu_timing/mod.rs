//! GPU草地分阶段时间戳与近中远管线统计功能域。
//! 所有查询都使用固定槽位和异步三缓冲，诊断不可用或繁忙时不允许阻塞正式渲染。

#[cfg(feature = "diagnostics")]
mod readback; // 创建查询集、解析结果并把最近样本发布到主世界F3。

use std::sync::atomic::Ordering; // 跨Compute节点与RenderCommand无锁声明本帧已经写入的固定查询槽。

use bevy::render::render_phase::TrackedRenderPass; // 在现有AlphaMask通道内包围单个近中远间接draw。
use bevy::render::render_resource::ComputePass; // 在同一个草地Compute通道内记录四个阶段边界。

use crate::{
    resources::GpuBladeTimingQueries, // 访问设备能力允许时创建的时间戳与管线统计查询集。
    types::GPU_GRASS_PROFILE_CAPACITY, // 普通草、麦田、芦苇和保留内容固定占四个诊断槽。
};

#[cfg(feature = "diagnostics")]
pub(crate) use readback::install_gpu_grass_timing; // 由可选诊断插件安装查询、回读与跨世界发布链路。

pub(crate) const COMPUTE_BEGIN_TIMESTAMP: u32 = 0; // 每种内容四槽组内的粗瓦片剔除开始偏移。
pub(crate) const TILE_CULL_END_TIMESTAMP: u32 = 1; // 每种内容四槽组内的瓦片阶段结束偏移。
pub(crate) const CANDIDATE_CLASSIFY_END_TIMESTAMP: u32 = 2; // 每种内容四槽组内的候选分类结束偏移。
pub(crate) const COMPUTE_FINALIZE_END_TIMESTAMP: u32 = 3; // 每种内容四槽组内的间接参数收尾结束偏移。
pub(crate) const COMPUTE_TIMESTAMP_STRIDE: u32 = 4; // 普通草、麦田等每种内容固定占四个Compute边界。
pub(crate) const DRAW_LOD_COUNT: u32 = 4; // 每种内容固定记录近、中、远、超远四档绘制。
pub(crate) const COMPUTE_TIMESTAMP_COUNT: u32 =
    GPU_GRASS_PROFILE_CAPACITY as u32 * COMPUTE_TIMESTAMP_STRIDE; // 四种内容共占时间戳零至十五。
pub(crate) const DRAW_TIMESTAMP_BASE: u32 = COMPUTE_TIMESTAMP_COUNT; // 十六组内容LOD绘制从时间戳十六开始。
pub(crate) const PIPELINE_STATISTICS_QUERY_COUNT: u32 =
    GPU_GRASS_PROFILE_CAPACITY as u32 * DRAW_LOD_COUNT; // 四种内容的四级LOD共十六个统计查询。
pub(crate) const TIMESTAMP_QUERY_COUNT: u32 =
    DRAW_TIMESTAMP_BASE + PIPELINE_STATISTICS_QUERY_COUNT * 2; // 十六个Compute边界加十六组绘制起止共四十八项。
pub(crate) const PIPELINE_STATISTICS_MASK_SHIFT: u32 = 48; // 统计位使用四十八至六十三位，与零至四十七位时间戳完全分离。

/// 把内容编号和四阶段组内偏移映射到固定Compute时间戳槽。
pub(crate) fn compute_timestamp_index(vegetation_profile: u32, stage_offset: u32) -> Option<u32> {
    if vegetation_profile >= GPU_GRASS_PROFILE_CAPACITY as u32
        || stage_offset >= COMPUTE_TIMESTAMP_STRIDE
    {
        return None; // 损坏内容编号或阶段偏移不允许移位越界或污染另一类内容的查询槽。
    }
    Some(vegetation_profile * COMPUTE_TIMESTAMP_STRIDE + stage_offset) // 普通草为零至三，麦田为四至七。
}

/// 把内容编号和近中远LOD映射到十二个稳定绘制测量槽。
fn draw_measurement_index(vegetation_profile: u32, lod_index: u32) -> Option<u32> {
    if vegetation_profile >= GPU_GRASS_PROFILE_CAPACITY as u32 || lod_index >= DRAW_LOD_COUNT {
        return None; // 未定义内容或几何LOD完全禁用诊断，正式绘制仍可继续。
    }
    Some(vegetation_profile * DRAW_LOD_COUNT + lod_index) // 每种内容连续占近、中、远、超远四个槽。
}

/// 单次近、中或远草地draw真正启用的查询范围。
pub(crate) struct GpuBladeDrawTimingScope {
    measurement_index: u32,   // 内容编号乘四加LOD，唯一定位时间戳对与管线统计槽。
    timestamp_recorded: bool, // 当前后端支持且本帧首次测量该LOD时才需要写结束时间戳。
    pipeline_statistics_recorded: bool, // 当前后端支持且本帧首次测量该LOD时才需要结束统计查询。
}

/// 在现有Compute通道内写入一个固定阶段边界时间戳。
pub(crate) fn record_compute_timestamp(
    queries: &GpuBladeTimingQueries, // 设备查询能力和本帧无锁写入掩码。
    pass: &mut ComputePass<'_>,      // 当前草地分类、压缩和间接参数生成通道。
    query_index: u32,                // 四类内容各四个固定阶段边界，禁止运行时动态增长。
) {
    let Some(query_set) = queries.timestamp_query_set.as_ref() else {
        return; // 不支持通道内时间戳的后端完全跳过，不执行CPU替代计时。
    };
    if query_index >= TIMESTAMP_QUERY_COUNT {
        return; // 非法内容编号生成的槽位不得触发六十四位移位或访问查询集外部。
    }
    let query_bit = 1_u64 << query_index; // 每个固定槽用一个原子位声明本帧唯一写入。
    if queries.write_mask.fetch_or(query_bit, Ordering::Relaxed) & query_bit != 0 {
        return; // 根图异常重复执行时只保留第一次完整测量，避免同槽覆盖产生混合样本。
    }
    pass.write_timestamp(query_set, query_index); // GPU命令流到达此处时写入设备时间戳tick。
}

/// 在一个草地LOD间接draw前开始时间戳和管线统计查询。
pub(crate) fn begin_draw_measurement(
    queries: &GpuBladeTimingQueries, // 固定查询集由所有草圈和玩法视图共享。
    pass: &mut TrackedRenderPass<'_>, // 已绑定草网格、紧凑实例和索引缓冲的AlphaMask通道。
    vegetation_profile: u32,         // 零普通草、一麦田等固定内容编号。
    lod_index: u32,                  // 草块uniform中的零至二几何LOD编号。
) -> GpuBladeDrawTimingScope {
    let Some(measurement_index) = draw_measurement_index(vegetation_profile, lod_index) else {
        return GpuBladeDrawTimingScope {
            measurement_index: 0,                // 禁用作用域不会读取该占位编号。
            timestamp_recorded: false,           // 未定义额外LOD不占用固定时间戳槽。
            pipeline_statistics_recorded: false, // 未定义额外LOD不占用固定统计槽。
        }; // 返回完全禁用的局部作用域。
    };
    let begin_timestamp_index = DRAW_TIMESTAMP_BASE + measurement_index * 2; // 普通草近景从十六开始，麦田近景从二十二开始。
    let begin_timestamp_bit = 1_u64 << begin_timestamp_index; // 起始时间戳独占一个低位标记。
    let timestamp_recorded = queries.timestamp_query_set.is_some()
        && queries
            .write_mask
            .fetch_or(begin_timestamp_bit, Ordering::Relaxed)
            & begin_timestamp_bit
            == 0; // 多视图或重复LOD只测量首个实际draw，避免固定槽被后续覆盖。
    let statistics_bit = 1_u64 << (PIPELINE_STATISTICS_MASK_SHIFT + measurement_index); // 十二档统计使用第四十八至五十九位。
    let pipeline_statistics_recorded = queries.pipeline_statistics_query_set.is_some()
        && queries
            .write_mask
            .fetch_or(statistics_bit, Ordering::Relaxed)
            & statistics_bit
            == 0; // 同一帧每个LOD只允许一个管线统计查询处于活动状态。
    let raw_pass = pass.wgpu_pass(); // 直接访问wgpu查询API并让Bevy下一绘制项重新确认绑定状态。
    if timestamp_recorded {
        raw_pass.write_timestamp(
            queries
                .timestamp_query_set
                .as_ref()
                .expect("已检查时间戳查询集"), // 布尔值只会在查询集存在时为真。
            begin_timestamp_index, // 记录该LOD间接draw的GPU开始边界。
        ); // 完成绘制开始时间戳写入。
    }
    if pipeline_statistics_recorded {
        raw_pass.begin_pipeline_statistics_query(
            queries
                .pipeline_statistics_query_set
                .as_ref()
                .expect("已检查管线统计查询集"), // 布尔值只会在统计查询集存在时为真。
            measurement_index, // 查询编号同时编码内容profile与近中远LOD。
        ); // 后续当前LOD八个径向深度桶间接命令的顶点与片元调用量开始累计。
    }
    GpuBladeDrawTimingScope {
        measurement_index,            // 结束函数复用同一内容LOD测量编号计算结束槽。
        timestamp_recorded,           // 仅对真实开始的查询执行配对结束。
        pipeline_statistics_recorded, // 仅对真实开始的统计执行配对结束。
    } // 返回必须在draw之后立即结束的轻量作用域。
}

/// 在一个草地LOD间接draw后结束管线统计并写入结束时间戳。
pub(crate) fn end_draw_measurement(
    queries: &GpuBladeTimingQueries, // 与开始函数完全相同的持久查询资源。
    pass: &mut TrackedRenderPass<'_>, // 刚执行完当前草地间接draw的AlphaMask通道。
    scope: GpuBladeDrawTimingScope,  // 开始函数声明的实际启用查询组合。
) {
    if !scope.timestamp_recorded && !scope.pipeline_statistics_recorded {
        return; // 不支持或重复LOD时不触碰原始通道，也不重置Bevy跟踪状态。
    }
    let raw_pass = pass.wgpu_pass(); // 取得同一wgpu通道以配对结束查询。
    if scope.pipeline_statistics_recorded {
        raw_pass.end_pipeline_statistics_query(); // 精确停止当前LOD全部八个深度桶间接命令的顶点与片元调用累计。
    }
    if scope.timestamp_recorded {
        let end_timestamp_index = DRAW_TIMESTAMP_BASE + scope.measurement_index * 2 + 1; // 十二组内容LOD各使用连续起止槽。
        raw_pass.write_timestamp(
            queries
                .timestamp_query_set
                .as_ref()
                .expect("已开始时间戳查询"), // 开始成功保证查询集整个渲染帧存活。
            end_timestamp_index, // 记录该LOD间接draw完成后的GPU时间戳。
        ); // 完成绘制结束时间戳写入。
        queries
            .write_mask
            .fetch_or(1_u64 << end_timestamp_index, Ordering::Relaxed); // 结束命令写入后才允许回读节点解析这一对时间戳。
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet; // 集合长度用于证明全部内容槽位彼此唯一。

    use super::*; // 验证固定查询ABI不会让草和麦覆盖彼此数据。

    #[test]
    fn profile_query_slots_are_unique_and_fit_the_u64_mask() {
        let mut timestamp_slots = BTreeSet::new(); // 收集四类Compute边界和十二组draw起止槽。
        let mut statistics_slots = BTreeSet::new(); // 收集四类近中远管线统计槽。
        for profile in 0..GPU_GRASS_PROFILE_CAPACITY as u32 {
            for stage in 0..COMPUTE_TIMESTAMP_STRIDE {
                timestamp_slots.insert(
                    compute_timestamp_index(profile, stage).expect("合法内容阶段必须有查询槽"),
                ); // 每种内容固定取得四个不同Compute时间戳。
            }
            for lod in 0..DRAW_LOD_COUNT {
                let measurement =
                    draw_measurement_index(profile, lod).expect("合法内容LOD必须有测量槽");
                timestamp_slots.insert(DRAW_TIMESTAMP_BASE + measurement * 2); // 当前内容LOD绘制开始槽。
                timestamp_slots.insert(DRAW_TIMESTAMP_BASE + measurement * 2 + 1); // 当前内容LOD绘制结束槽。
                statistics_slots.insert(measurement); // 同一个测量编号直接作为统计查询索引。
            }
        }
        assert_eq!(timestamp_slots.len(), TIMESTAMP_QUERY_COUNT as usize); // 四十个时间戳槽无重复。
        assert_eq!(
            timestamp_slots.last().copied(),
            Some(TIMESTAMP_QUERY_COUNT - 1)
        ); // 最高时间戳恰为三十九。
        assert_eq!(
            statistics_slots.len(),
            PIPELINE_STATISTICS_QUERY_COUNT as usize
        ); // 十二个管线统计槽无重复。
        const {
            assert!(PIPELINE_STATISTICS_MASK_SHIFT + PIPELINE_STATISTICS_QUERY_COUNT <= u64::BITS);
        } // 统一写掩码最高使用第五十九位，不发生移位溢出。
    }
}
