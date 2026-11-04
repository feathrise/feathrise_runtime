//! F3 GPU 草地诊断采样与文本构建。
//!
//! 草地候选、GPU回读、四档LOD和每档八个深度桶在本文件中解释；
//! 通用地表由独立格式器提供一行报告，避免植被与地表职责再次混合。

mod adoption; // 解析密度采用阶段与窗口分子/分母独立格式化。
mod profile; // 单类profile的筛选、几何与性能三行独立格式化。

use self::adoption::format_blade_adoption_report; // 主报告只编排后端真实采用证据。
use self::profile::format_content_gpu_report; // 主报告只编排当前普通草或麦田页的三行GPU分项。

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use feathrise_map_content::{VegetationPresetRegistry, VegetationPresetRuntime};
use feathrise_environment::vegetation::{VegetationBladeAdoptionStatus, VegetationBladeLayer}; // F3观察当前布局是否真正进入全部可见绑定窗。
use feathrise_environment::vegetation::blade::{
    GPU_GRASS_DEPTH_BUCKET_COUNT,          // 每个 LOD 固定径向深度桶数量。
    GPU_GRASS_INTERACTION_SOURCE_CAPACITY, // F3从渲染ABI读取固定踩踏源上限，不重复手写八。
    GpuBladeGpuReadbackStats,              // GPU 三缓冲异步回读的真实存活统计。
    GpuBladeGpuTimingStats,                // GPU 分阶段耗时与近中远着色器调用量。
    GpuBladeInteraction,                   // 一份共享风场和固定容量踩踏源状态。
    GpuBladePatch,                         // CPU 提交的近中远候选窗口规格。
    GpuBladeProfileSlot,                   // 按普通草、麦田和后续叶片内容拆分窗口与GPU诊断。
    GpuBladeRenderStats,                   // 上一完整渲染流水线快照。
    GpuBladeStreamingPhase,                // 阶段5首载、淡入、稳定输出和覆盖冻结状态。
    GpuBladeStreamingStats,                // 场景层写入的预测与叶片资产生命周期快照。
    GpuBladeVegetationReadbackStats,       // 单类内容的候选、实绘与真实几何提交量。
    GpuBladeVegetationTimingStats,         // 单类内容的Compute、Draw和着色器调用量。
};

/// F3 草地功能域所需的主世界查询。
#[derive(SystemParam)]
pub(super) struct GrassMonitorQueries<'w, 's> {
    grass_patches: Query<'w, 's, (&'static GpuBladePatch, &'static VegetationBladeLayer)>, // 确认近中远超远四个候选窗口是否创建。
}

/// 单次叶片分页报告共享的只读运行时快照。
///
/// 六个内容页只改变内容身份；渲染、回读、风场和流送证据必须来自同一批资源，
/// 集中传递可避免调用点遗漏某一项诊断或让不同页面形成参数顺序分叉。
pub(super) struct VegetationReportSources<'a> {
    pub(super) presets: Option<&'a VegetationPresetRegistry>, // 正式层身份与档数只读当前目录。
    pub(super) render_stats: Option<&'a GpuBladeRenderStats>,
    pub(super) gpu_stats: Option<&'a GpuBladeGpuReadbackStats>,
    pub(super) gpu_timing: Option<&'a GpuBladeGpuTimingStats>,
    pub(super) interaction: Option<&'a GpuBladeInteraction>,
    pub(super) streaming: Option<&'a GpuBladeStreamingStats>,
    pub(super) adoption: Option<&'a VegetationBladeAdoptionStatus>,
    pub(super) terrain_report: &'a str, // 独立地表格式器生成的阶段、完整纹理组、管线和Opaque证据。
    pub(super) frame_ms: f64,
}

/// 构建单类叶片植被诊断段，普通草、麦田与杂粮田分页分别调用且不会生成其他内容字符串。
pub(super) fn build_vegetation_report(
    preset_id: &str, // 当前分页借用消费工程提供的稳定预设，不要求编译期常量。
    content_label: &'static str,       // 互斥地图复用profile时仍保持普通草与湿地草独立用户身份。
    queries: &GrassMonitorQueries<'_, '_>, // 提供主世界草窗与正式地表承接标记查询。
    sources: &VegetationReportSources<'_>, // 当前刷新周期共享的渲染、回读和流送证据。
) -> String {
    let render_stats = sources.render_stats;
    let gpu_stats = sources.gpu_stats;
    let gpu_timing = sources.gpu_timing;
    let interaction = sources.interaction;
    let streaming = sources.streaming;
    let adoption = sources.adoption;
    let terrain_report = sources.terrain_report; // 地表诊断已与叶片统计分开计算。
    let frame_ms = sources.frame_ms;
    let Some(preset) = sources.presets.and_then(|registry| registry.resolve(preset_id)) else {
        return format!("{content_label}：素材预设尚未就绪");
    };
    let runtime_layer_id = preset.resolved_runtime_layer_id();
    let expected_content_windows = match &preset.runtime {
        VegetationPresetRuntime::Blade { lods, .. } => lods.len() as u32,
        _ => return format!("{content_label}：预设不是GPU叶片"),
    };
    // 按正式层身份筛选，局部GPU槽由绘制批次分配，不能再用地图编号推断草种。
    let mut grass_lods = queries.grass_patches.iter()
        .filter(|(_, layer)| layer.0.0 == runtime_layer_id)
        .map(|(patch, _)| *patch)
        .collect::<Vec<_>>();
    let content_kind = grass_lods.first().map_or(GpuBladeProfileSlot::SLOT_0, |patch| patch.profile_slot());
    grass_lods.sort_by(|left, right| {
        left.profile_slot()
            .index() // 普通草、麦田和后续内容必须各自形成连续的近中远诊断段。
            .cmp(&right.profile_slot().index()) // 稳定profile编号定义跨内容顺序，不依赖窗口创建先后。
            .then_with(|| left.lod_level().cmp(&right.lod_level())) // 同一内容严格按近、中、远编号排序。
            .then_with(|| left.grid_spacing().total_cmp(&right.grid_spacing())) // 同起点时再按格距稳定排序。
    });

    let grass_window_count = grass_lods.len(); // 窗口数应与地图 LOD 圈数一致。
    let has_grass_windows = grass_window_count > 0; // 离开草地图后异步回读资源仍可能保留最后样本，必须以实体生命周期截断。
    let grass_content_kinds = if has_grass_windows {
        vec![content_kind] // 当前页只允许对应稳定profile进入三行GPU分项。
    } else {
        Vec::new() // 当前地图没有该内容时不伪造预留profile数据。
    }; // 完成当前分页唯一内容列表。
    let grass_candidate_count = grass_lods
        .iter()
        .map(|patch| u64::from(patch.instance_count()))
        .sum::<u64>(); // 汇总 CPU 描述的 GPU 候选上限，而非最终可见草株。
    let grass_parameter_bytes = grass_window_count * std::mem::size_of::<GpuBladePatch>(); // 统计每帧提取的草窗参数体积。
    let grass_density_gain_report =
        format_density_gain_report(content_label, &grass_content_kinds, &grass_lods); // 复用profile仍显示当前页自己的密度增益身份。
    let grass_lod_report = format_lod_report(content_label, &grass_lods); // 独立生成稳定排序的近中远诊断行。

    let render_stats = if has_grass_windows {
        render_stats.copied().unwrap_or_default() // 当前草地图可消费本帧RenderApp渲染快照。
    } else {
        GpuBladeRenderStats::default() // 离图后主世界仍可能保留上一帧提取统计，必须随草窗一起清零。
    };
    let streaming = if has_grass_windows {
        streaming.copied().unwrap_or_default() // 当前地图存在草窗时才允许展示阶段5生命周期。
    } else {
        GpuBladeStreamingStats::default() // 离图后不把上一代阶段、预测或资产计数归给当前场景。
    };
    let streaming_is_active = streaming.phase != GpuBladeStreamingPhase::Inactive; // 插件全局注册资源不等于当前地图启用了阶段5会话。
    let (gpu_stats, gpu_timing) =
        current_gpu_diagnostics(has_grass_windows, streaming.phase, gpu_stats, gpu_timing); // 无草窗或新会话首载等待时截断上一代异步样本。
    let profile_index = content_kind.index() as usize; // 稳定两位GPU profile编号直接索引公开分项数组。
    let profile_stats = gpu_stats.by_profile[profile_index]; // 当前页只消费自身候选、实绘、几何与缓冲容量。
    let profile_timing = gpu_timing.by_profile[profile_index]; // 当前页只消费自身Compute、Draw和着色器调用量。

    let grass_compacted_percent = percent(
        profile_stats.rendered_instances,
        profile_stats.candidate_count,
    ); // 当前页最终间接实例只除以自身原始候选。
    let grass_compact_mebibytes = profile_stats.compact_capacity as f64 * 16.0 / 1_048_576.0; // 当前内容每株固定十六字节紧凑实例容量。
    let grass_window_buffer_bytes = profile_stats.dedicated_window_buffer_bytes(); // 当前页只派生自身窗口专用缓冲字节。
    let grass_submitted_vertices = profile_stats
        .submitted_vertices_by_lod
        .into_iter()
        .fold(0_u64, u64::saturating_add); // 从GPU窗口记录的真实原型顶点数汇总，不再把麦田套用普通草常量。
    let grass_submitted_indices = profile_stats
        .submitted_indices_by_lod
        .into_iter()
        .fold(0_u64, u64::saturating_add); // 索引量同样来自每窗回读的真实原型预算，支持草麦网格不同。
    let grass_aggregate_timing_report = if has_grass_windows {
        format_profile_gpu_timing(content_label, &profile_timing, &gpu_timing) // 当前页只保留自身计算/绘制关键耗时。
    } else {
        "GPU计算/绘制 无草窗".to_string() // 不把旧地图的GPU耗时残留到当前场景。
    };
    let grass_lod_count = grass_lods
        .iter()
        .map(|patch| patch.lod_count() as usize)
        .max()
        .unwrap_or(4); // 按局部规格显示扩展档，缺窗不能把八档诊断缩回四档。
    let grass_content_gpu_report = format_content_gpu_report(
        &grass_content_kinds,
        content_label,
        grass_lod_count,
        &gpu_stats,
        &gpu_timing,
    ); // 普通草、湿地草和作物分别解释候选、几何、耗时和调用量。

    let grass_interaction_sources = interaction.map_or(0, GpuBladeInteraction::active_source_count); // 显示玩家与仍在回弹期内的足迹数量。
    let grass_wind_strength = interaction.map_or(0.0, GpuBladeInteraction::wind_tip_strength); // 零表示风场未安装或被关闭。
    let grass_wind_speed = interaction.map_or(0.0, GpuBladeInteraction::wind_speed); // 显示三层世界风场的时间速度。
    let grass_interaction_bytes =
        interaction.map_or(0, |_| std::mem::size_of::<GpuBladeInteraction>()); // 资源存在时报告固定动态参数体积。
    let grass_streaming_phase = format_streaming_phase(streaming.phase); // 使用中文短标签区分首载等待、稳定输出与结果托底。
    let grass_adoption_report = format_blade_adoption_report(has_grass_windows, adoption); // 离图截断旧快照，当前页显示全局三层十二窗采用门。
    let grass_prediction_distance = streaming.prediction_offset.length(); // 前视偏移由场景预测器给出，不把草窗原点误报为加载延迟。
    let grass_anchor_offset = if streaming_is_active {
        // 只有当前地图存在有效流送会话时才读取近圈参数。
        grass_lods.first().map_or(0.0, |patch| {
            // 稳定排序后的首项必定是内半径为零的近圈。
            let origin_focus = patch.origin() - patch.focus(); // 近圈吸附原点与连续玩家中心之差才是真实窗口锚偏。
            Vec2::new(origin_focus.x, origin_focus.z).length() // 只报告地表水平距离，固定参考高度不参与锚点诊断。
        }) // 阶段5资源存在但草窗尚未创建时保持零锚偏。
    } else {
        // 插件缺失或已离开草地图时不得从残留/独立组件查询推导阶段5数值。
        0.0 // 未启用会话时即使外部手工插入草窗，阶段5诊断仍严格全零。
    }; // 完成近圈真实锚偏与插件缺失回退计算。
    let grass_activation_percent = streaming.activation.clamp(0.0, 1.0) * 100.0; // 防御异常遥测值污染面板百分比。
    let grass_expected_draws =
        expected_draw_command_count(grass_window_count, expected_content_windows); // 当前页缺窗时仍保留自身正式分母。
    let grass_stage6_report = format_stage6_acceptance_report(
        content_label, // 摘要标题严格使用当前分页用户可读内容身份。
        &streaming,    // 地图层提供正式编号、内容身份、资产与输入显存规格。
        &Stage6AcceptanceInputs {
            actual_windows: grass_window_count, // 主世界实际存在的窗口数量。
            expected_windows: expected_content_windows, // 当前内容固定窗口规格。
            profile_stats: &profile_stats,      // 异步回读提供当前内容实际实绘实例。
            profile_timing: &profile_timing,    // 当前内容Compute与Draw耗时。
            gpu_timing: &gpu_timing,            // 聚合资源只提供能力标志和样本序号。
            window_buffer_bytes: grass_window_buffer_bytes, // 可精确派生的窗口缓冲。
            frame_ms,                           // 使用资源面板顶部同一平滑帧耗口径。
        },
    ); // 生成阶段6固定路线实机验收可直接截图保存的两行摘要。

    format!(
        "{content_label} / GPU叶片植被\n\
         {grass_stage6_report}\n\
         {content_label}窗口 {grass_window_count} | 候选 {grass_candidate_count} | 参数 {grass_parameter_bytes}B | 增益 {grass_density_gain_report}\n\
         草动态 风 {grass_wind_strength:.3}m×{grass_wind_speed:.2} | 踩踏源 {grass_interaction_sources}/{grass_interaction_capacity} | 参数 {grass_interaction_bytes}B\n\
         {grass_lod_report}\n\
         {terrain_report}\n\
         草流送 阶段 {grass_streaming_phase} | 代次 {grass_streaming_generation} | 常驻资产 {grass_resident_assets}/{grass_expected_resident_assets} | 速度 {grass_streaming_speed:.1}m/s | 预测 {grass_prediction_distance:.1}m | 锚偏 {grass_anchor_offset:.1}m | 淡入 {grass_activation_percent:.0}% | 锚更新 {grass_anchor_updates} | 旧结果 {grass_resident_reused}\n\
         种植布局密度 {grass_adoption_report}\n\
         共享后端 输入提取 {grass_extracted} | 叶片输入就绪 {grass_input_ready} 高度待 {grass_height_waiting} 密度待 {grass_density_waiting} | 绑定 {grass_bind_groups} U待 {grass_uniform_waiting}\n\
         共享后端 Compute 就绪 {grass_compute_ready} 等待 {grass_compute_waiting} 错误 {grass_compute_error}\n\
         共享后端 排队 遮罩视图 {grass_target_views} 可见窗 {grass_visible} | Draw {grass_queued_draws}/{grass_queued_candidates}\n\
         共享后端 AlphaMask 就绪 {grass_pipeline_ready} 等待 {grass_pipeline_waiting} 错误 {grass_pipeline_error} | LOD掩码 准备/管线/实绘 {grass_prepared_lod_mask:04b}/{grass_pipeline_lod_mask:04b}/{grass_drawn_lod_mask:04b} | 内容LOD {grass_prepared_profile_lod_mask:016b}/{grass_pipeline_profile_lod_mask:016b}/{grass_drawn_profile_lod_mask:016b} | 深度桶 {grass_depth_buckets}/LOD | 当前页期望命令 {grass_expected_draws}\n\
         共享槽采样 GPU样本 {grass_gpu_sequence} | 窗 {grass_gpu_windows} | 候选 {grass_gpu_candidates} | 最终 {grass_gpu_rendered} ({grass_compacted_percent:.2}%) | 几何 顶点 {grass_submitted_vertices} 索引 {grass_submitted_indices} | 容量 {grass_gpu_capacity}/{grass_compact_mebibytes:.1}MiB | {grass_aggregate_timing_report} | 溢出 {grass_gpu_overflow}\n\
         GPU按共享槽采样，可能包含其他预设；逐层数据见F4\n\
         {grass_content_gpu_report}\n\
         共享回读 三缓冲丢样 {grass_gpu_dropped} | 截断窗 {grass_gpu_truncated} | 映射错误 {grass_gpu_map_errors}\n\
         共享后端 跳过阶段 {grass_missing_phases} 主网格 {grass_missing_mesh_instances} GPU网格 {grass_missing_gpu_meshes} 特化 {grass_specialization_errors}",
        grass_extracted = render_stats.extracted_windows, // RenderApp 提取到的草窗口数。
        grass_input_ready = render_stats.input_ready_windows, // 高度与植被密度输入均已就绪的窗口数。
        grass_height_waiting = render_stats.height_waiting_windows, // 等待 GPU 高度纹理的窗口数。
        grass_density_waiting = render_stats.density_waiting_windows, // 等待 GPU 密度纹理的窗口数。
        grass_bind_groups = render_stats.prepared_bind_groups, // 已建立窗口绑定组数。
        grass_streaming_generation = streaming.generation, // 每次进入草地图或重建整组驻留资源时递增。
        grass_resident_assets = streaming.resident_assets, // 当前地图会话已真实可达的叶片网格、高度和植被密度资产数量。
        grass_expected_resident_assets = streaming.expected_resident_assets, // 地图规格声明的稳定输出完整资产数。
        grass_streaming_speed = streaming.smoothed_speed, // 预测器使用的平滑水平速度，不是CPU任务吞吐。
        grass_anchor_updates = streaming.anchor_updates,  // 本会话跨越22.4米吸附单元的次数。
        grass_resident_reused = if streaming_is_active {
            render_stats.resident_reused_windows // 当前草地图本帧继续消费上一份有效GPU输出的窗口数。
        } else {
            0 // 离图或未启用阶段5时不得把全局渲染残留显示成当前地图状态。
        },
        grass_uniform_waiting = render_stats.uniform_waiting_windows, // 等待动态 uniform 的窗口数。
        grass_compute_ready = render_stats.compute_pipeline_ready_stages, // 已完成异步编译并可执行的固定 Compute 阶段数。
        grass_compute_waiting = render_stats.compute_pipeline_waiting_stages, // 仍在排队或创建的固定 Compute 阶段数。
        grass_compute_error = render_stats.compute_pipeline_error_stages, // 编译或设备创建失败的固定 Compute 阶段数。
        grass_target_views = render_stats.target_views, // 进入草地排队的玩法视图数。
        grass_visible = render_stats.visible_windows,   // 玩法视图可见集中的草窗口数。
        grass_queued_draws = render_stats.queued_draws, // AlphaMask 阶段接收的草 LOD 项数。
        grass_queued_candidates = render_stats.queued_candidates, // 排队项描述的候选实例上限。
        grass_pipeline_ready = render_stats.pipeline_ready_draws, // 排队时管线已可执行的 LOD 项数。
        grass_pipeline_waiting = render_stats.pipeline_waiting_draws, // 管线仍在异步创建的 LOD 项数。
        grass_pipeline_error = render_stats.pipeline_error_draws,     // 管线创建失败的 LOD 项数。
        grass_prepared_lod_mask = render_stats.prepared_lod_mask, // Compute绑定组实际覆盖的近中远身份。
        grass_pipeline_lod_mask = render_stats.pipeline_ready_lod_mask, // 已就绪AlphaMask管线覆盖的近中远身份。
        grass_drawn_lod_mask = render_stats.drawn_lod_mask, // 真正编码间接命令的近中远身份。
        grass_prepared_profile_lod_mask = render_stats.prepared_profile_lod_mask, // Compute绑定组实际覆盖的内容与LOD组合。
        grass_pipeline_profile_lod_mask = render_stats.pipeline_ready_profile_lod_mask, // 已就绪管线覆盖的内容与LOD组合。
        grass_drawn_profile_lod_mask = render_stats.drawn_profile_lod_mask, // 真正编码间接命令的内容与LOD组合。
        grass_depth_buckets = GPU_GRASS_DEPTH_BUCKET_COUNT, // 每 LOD 固定八桶，三个 LOD 最多二十四条命令。
        grass_interaction_capacity = GPU_GRASS_INTERACTION_SOURCE_CAPACITY, // 玩家与足迹源上限直接读取渲染ABI。
        grass_gpu_sequence = gpu_stats.sample_sequence, // 零表示首份非阻塞回读尚未完成。
        grass_gpu_windows = profile_stats.sampled_windows, // 本样本当前内容实际汇总的窗口数。
        grass_gpu_truncated = gpu_stats.truncated_windows, // 超过固定回读容量的草窗数。
        grass_gpu_candidates = profile_stats.candidate_count, // 当前内容draw state记录的GPU候选总数。
        grass_gpu_capacity = profile_stats.compact_capacity,  // 当前内容紧凑输出可容纳的实例总数。
        grass_gpu_rendered = profile_stats.rendered_instances, // 当前内容间接绘制最终消费实例数。
        grass_gpu_overflow = profile_stats.overflow_instances, // 当前内容容量不足时未进入绘制的存活数。
        grass_gpu_dropped = gpu_stats.dropped_samples, // 槽位忙时丢弃诊断样本，不代表渲染丢帧。
        grass_gpu_map_errors = gpu_stats.map_errors,   // WebGPU 异步映射错误累计。
        grass_missing_phases = render_stats.missing_phase_views, // 缺少 AlphaMask3d 阶段的目标视图数。
        grass_missing_mesh_instances = render_stats.missing_mesh_instances, // 缺少主世界 Mesh3d 同步数据的窗口数。
        grass_missing_gpu_meshes = render_stats.missing_gpu_meshes, // 草原型尚未上传 GPU 的窗口数。
        grass_specialization_errors = render_stats.specialization_errors, // 管线与网格布局专用化失败数。
    )
}

/// 阶段6验收摘要使用的当前内容实测输入。
struct Stage6AcceptanceInputs<'a> {
    actual_windows: usize,
    expected_windows: u32,
    profile_stats: &'a GpuBladeVegetationReadbackStats,
    profile_timing: &'a GpuBladeVegetationTimingStats,
    gpu_timing: &'a GpuBladeGpuTimingStats,
    window_buffer_bytes: u64,
    frame_ms: f64,
}

/// 生成阶段6正式地图集成与固定路线性能记录摘要。
fn format_stage6_acceptance_report(
    content_label: &'static str,         // 当前分页唯一用户可读内容身份。
    streaming: &GpuBladeStreamingStats,  // 地图层发布正式地图身份、应有资源与数值输入字节。
    inputs: &Stage6AcceptanceInputs<'_>, // 当前内容窗口、GPU样本、缓冲与CPU帧耗。
) -> String {
    let actual_windows = inputs.actual_windows;
    let expected_windows = inputs.expected_windows;
    let profile_stats = inputs.profile_stats;
    let profile_timing = inputs.profile_timing;
    let gpu_timing = inputs.gpu_timing;
    let window_buffer_bytes = inputs.window_buffer_bytes;
    let frame_ms = inputs.frame_ms;
    if streaming.map_code.is_empty() {
        return "草正式接入 未启用".to_string(); // 未登记流送遥测时不伪造已通过的验收记录。
    } // 完成无正式地图身份快速返回。
    let gpu_timing_summary = if !gpu_timing.timestamp_supported {
        "不可用".to_string() // 后端未启用通道内时间戳时不以CPU帧耗冒充GPU耗时。
    } else if gpu_timing.sample_sequence == 0 {
        "等待样本".to_string() // 三缓冲尚未完成首份异步映射时保持明确等待。
    } else {
        format!(
            "{:.3}/{:.3}ms",
            profile_timing.compute_total_ms, // 当前内容粗瓦片、分类与收尾Compute总耗时。
            profile_timing.draw_total_ms,    // 当前内容近中远全部间接draw总耗时。
        ) // 固定使用“计算/绘制”顺序便于不同截图横向比较。
    }; // 完成GPU计时能力分支。
    let input_mebibytes = streaming.resident_input_bytes as f64 / 1_048_576.0; // 三份地图专用数值纹理精确换算MiB。
    let window_buffer_mebibytes = window_buffer_bytes as f64 / 1_048_576.0; // 实际窗口缓冲精确换算MiB。
    let known_dedicated_mebibytes = streaming
        .resident_input_bytes
        .saturating_add(window_buffer_bytes) as f64
        / 1_048_576.0; // 只给出当前能够证明的草地专用显存下界，不冒充进程总显存。
    let safe_frame_ms = if frame_ms.is_finite() {
        frame_ms.max(0.0) // 平滑诊断合法时保留真实CPU帧耗。
    } else {
        0.0 // 非有限诊断样本不得污染F3文本。
    }; // 完成CPU帧耗防御。
    format!(
        "{content_label}正式接入 {} | 窗口 {}/{} | 共享资产 {}/{}\n\
         {content_label}验收 实绘 {} | CPU帧 {:.2}ms | GPU计算/绘制 {} | 速度 当前/峰 {:.1}/{:.1}m/s | 已知下界 {:.1}MiB [共享输入 {:.1}+本页窗口 {:.1}]",
        streaming.map_code,                 // 显示设计文档稳定地图编号。
        actual_windows,                     // 显示主世界实际草窗数量。
        expected_windows,                   // 显示当前内容自身正式规格窗数。
        streaming.resident_assets,          // 显示当前会话真实可达资产数。
        streaming.expected_resident_assets, // 显示稳定输出完整资产数。
        profile_stats.rendered_instances,   // 使用当前内容GPU间接绘制真实消费实例数。
        safe_frame_ms,                      // 固定路线截图直接保留CPU帧耗。
        gpu_timing_summary,                 // 按“计算/绘制”显示GPU时间戳或明确能力状态。
        streaming.smoothed_speed,           // 显示当前平滑水平速度。
        streaming.peak_speed,               // 显示排除传送后的本会话最坏水平速度。
        known_dedicated_mebibytes,          // 已知数值输入与窗口缓冲总和是显存下界。
        input_mebibytes,                    // 单独显示地图数值输入方便核对资产预算。
        window_buffer_mebibytes,            // 单独显示随候选容量变化的窗口缓冲压力。
    )
}

/// 把通用流送阶段转换为适合 F3 紧凑面板的中文状态。
fn format_streaming_phase(phase: GpuBladeStreamingPhase) -> &'static str {
    match phase {
        GpuBladeStreamingPhase::Inactive => "未启用", // 非草地图不应误报为加载失败。
        GpuBladeStreamingPhase::Waiting => "首载等待", // 尚无可复用驻留结果时允许等待首份输出。
        GpuBladeStreamingPhase::FadingIn => "逐株淡入", // 世界稳定遮罩覆盖率正在零到一推进。
        GpuBladeStreamingPhase::Resident => "稳定输出", // 数值输入、三类内容管线和十个重叠窗口均处于稳定状态。
        GpuBladeStreamingPhase::Holding => "覆盖冻结等待", // 阶段只承诺冻结淡入；是否真实复用旧GPU输出由同一行“旧结果”计数证明。
    }
}

/// 计算百分比并统一保护空分母。
fn percent(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 0.0; // 无样本时明确返回零，避免 NaN 污染 F3 文本。
    }
    numerator as f64 * 100.0 / denominator as f64 // 使用 f64 保持大候选量下的显示精度。
}

/// 只把异步GPU样本归属给当前仍存在的草窗；RenderApp全局资源本身可以跨地图长期存在。
fn current_gpu_diagnostics(
    has_grass_windows: bool,
    streaming_phase: GpuBladeStreamingPhase,
    gpu_stats: Option<&GpuBladeGpuReadbackStats>,
    gpu_timing: Option<&GpuBladeGpuTimingStats>,
) -> (GpuBladeGpuReadbackStats, GpuBladeGpuTimingStats) {
    if !has_grass_windows || streaming_phase == GpuBladeStreamingPhase::Waiting {
        return (Default::default(), Default::default()); // 离图或新会话首载时清零，禁止显示上一代GPU样本。
    }
    (
        gpu_stats.copied().unwrap_or_default(), // 首份实例回读前以序号零表达等待。
        gpu_timing.copied().unwrap_or_default(), // 后端能力或首份计时样本缺失时使用明确默认快照。
    )
}

/// 按内容profile与LOD编号格式化普通草、麦田等独立候选窗口。
fn format_lod_report(content_label: &'static str, grass_lods: &[GpuBladePatch]) -> String {
    if grass_lods.is_empty() {
        return "草LOD 无".to_string(); // 非草地图或窗口尚未创建时给出明确状态。
    }

    grass_lods
        .iter()
        .map(|patch| {
            let lod_label = if patch.lod_count() > 4 {
                patch.lod_level().to_string() // 扩展档以真实编号区分，不能都标成超远。
            } else {
                match patch.lod_level() {
                    0 => "近",
                    1 => "中",
                    2 => "远",
                    _ => "超远",
                }
                .to_string()
            };
            let ultra_budget = if patch.uses_ultra_lod_chain()
                && patch.lod_level() + 1 == patch.lod_count()
            {
                format!(" | 固定预算 {}×{}", patch.ultra_lod_budget_block_side(), patch.ultra_lod_budget_block_side()) // F3直接显示每块只选一个超远代理的档位。
            } else {
                String::new() // 仅真正的最终粗档使用固定预算。
            };
            format!(
                "{content_label}LOD{lod_label} {} | 格距 {:.3}m | 窗口裁剪 {:.0}-{:.0}m{ultra_budget}",
                patch.instance_count(), // 当前 LOD 候选实例数。
                patch.grid_spacing(),   // GPU 世界稳定草格间距。
                patch.inner_radius(),   // 正式模式为零，旧径向兼容模式仍显示其候选裁剪内沿。
                patch.visible_radius(), // 玩家周围候选窗口安全外沿，明确不代表视觉LOD切换线。
            )
        })
        .collect::<Vec<_>>()
        .join("\n") // 每个 LOD 独占一行，便于定位缺失层级。
}

/// 按当前实际内容显示各自独立的密度概率增益。
fn format_density_gain_report(
    content_label: &'static str, // 互斥地图复用零号profile时显示当前页面身份。
    content_kinds: &[GpuBladeProfileSlot], // 已按稳定profile去重的当前内容列表。
    grass_lods: &[GpuBladePatch], // 当前内容的近中远窗口参数。
) -> String {
    if content_kinds.is_empty() {
        return "无".to_string(); // 非草地图不显示一个容易被误读为普通草配置的零值。
    }
    content_kinds
        .iter()
        .map(|kind| {
            let gain = grass_lods
                .iter()
                .find(|patch| patch.profile_slot() == *kind)
                .map_or(0.0, |patch| patch.density_gain()); // 同一内容的三个LOD共享密度图与增益，读取任意首窗即可。
            format!("{content_label}{gain:.2}") // 两位小数保留当前内容真实配置且不把湿地草误标普通草。
        })
        .collect::<Vec<_>>()
        .join("/") // 内容较多时仍保持单行紧凑可截图。
}

/// 计算正式地图应该提交的间接绘制命令数。
fn expected_draw_command_count(actual_windows: usize, expected_windows: u32) -> u32 {
    let formal_windows = if expected_windows == 0 {
        u32::try_from(actual_windows).unwrap_or(u32::MAX) // 独立样例没有地图规格时继续按实际窗口给出兼容口径。
    } else {
        expected_windows // 从当前预设读取完整规格，缺窗时仍保留分母以暴露故障。
    };
    formal_windows.saturating_mul(GPU_GRASS_DEPTH_BUCKET_COUNT) // 每窗固定近到远八个径向深度桶。
}

/// 当前分页只显示自身profile的Compute/Draw关键耗时。
fn format_profile_gpu_timing(
    label: &'static str, // 决定普通草、湿地草、麦田等耗时行内容名。
    profile: &GpuBladeVegetationTimingStats, // 当前内容独立时间戳分项。
    timing: &GpuBladeGpuTimingStats, // 提供本次查询能力标记和样本序号。
) -> String {
    if !timing.timestamp_supported {
        return format!("{label} GPU计算/绘制 不可用"); // 不支持时间戳时不以CPU数字冒充。
    }
    if timing.sample_sequence == 0 {
        return format!("{label} GPU计算/绘制 等待样本"); // 三缓冲尚未完成首份映射时不展示伪零耗时。
    }
    format!(
        "{label} GPU计算/绘制 {:.3}/{:.3}ms",
        profile.compute_total_ms, // 当前内容Compute耗时。
        profile.draw_total_ms,    // 当前内容Draw耗时。
    )
}

#[cfg(test)] // 纯逻辑测试拆到独立文件，生产报告模块保持低于五百行。
mod tests; // 覆盖样本代次、窗口分母、内容身份和profile几何预算。
