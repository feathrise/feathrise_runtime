use std::sync::atomic::AtomicU64;

use bevy::prelude::{Resource, Vec2};
#[cfg(feature = "diagnostics")]
use bevy::render::render_resource::Buffer;
use wgpu::QuerySet;

#[cfg(feature = "diagnostics")]
use crate::constants::{
    GPU_GRASS_COMPACT_INSTANCE_BYTES, GPU_GRASS_DEPTH_BUCKETS_BYTES, GPU_GRASS_DRAW_STATE_BYTES,
    GPU_GRASS_TILE_DISPATCH_BYTES, GPU_GRASS_TILE_INDEX_BYTES,
};
#[cfg(feature = "diagnostics")]
use crate::types::{GPU_GRASS_LOD_CAPACITY, GPU_GRASS_PROFILE_CAPACITY};

/// GPU草地驻留窗口的主世界生命周期阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuBladeStreamingPhase {
    #[default]
    Inactive, // 当前场景没有启用GPU草地，所有阶段5指标应为零。
    Waiting,  // 地表或GPU管线尚未产生首份可安全绘制的驻留输出。
    FadingIn, // 首份驻留输出已经就绪，草地正通过稳定抖动覆盖率渐入。
    Resident, // 预测窗口、数值输入和GPU输出均处于稳定可复用状态。
    Holding,  // 临时异常期间冻结现有覆盖率；是否仍真实绘制旧输出必须另看驻留复用与实绘计数。
}

/// 场景层写入、F3读取的通用GPU草地流送快照。
///
/// 【功能设计】该资源只存在于主世界，不提取到RenderApp；渲染统计继续走独立帧快照，避免预测状态参与GPU同步。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuBladeStreamingStats {
    pub generation: u32, // 每次进入草场景或重建整组驻留资源时递增的生命周期代次。
    pub phase: GpuBladeStreamingPhase, // 当前首载、淡入、驻留或覆盖冻结等待阶段。
    pub scene_code: &'static str, // 消费场景可写入稳定编号，空字符串表示当前没有正式内容接入。
    pub content_label: &'static str, // 区分普通草、作物或其他未来植被内容，避免验收口径混淆。
    pub expected_windows: u32, // 当前场景规格要求的完整草窗数量，用于F3显示实际/应有。
    pub resident_assets: u32, // 当前场景生命周期显式持有的高度、密度、网格与材质资产数量。
    pub expected_resident_assets: u32, // 当前场景稳定输出时应真实可达的完整资产数量。
    pub resident_input_bytes: u64, // 场景层能够精确证明的专用GPU数值纹理驻留字节。
    pub smoothed_speed: f32, // 场景预测器使用的米每秒平滑玩家速度。
    pub peak_speed: f32, // 本场景会话排除传送后观察到的最大真实水平速度。
    pub prediction_offset: Vec2, // 预测锚点相对真实玩家位置的世界X/Z偏移米数。
    pub anchor_updates: u64, // 本生命周期内草窗驻留锚点跨吸附单元更新的累计次数。
    pub activation: f32, // 首份有效GPU输出从零到一渐入时使用的全局稳定遮罩覆盖率。
}

/// 最近一份已完成的GPU草地压缩状态异步回读。
///
/// 【功能设计】这些值来自计算着色器写入的80字节`draw_state`，而不是CPU估算；
/// 三缓冲回读允许统计滞后数帧，但绝不允许为了F3数字阻塞渲染线程。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg(feature = "diagnostics")]
pub struct GpuBladeVegetationReadbackStats {
    pub sampled_windows: u32,    // 当前内容本样本实际回读的近中远窗口数。
    pub candidate_count: u64,    // 当前内容全部窗口的原始GPU候选总数。
    pub total_tiles: u64,        // 当前内容候选网格切分后的粗瓦片总数。
    pub active_tiles: u64,       // 当前内容通过距离与视锥粗判的活跃瓦片数。
    pub scanned_candidates: u64, // 当前内容真正启动细分类的候选线程数。
    pub compact_capacity: u64,   // 当前内容紧凑实例缓冲可容纳的总实例数。
    pub distance_survivors: u64, // 当前内容通过距离、视锥与层级LOD的实际候选数。
    pub density_survivors: u64,  // 当前内容继续通过独立密度图概率判定的候选数。
    pub surface_survivors: u64,  // 当前内容继续取得合法地表高度的最终存活数。
    pub rendered_instances: u64, // 当前内容最终由间接绘制消费的钳制实例数。
    pub rendered_instances_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // 当前内容近中远超远四档实绘实例数。
    pub submitted_vertices_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // 实绘实例乘当前原型真实顶点数的几何提交量。
    pub submitted_indices_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // 实绘实例乘当前原型真实索引数的几何提交量。
    pub overflow_instances: u64, // 当前内容超出紧凑容量而未能进入绘制的存活数。
}

#[cfg(feature = "diagnostics")]
impl GpuBladeVegetationReadbackStats {
    /// 返回当前内容全部窗口可由GPU状态精确证明的专用缓冲字节。
    pub fn dedicated_window_buffer_bytes(&self) -> u64 {
        let compact_bytes = self
            .compact_capacity
            .saturating_mul(GPU_GRASS_COMPACT_INSTANCE_BYTES); // 每个最终实例固定二十字节。
        let tile_index_bytes = self.total_tiles.saturating_mul(GPU_GRASS_TILE_INDEX_BYTES); // 每个候选粗瓦片固定一个u32索引槽。
        let fixed_window_bytes = u64::from(self.sampled_windows).saturating_mul(
            GPU_GRASS_DRAW_STATE_BYTES
                + GPU_GRASS_DEPTH_BUCKETS_BYTES
                + GPU_GRASS_TILE_DISPATCH_BYTES * 2,
        ); // 每窗持有聚合状态、八桶状态、真实间接派发参数及用于隔离 wgpu 使用域的同尺寸 scratch。
        compact_bytes
            .saturating_add(tile_index_bytes)
            .saturating_add(fixed_window_bytes) // 保持与聚合统计完全相同的饱和预算口径。
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg(feature = "diagnostics")]
pub struct GpuBladeGpuReadbackStats {
    pub sample_sequence: u64, // 单调递增的GPU样本序号，零表示尚未完成首次回读。
    pub source_revision: u64, // 可选统一作者票据；缺失、混合或输出未就绪为0，不能冒称当前新修订。
    pub sampled_windows: u32, // 本样本实际回读的草窗状态数量。
    pub truncated_windows: u32, // 超过固定诊断容量而未回读的额外草窗数量。
    pub candidate_count: u64, // GPU状态中的原始候选总数，用作各阶段存活率分母。
    pub total_tiles: u64,     // 候选正方形切分后的全部十六乘十六粗瓦片数。
    pub active_tiles: u64,    // 通过GPU环带与保守视锥粗判、实际进入细分类的瓦片数。
    pub scanned_candidates: u64, // 活跃瓦片实际启动的细分类候选线程数。
    pub compact_capacity: u64, // 所有内容窗口的20字节紧凑实例缓冲可容纳的总实例数。
    pub distance_survivors: u64, // 通过玩法视锥、层级LOD、常驻范围和激活阈值的实际候选数。
    pub density_survivors: u64, // 继续通过密度图概率判定的实际候选数。
    pub surface_survivors: u64, // 继续取得合法地表高度的最终存活数，容量不足时可大于实绘数。
    pub rendered_instances: u64, // `draw_indexed_indirect`最终消费的已钳制实例总数。
    pub rendered_instances_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // 所有内容近中远超远实际实例数，保留聚合口径。
    pub by_profile: [GpuBladeVegetationReadbackStats; GPU_GRASS_PROFILE_CAPACITY], // 普通草、麦田、芦苇和保留内容按稳定profile分别统计。
    pub overflow_instances: u64, // 最终存活数超出紧凑输出容量的总数，正常必须为零。
    pub dropped_samples: u64,    // 三个回读槽都被GPU占用时主动丢弃的诊断样本数。
    pub scene_errors: u64,         // 异步映射失败的累计次数，用于区分统计通道与草地管线故障。
}

#[cfg(feature = "diagnostics")]
impl GpuBladeGpuReadbackStats {
    /// 返回本样本覆盖草窗的紧凑实例、瓦片列表和固定状态缓冲总字节。
    ///
    /// 这里只统计能够从真实容量精确派生的窗口专用长期缓冲；
    /// 网格、纹理、共享uniform、查询回读槽与驱动内部开销由各自指标说明，不伪装成进程总显存。
    pub fn dedicated_window_buffer_bytes(&self) -> u64 {
        let compact_bytes = self
            .compact_capacity
            .saturating_mul(GPU_GRASS_COMPACT_INSTANCE_BYTES); // 每个最终实例固定二十字节。
        let tile_index_bytes = self.total_tiles.saturating_mul(GPU_GRASS_TILE_INDEX_BYTES); // 每个候选粗瓦片固定一个u32索引槽。
        let fixed_window_bytes = u64::from(self.sampled_windows).saturating_mul(
            GPU_GRASS_DRAW_STATE_BYTES
                + GPU_GRASS_DEPTH_BUCKETS_BYTES
                + GPU_GRASS_TILE_DISPATCH_BYTES * 2,
        ); // 每窗持有聚合状态、八桶状态、真实间接派发参数及同尺寸分类 scratch。
        compact_bytes
            .saturating_add(tile_index_bytes)
            .saturating_add(fixed_window_bytes) // 所有加法饱和，损坏诊断值也不会回绕成极小显存。
    }
}

/// 最近一份已完成的GPU草地分阶段耗时与着色器调用量。
///
/// 【功能设计】时间戳和管线统计都在GPU命令流内部采集，经三缓冲异步回读；
/// 不支持对应查询能力的显卡或后端只把支持标记设为false，绝不回退为同步等待或伪造CPU耗时。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg(feature = "diagnostics")]
pub struct GpuBladeVegetationTimingStats {
    pub compute_total_ms: f64,      // 当前内容粗瓦片至间接参数收尾的GPU总耗时。
    pub tile_cull_ms: f64,          // 当前内容粗瓦片剔除和间接派发参数生成耗时。
    pub candidate_classify_ms: f64, // 当前内容活跃候选逐株分类与压缩耗时。
    pub compute_finalize_ms: f64,   // 当前内容实例钳制和深度桶间接参数收尾耗时。
    pub draw_total_ms: f64,         // 当前内容近中远全部间接绘制耗时。
    pub draw_lod_ms: [f64; GPU_GRASS_LOD_CAPACITY], // 当前内容近中远超远各八个径向桶的合计绘制耗时。
    pub vertex_shader_invocations_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // 当前内容四档实际顶点着色器调用量。
    pub fragment_shader_invocations_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // 当前内容四档实际片元着色器调用量。
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
#[cfg(feature = "diagnostics")]
pub struct GpuBladeGpuTimingStats {
    pub sample_sequence: u64, // 单调递增样本序号，零表示查询支持但尚未完成首份回读。
    pub timestamp_supported: bool, // 当前图形后端是否同时支持查询与通道内时间戳。
    pub pipeline_statistics_supported: bool, // 当前图形后端是否支持顶点与片元调用量查询。
    pub compute_total_ms: f64, // 粗瓦片剔除至间接绘制参数收尾的GPU总耗时。
    pub tile_cull_ms: f64,    // 粗瓦片剔除和活跃瓦片间接派发参数生成耗时。
    pub candidate_classify_ms: f64, // 活跃候选逐草分类、密度与地表筛选耗时。
    pub compute_finalize_ms: f64, // 钳制紧凑实例数并生成四档各八条深度桶间接参数的耗时。
    pub draw_total_ms: f64,   // 全部内容窗口的草地间接命令耗时之和，不包含其他AlphaMask物体。
    pub draw_lod_ms: [f64; GPU_GRASS_LOD_CAPACITY], // 近、中、远、超远各八桶间接命令的合计GPU绘制耗时。
    pub vertex_shader_invocations_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // GPU实际执行的四档顶点着色器调用量。
    pub fragment_shader_invocations_by_lod: [u64; GPU_GRASS_LOD_CAPACITY], // GPU实际执行的四档片元着色器调用量，用于识别像素过绘瓶颈。
    pub by_profile: [GpuBladeVegetationTimingStats; GPU_GRASS_PROFILE_CAPACITY], // 四类内容独立计时，旧聚合字段继续保存分项之和。
    pub dropped_samples: u64, // 三个回读槽全忙时主动丢弃的诊断样本数。
    pub scene_errors: u64,      // 异步映射失败累计数，只影响F3诊断而不影响草地绘制。
}

/// RenderApp 内用于协调 GPU 草地查询与 Bevy 全局渲染诊断的兼容开关。
///
/// 【功能设计】wgpu 同一渲染通道只允许一个活动的管线统计查询；Bevy 的
/// `RenderDiagnosticsPlugin` 会包围整个 AlphaMask 通道，因此该插件存在时草地必须只保留
/// 自己的时间戳，避免在外层统计查询内部再次开启统计查询并触发设备校验错误。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg(feature = "diagnostics")]
pub(crate) struct GpuBladeTimingCompatibility {
    pub(crate) reserve_pipeline_statistics_for_render_diagnostics: bool, // true 时让 Bevy 独占通道级管线统计，草地时间戳仍正常采集。
}

/// RenderApp持有的一组固定GPU时间戳、管线统计查询和解析缓冲。
#[derive(Resource, Default)]
pub(crate) struct GpuBladeTimingQueries {
    pub(crate) timestamp_query_set: Option<QuerySet>, // 四十个固定槽记录四类内容的Compute四阶段与近中远绘制起止。
    pub(crate) pipeline_statistics_query_set: Option<QuerySet>, // 十二个固定槽记录四类内容各近中远着色器调用量。
    #[cfg(feature = "diagnostics")]
    pub(crate) resolve_buffer: Option<Buffer>, // QUERY_RESOLVE写入后再拷贝到可映射三缓冲，CPU从不直接等待该缓冲。
    #[cfg(feature = "diagnostics")]
    pub(crate) timestamp_period_ns: f32, // GPU tick到纳秒的设备队列换算因子。
    pub(crate) write_mask: AtomicU64, // 四类Compute和十二组Draw查询共使用六十位无锁标记。
}
