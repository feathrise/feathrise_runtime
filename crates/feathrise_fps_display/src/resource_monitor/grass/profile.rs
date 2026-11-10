//! 单类GPU叶片植被的筛选、几何与性能分项格式化。
//! 普通草和麦田共享函数结构，但始终由调用方传入一个稳定profile身份，禁止重新聚合成一页。

use feathrise_environment::vegetation::blade::{
    GpuBladeGpuReadbackStats, GpuBladeGpuTimingStats, GpuBladeProfileSlot,
    GpuBladeVegetationReadbackStats, GpuBladeVegetationTimingStats,
}; // 读取单类候选、几何、耗时及查询能力状态。

use super::percent; // 复用父模块统一百分比保护。

/// 格式化当前分页实际存在内容的独立GPU诊断段。
pub(super) fn format_content_gpu_report(
    content_kinds: &[GpuBladeProfileSlot], // 分页模式正常只有一个稳定profile。
    content_label: &'static str, // 页面身份可把互斥场景复用的零号profile显示为普通草或湿地草。
    lod_count: usize,            // 局部LOD规格决定实际显示的档数，不再固定截断到第四档。
    gpu_stats: &GpuBladeGpuReadbackStats, // 四类内容的真实GPU候选、存活与几何提交量。
    gpu_timing: &GpuBladeGpuTimingStats, // 四类内容的Compute、Draw和着色器调用量。
) -> String {
    if content_kinds.is_empty() {
        return "草内容 无".to_string(); // 当前场景没有该页内容时不展开伪诊断。
    } // 完成空内容快速返回。
    let lod_count = lod_count.clamp(4, gpu_stats.by_profile[0].rendered_instances_by_lod.len()); // 旧场景保留四列，扩展档只访问已分配诊断槽。
    let lod_labels = if lod_count == 4 {
        "近/中/远/超远".to_string()
    } else {
        (0..lod_count)
            .map(|level| format!("LOD{level}"))
            .collect::<Vec<_>>()
            .join("/")
    };
    content_kinds
        .iter()
        .flat_map(|kind| {
            let profile_index = kind.index() as usize; // repr(u32)稳定编号与四项诊断数组一一对应。
            let profile_stats = &gpu_stats.by_profile[profile_index]; // 读取当前内容真实回读分项。
            let profile_timing = &gpu_timing.by_profile[profile_index]; // 读取当前内容真实查询分项。
            [
                format_profile_filter_report(content_label, profile_stats), // 候选、瓦片、筛选和实绘分项。
                format_profile_geometry_report(
                    content_label,
                    profile_stats,
                    lod_count,
                    &lod_labels,
                ), // 全部有效档的实例及真实网格提交量。
                format_profile_performance_report(
                    content_label,  // 当前分页用户可读内容身份。
                    profile_stats,  // 当前内容实例与几何回读。
                    profile_timing, // 当前内容独立GPU查询分项。
                    gpu_timing,     // 共享查询能力与样本序号。
                    lod_count,
                    &lod_labels,
                ), // Compute、Draw和调用量共享一行便于横向比较。
            ] // 完成当前profile固定三行。
        })
        .collect::<Vec<_>>()
        .join("\n") // 分页模式只产生当前内容三行。
} // 完成单类GPU诊断段格式化。

/// 格式化单类叶片内容的候选、瓦片和逐阶段存活量。
fn format_profile_filter_report(
    label: &'static str,                     // 决定普通草、湿地草、麦田等中文行首。
    stats: &GpuBladeVegetationReadbackStats, // 提供当前内容真实GPU回读分项。
) -> String {
    let active_tile_percent = percent(stats.active_tiles, stats.total_tiles); // 当前内容粗判后的活跃瓦片率。
    let scan_percent = percent(stats.scanned_candidates, stats.candidate_count); // 当前内容实际启动细分类的候选率。
    let compacted_percent = percent(stats.rendered_instances, stats.candidate_count); // 当前内容最终实绘占原始候选比例。
    format!(
        "{label}筛选 窗 {} | 候选 {} | 瓦片 {}/{} ({active_tile_percent:.1}%) | 实扫 {} ({scan_percent:.1}%) | LOD/常驻/激活 {} 密 {} 高程 {} | 最终 {} ({compacted_percent:.2}%) | 容量 {} 溢出 {}",
        stats.sampled_windows,    // 当前内容实际回读窗口数。
        stats.candidate_count,    // 当前内容原始候选总数。
        stats.active_tiles,       // 当前内容通过粗判的活跃瓦片数。
        stats.total_tiles,        // 当前内容全部候选瓦片数。
        stats.scanned_candidates, // 当前内容真正启动逐株分类的候选数。
        stats.distance_survivors, // 当前内容通过距离、视锥和层级LOD的候选数。
        stats.density_survivors,  // 当前内容继续通过独立密度图概率的候选数。
        stats.surface_survivors,  // 当前内容继续获得合法地表高度的候选数。
        stats.rendered_instances, // 当前内容最终由间接绘制消费的实例数。
        stats.compact_capacity,   // 当前内容紧凑实例缓冲容量。
        stats.overflow_instances, // 当前内容因容量不足被截断的实例数。
    ) // 返回当前内容筛选报告。
} // 完成筛选分项格式化。

/// 格式化单类叶片内容全部有效LOD的实例和真实网格提交量。
fn format_profile_geometry_report(
    label: &'static str,                     // 决定普通草、湿地草、麦田等中文行首。
    stats: &GpuBladeVegetationReadbackStats, // 网格预算已在GPU状态中按窗口记录。
    lod_count: usize,
    lod_labels: &str,
) -> String {
    let total_vertices = stats
        .submitted_vertices_by_lod
        .into_iter()
        .fold(0_u64, u64::saturating_add); // 全部档的真实顶点提交量做饱和汇总。
    let total_indices = stats
        .submitted_indices_by_lod
        .into_iter()
        .fold(0_u64, u64::saturating_add); // 全部档的真实索引提交量做饱和汇总。
    format!(
        "{label}几何 实例{lod_labels} {} | 顶点 {total_vertices} [{}] | 索引 {total_indices} [{}]",
        format_lod_values(&stats.rendered_instances_by_lod[..lod_count]),
        format_lod_values(&stats.submitted_vertices_by_lod[..lod_count]),
        format_lod_values(&stats.submitted_indices_by_lod[..lod_count]),
    ) // 返回当前内容几何报告。
} // 完成几何分项格式化。

/// 格式化单类叶片内容的Compute、Draw和着色器调用量。
fn format_profile_performance_report(
    label: &'static str,                     // 决定普通草、湿地草、麦田等中文行首。
    stats: &GpuBladeVegetationReadbackStats, // 以当前内容实绘量归一化片元压力。
    profile: &GpuBladeVegetationTimingStats, // 当前内容独立时间戳与管线统计结果。
    timing: &GpuBladeGpuTimingStats,         // 提供本次查询的能力标记与样本序号。
    lod_count: usize,
    lod_labels: &str,
) -> String {
    let timing_report = if !timing.timestamp_supported {
        "GPU耗时 不可用".to_string() // 当前后端不支持时间戳时不以CPU数字冒充。
    } else if timing.sample_sequence == 0 {
        "GPU耗时 等待样本".to_string() // 查询尚未完成首份异步映射时不输出伪零。
    } else {
        let draw_lod_report = profile.draw_lod_ms[..lod_count]
            .iter()
            .map(|elapsed| format!("{elapsed:.3}"))
            .collect::<Vec<_>>()
            .join("/");
        format!(
            "GPU耗时 计算 {:.3}ms [瓦片 {:.3} 分类 {:.3} 收尾 {:.3}] | 绘制 {:.3}ms [{lod_labels} {draw_lod_report}]",
            profile.compute_total_ms,      // 当前内容Compute总耗时。
            profile.tile_cull_ms,          // 当前内容粗瓦片判断耗时。
            profile.candidate_classify_ms, // 当前内容逐株分类与压缩耗时。
            profile.compute_finalize_ms,   // 当前内容实例钳制和间接参数收尾耗时。
            profile.draw_total_ms,         // 当前内容全部LOD的Draw总耗时。
        ) // 完成当前内容GPU耗时文本。
    }; // 完成当前内容时间戳能力分支。
    let invocation_report = if !timing.pipeline_statistics_supported {
        "着色器调用 不可用".to_string() // 当前后端缺少管线统计能力时保持明确状态。
    } else if timing.sample_sequence == 0 {
        "着色器调用 等待样本".to_string() // 首份查询未完成时不把全零解释为没有调用。
    } else {
        let total_fragments = profile
            .fragment_shader_invocations_by_lod
            .iter()
            .copied()
            .fold(0_u64, u64::saturating_add); // 汇总当前内容全部LOD的片元调用量。
        let fragments_per_instance = if stats.rendered_instances > 0 {
            total_fragments as f64 / stats.rendered_instances as f64 // 估算当前内容每实绘实例的平均片元压力。
        } else {
            0.0 // 当前内容没有实绘实例时保护除零。
        }; // 完成当前内容片元压力归一化。
        format!(
            "着色器调用 顶点{lod_labels} {} | 片元 {} | 片元/实绘 {fragments_per_instance:.1}",
            format_lod_values(&profile.vertex_shader_invocations_by_lod[..lod_count]),
            format_lod_values(&profile.fragment_shader_invocations_by_lod[..lod_count]),
        ) // 完成当前内容着色器调用文本。
    }; // 完成当前内容管线统计能力分支。
    format!("{label}{timing_report} | {label}{invocation_report}") // 两个分段都带内容名，截图截取后仍能明确归属。
} // 完成单类性能分项格式化。

fn format_lod_values(values: &[u64]) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("/")
}
