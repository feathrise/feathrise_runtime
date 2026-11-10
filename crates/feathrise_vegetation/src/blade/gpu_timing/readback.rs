//! GPU草地固定查询集的异步三缓冲回读与主世界发布。
//! 每四帧只解析真实写入的连续查询段；槽位繁忙时丢样，正式Compute与draw永不等待诊断。

mod decode; // 只把完成映射的稀疏字节解码为草麦分项与既有聚合计时。
mod resolve_plan; // 把本帧查询写入掩码切成不含空洞的对齐解析段。

use std::sync::{
    // 映射回调可在渲染线程之外完成。
    Arc,   // 保证回调完成前staging缓冲、槽位状态和共享快照持续存活。
    Mutex, // 只保护一份百余字节最新快照，锁内不调用任何GPU接口。
    atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering}, // 三缓冲状态、六十位查询掩码和累计故障使用无锁原子量。
}; // 完成跨线程基础类型导入。

use bevy::app::SubApp; // 在既有RenderApp中安装查询资源和根RenderGraph调度系统。
use bevy::core_pipeline::schedule::camera_driver; // 所有玩法相机绘制结束后才能解析近中远draw查询。
use bevy::prelude::*; // Commands、Res、ResMut与Resource等ECS基础类型。
use bevy::render::render_resource::{
    Buffer,           // 解析缓冲和三个MAP_READ staging槽的GPU句柄。
    BufferDescriptor, // 固定最坏容量缓冲创建描述。
    BufferUsages,     // 区分QUERY_RESOLVE、COPY_SRC、COPY_DST与MAP_READ用途。
    MapMode,          // staging槽只发起非阻塞只读映射。
}; // 完成wgpu查询和缓冲类型导入。
use bevy::render::renderer::{
    RenderContext, RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue,
}; // 创建查询、取得tick周期，并确保回读命令在根图提交阶段前完成录入。
use bevy::render::{ExtractSchedule, MainWorld, RenderStartup}; // 帧首发起map_async并向主世界F3发布已完成样本。
use wgpu::{Features, PipelineStatisticsTypes, QuerySetDescriptor, QueryType}; // 使用Bevy同版本wgpu的固定查询描述与设备能力位。

use crate::resources::{
    GpuBladeGpuTimingStats,      // 公开F3聚合快照与四类内容分项。
    GpuBladeTimingCompatibility, // 决定全局渲染诊断存在时是否让出通道级管线统计。
    GpuBladeTimingQueries,       // 渲染世界固定查询资源。
};

use self::{
    decode::decode_gpu_grass_timing, // 映射回调使用纯解码模块恢复分内容查询数据。
    resolve_plan::{TIMING_RESOLVE_BUFFER_BYTES, build_query_resolve_plan}, // GPU录制与CPU解码共享同一稀疏布局。
};
use super::{PIPELINE_STATISTICS_QUERY_COUNT, TIMESTAMP_QUERY_COUNT}; // 初始化固定设备查询集。

const TIMING_READBACK_SLOT_COUNT: usize = 3; // 三缓冲允许GPU解析、CPU映射和下一次采样重叠。
const TIMING_READBACK_FRAME_INTERVAL: u64 = 4; // 十五赫兹左右足够F3观察且显著降低驱动映射频率。
const SLOT_FREE: u8 = 0; // staging槽可被当前帧回读节点占用。
const SLOT_COPY_RECORDED: u8 = 1; // 上一帧已经录入GPU解析和拷贝命令。
const SLOT_MAPPING: u8 = 2; // CPU异步映射已发起，完成前禁止再次作为COPY_DST。
const SLOT_RESERVED: u8 = 3; // 当前根图节点正在为该槽录入命令。

/// 只在设备支持且没有 Bevy 外层通道统计时启用草地自己的管线统计查询。
fn should_enable_pipeline_statistics(
    device_supported: bool, // 当前 RenderDevice 是否启用了 wgpu 管线统计能力。
    reserved_for_render_diagnostics: bool, // Bevy 全局渲染诊断是否需要独占同一通道的唯一活动查询。
) -> bool {
    device_supported && !reserved_for_render_diagnostics // 任何外层统计存在时都只保留不冲突的草地时间戳。
}

/// 一个在GPU解析、CPU映射与复用之间独立转换的固定槽位。
struct GpuBladeTimingReadbackSlot {
    buffer: Buffer,            // 只拥有COPY_DST与MAP_READ用途，不参与正式渲染资源绑定。
    state: AtomicU8,           // FREE→RESERVED→COPY_RECORDED→MAPPING→FREE保证不覆写映射中内存。
    write_mask: AtomicU64,     // 回调只解析本样本真正写入的四十项时间戳与十二项统计查询槽。
    sequence: AtomicU64,       // 并发回调按提交序号拒绝旧样本覆盖新样本。
    resolved_bytes: AtomicU64, // 只映射当前稀疏计划实际复制的对齐前缀，不触碰未初始化尾部。
}

/// 渲染世界持有的固定三缓冲环。
#[derive(Resource)]
struct GpuBladeTimingReadbackRing {
    slots: Vec<Arc<GpuBladeTimingReadbackSlot>>, // Arc让map_async回调安全持有槽位至解除映射。
    cursor: AtomicUsize,                         // 每次从下一个槽开始尝试，避免长期偏向槽零。
}

impl GpuBladeTimingReadbackRing {
    /// 尝试占用一个既不在GPU使用也未映射的槽位。
    fn try_reserve(&self) -> Option<Arc<GpuBladeTimingReadbackSlot>> {
        if self.slots.is_empty() {
            return None; // 当前后端完全不支持查询时不执行模零或创建无意义staging。
        }
        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % self.slots.len(); // 轮转本次无锁扫描起点。
        for offset in 0..self.slots.len() {
            let slot = &self.slots[(start + offset) % self.slots.len()]; // 最多检查三个固定槽位。
            if slot
                .state
                .compare_exchange(
                    SLOT_FREE,         // 只有完全释放的槽才允许新GPU拷贝。
                    SLOT_RESERVED,     // CAS成功后由当前回读节点独占。
                    Ordering::AcqRel,  // 获取上次unmap并发布本次占用。
                    Ordering::Acquire, // 失败后继续检查下一个槽位。
                )
                .is_ok()
            {
                return Some(Arc::clone(slot)); // 返回独占槽的强引用供命令录入和回调共享。
            }
        }
        None // GPU或映射积压超过三槽时由调用方丢弃本次F3样本。
    }
}

/// 最新完整计时样本和累计诊断故障的跨回调共享状态。
#[derive(Resource, Clone, Default)]
struct GpuBladeTimingShared {
    latest: Arc<Mutex<GpuBladeGpuTimingStats>>, // 回调发布和ExtractSchedule复制各锁一次小结构。
    next_sequence: Arc<AtomicU64>,              // 录入解析命令时分配单调序号，与回调完成顺序解耦。
    dropped_samples: Arc<AtomicU64>,            // 回读槽全忙时只增加计数，不阻塞GPU或CPU。
    scene_errors: Arc<AtomicU64>,                 // 映射失败时保留上一份有效样本并累计错误。
    render_frames: Arc<AtomicU64>,              // 控制每四帧才执行一次稀疏查询诊断解析。
}

impl GpuBladeTimingShared {
    /// 发布一份已解析样本，较旧并发回调不得让F3倒退。
    fn publish(&self, stats: GpuBladeGpuTimingStats) {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()); // 即使历史回调panic也恢复诊断通道。
        if stats.sample_sequence > latest.sample_sequence {
            *latest = stats; // 只接受提交序号更新的完整样本。
        }
    }

    /// 生成带最新累计故障计数的主世界快照。
    fn snapshot(&self) -> GpuBladeGpuTimingStats {
        let mut snapshot = *self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()); // 复制固定值类型，不访问GPU缓冲。
        snapshot.dropped_samples = self.dropped_samples.load(Ordering::Relaxed); // 丢样累计值可比有效数据样本更新。
        snapshot.scene_errors = self.scene_errors.load(Ordering::Relaxed); // 映射错误同样实时发布。
        snapshot // 返回F3可以直接读取的不可变快照。
    }
}

/// 所有玩法相机完成后解析查询并拷贝到一个可用staging槽。
fn copy_gpu_grass_timing_readback(
    mut render_context: RenderContext, // 复用当前帧根命令编码器录入query resolve与小缓冲拷贝。
    queries: Option<Res<GpuBladeTimingQueries>>, // RenderStartup尚未创建查询资源时允许安全快返。
    ring: Option<Res<GpuBladeTimingReadbackRing>>, // 设备资源建立前不访问尚不存在的回读环。
    shared: Res<GpuBladeTimingShared>, // 取得采样节流、序号和累计故障状态。
) {
    let Some(queries) = queries else {
        return; // RenderStartup尚未创建查询资源时安全跳过。
    };
    let write_mask = queries.write_mask.swap(0, Ordering::AcqRel); // 每个渲染帧都清零，让下一帧可以重写固定槽位。
    let render_frame = shared.render_frames.fetch_add(1, Ordering::Relaxed); // 根图每运行一次推进一个诊断帧。
    if !render_frame.is_multiple_of(TIMING_READBACK_FRAME_INTERVAL) || write_mask == 0 {
        return; // 非采样帧或没有任何草命令时不解析、不拷贝也不映射。
    }
    let Some(resolve_buffer) = queries.resolve_buffer.as_ref() else {
        return; // 后端完全不支持查询时保持明确不可用状态。
    };
    let Some(ring) = ring else {
        return; // 设备初始化尚未建立固定槽时保持明确不可用状态。
    };
    let Some(slot) = ring.try_reserve() else {
        shared.dropped_samples.fetch_add(1, Ordering::Relaxed); // 所有槽繁忙时只丢弃F3样本。
        return; // 禁止同步poll或等待GPU腾出槽位。
    };
    let resolve_plan = build_query_resolve_plan(write_mask); // 按真实写入位切开所有空洞，禁止Vulkan等待未写查询。
    if let Some(query_set) = queries.timestamp_query_set.as_ref() {
        for run in resolve_plan.timestamp_runs() {
            render_context.command_encoder().resolve_query_set(
                query_set,                      // 四十个固定时间戳槽的设备查询集。
                run.start_query..run.end_query, // 当前范围内部每个查询都已在本帧真实写入。
                resolve_buffer,                 // 不可映射QUERY_RESOLVE缓冲接收原始u64 tick。
                run.destination_offset,         // 每个稀疏连续段使用独立二百五十六字节对齐区域。
            ); // 完成一段无空洞时间戳解析命令录入。
        }
    }
    if let Some(query_set) = queries.pipeline_statistics_query_set.as_ref() {
        for run in resolve_plan.pipeline_statistics_runs() {
            render_context.command_encoder().resolve_query_set(
                query_set,                      // 四类内容近中远十二项固定管线统计查询集。
                run.start_query..run.end_query, // 当前范围只包含真实开始并结束的统计查询。
                resolve_buffer,                 // 与时间戳共用同一持久小解析缓冲。
                run.destination_offset,         // 从上一对齐段之后写入，绝不覆盖时间戳。
            ); // 完成一段无空洞管线统计解析命令录入。
        }
    }
    render_context.command_encoder().copy_buffer_to_buffer(
        resolve_buffer,              // 查询结果已在同一GPU命令流内解析完成。
        0,                           // 稀疏解析布局仍从字节零开始紧凑排列对齐段。
        &slot.buffer,                // 当前独占的MAP_READ staging槽。
        0,                           // staging使用完全相同的稀疏布局便于确定性解码。
        resolve_plan.resolved_bytes, // 只拷贝本帧真实查询段占用的对齐前缀。
    ); // 完成GPU端解析结果到异步回读槽的拷贝。
    let sequence = shared.next_sequence.fetch_add(1, Ordering::Relaxed) + 1; // 为本次已录入拷贝分配单调样本序号。
    slot.write_mask.store(write_mask, Ordering::Relaxed); // 回调只读取当前帧真实写入的查询项。
    slot.sequence.store(sequence, Ordering::Relaxed); // 发布该槽对应的提交序号。
    slot.resolved_bytes
        .store(resolve_plan.resolved_bytes, Ordering::Relaxed); // 映射阶段只读取本次GPU实际复制的对齐前缀。
    slot.state.store(SLOT_COPY_RECORDED, Ordering::Release); // 下一帧ExtractSchedule可以发起非阻塞映射。
    // 当前帧草地诊断录入完成，根图继续提交命令缓冲。
}

/// RenderStartup按当前设备真实能力创建固定查询、解析缓冲和三个回读槽。
fn init_gpu_grass_timing_resources(
    mut commands: Commands,            // 向渲染世界插入查询资源与固定三缓冲环。
    render_device: Res<RenderDevice>,  // 查询当前启用特性并创建GPU对象。
    render_queue: Res<RenderQueue>,    // 取得时间戳tick到纳秒的设备换算周期。
    shared: Res<GpuBladeTimingShared>, // 立即发布支持状态，首份GPU样本前F3也能说明原因。
    compatibility: Res<GpuBladeTimingCompatibility>, // 读取插件finish阶段确定的外层统计独占状态。
) {
    let features = render_device.features(); // 只依据Bevy实际启用的设备特性，绝不假设显卡型号能力。
    let timestamp_supported = features.contains(Features::TIMESTAMP_QUERY)
        && features.contains(Features::TIMESTAMP_QUERY_INSIDE_PASSES); // 本实现所有边界都位于Compute或RenderPass内部。
    let pipeline_statistics_supported = should_enable_pipeline_statistics(
        features.contains(Features::PIPELINE_STATISTICS_QUERY), // Vulkan与DX12通常提供真实着色器调用量能力。
        compatibility.reserve_pipeline_statistics_for_render_diagnostics, // Solari诊断启用时避免嵌套同一通道的统计查询。
    ); // 只把本插件当前可安全使用的统计能力发布给F3。
    let timestamp_query_set = timestamp_supported.then(|| {
        render_device
            .wgpu_device()
            .create_query_set(&QuerySetDescriptor {
                label: Some("gpu_grass_stage_timestamps"), // GPU调试器中明确识别草地阶段时间戳。
                ty: QueryType::Timestamp,                  // 每槽返回一个u64设备tick。
                count: TIMESTAMP_QUERY_COUNT,              // 四类内容共四十槽，无运行时分配或增长。
            }) // 完成草地时间戳查询集创建。
    });
    let pipeline_statistics_query_set = pipeline_statistics_supported.then(|| {
        render_device
            .wgpu_device()
            .create_query_set(&QuerySetDescriptor {
                label: Some("gpu_grass_profile_lod_pipeline_statistics"), // GPU调试器中明确识别内容与近中远统计。
                ty: QueryType::PipelineStatistics(PipelineStatisticsTypes::all()), // 保留标准五项以维持固定40字节ABI。
                count: PIPELINE_STATISTICS_QUERY_COUNT, // 四类内容近中远各一个槽，共十二项。
            }) // 完成草地管线统计查询集创建。
    });
    let any_query_supported = timestamp_supported || pipeline_statistics_supported; // 至少一种查询存在才需要解析和回读缓冲。
    let resolve_buffer = any_query_supported.then(|| {
        render_device.create_buffer(&BufferDescriptor {
            label: Some("gpu_grass_timing_resolve"), // 查询只能先解析到不可映射GPU缓冲。
            size: TIMING_RESOLVE_BUFFER_BYTES,       // 固定最坏容量覆盖二十六个稀疏查询对齐段。
            usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC, // 只由resolve写入并拷贝到staging。
            mapped_at_creation: false, // 正式解析缓冲永不映射到CPU。
        }) // 完成共享查询解析缓冲创建。
    });
    let slots = if any_query_supported {
        (0..TIMING_READBACK_SLOT_COUNT)
            .map(|_| {
                Arc::new(GpuBladeTimingReadbackSlot {
                    buffer: render_device.create_buffer(&BufferDescriptor {
                        label: Some("gpu_grass_timing_readback"), // 三个槽使用同一可识别标签。
                        size: TIMING_RESOLVE_BUFFER_BYTES, // 每槽六千六百五十六字节，常驻总量十九点五KiB。
                        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ, // GPU拷贝完成后才允许CPU异步读取。
                        mapped_at_creation: false, // 首帧同样走统一map_async状态机。
                    }),
                    state: AtomicU8::new(SLOT_FREE), // 启动时三个槽均可立即接收诊断拷贝。
                    write_mask: AtomicU64::new(0),   // 尚无有效查询结果。
                    sequence: AtomicU64::new(0),     // 零序号不会覆盖任何有效样本。
                    resolved_bytes: AtomicU64::new(0), // 尚未录入任何稀疏查询解析与拷贝计划。
                }) // 完成单个固定回读槽创建。
            })
            .collect::<Vec<_>>() // 收集三个可并行在途的槽位。
    } else {
        Vec::new() // 无查询能力时不浪费任何staging缓冲。
    };
    {
        let mut latest = shared
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()); // 极短锁内只写两项设备能力标记。
        latest.timestamp_supported = timestamp_supported; // F3区分时间戳不支持与尚未采到样本。
        latest.pipeline_statistics_supported = pipeline_statistics_supported; // F3区分统计不支持与零片元调用。
    }
    commands.insert_resource(GpuBladeTimingQueries {
        timestamp_query_set,           // 支持时持有四十槽时间戳查询集，否则为None。
        pipeline_statistics_query_set, // 支持时持有十二槽统计查询集，否则为None。
        resolve_buffer,                // 至少一种查询存在时创建固定解析缓冲。
        timestamp_period_ns: render_queue.get_timestamp_period(), // 保存当前队列tick到纳秒换算值。
        write_mask: AtomicU64::new(0), // 首帧等待Compute和draw声明真实写入槽。
    }); // 完成渲染世界查询资源插入。
    commands.insert_resource(GpuBladeTimingReadbackRing {
        slots,                       // 支持查询时三个固定槽，不支持时为空集合。
        cursor: AtomicUsize::new(0), // 首次从槽零开始轮转。
    }); // 完成异步回读环插入。
}

/// 帧首对已提交槽发起非阻塞映射，并发布最近完成样本到主世界。
fn begin_scenes_and_publish_gpu_grass_timing(
    mut main_world: ResMut<MainWorld>, // Bevy官方RenderApp到主世界资源写回入口。
    ring: Res<GpuBladeTimingReadbackRing>, // 扫描三个COPY_RECORDED槽。
    queries: Res<GpuBladeTimingQueries>, // 提供设备tick周期和长期支持状态。
    shared: Res<GpuBladeTimingShared>, // 回调共享发布容器与累计故障计数。
) {
    for slot in &ring.slots {
        if slot
            .state
            .compare_exchange(
                SLOT_COPY_RECORDED, // 只映射上一帧已提交GPU拷贝的槽。
                SLOT_MAPPING,       // 从现在起禁止回读节点再次选择该槽。
                Ordering::AcqRel,   // 获取节点发布的掩码和序号并发布映射占用。
                Ordering::Acquire,  // 非目标状态直接检查下一槽。
            )
            .is_err()
        {
            continue; // FREE、RESERVED或MAPPING槽都不发起重复映射。
        }
        let write_mask = slot.write_mask.load(Ordering::Relaxed); // 读取当前样本实际写入的固定查询槽。
        let sequence = slot.sequence.load(Ordering::Relaxed); // 读取拒绝旧回调覆盖的提交序号。
        let resolved_bytes = slot.resolved_bytes.load(Ordering::Relaxed); // 读取当前稀疏计划已经复制的有效对齐前缀。
        if resolved_bytes == 0 {
            slot.state.store(SLOT_FREE, Ordering::Release); // 损坏的空计划直接归还槽位，禁止发起零长度映射。
            continue; // 正常write_mask非零路径不会进入，仅保留防御性恢复。
        }
        let timestamp_period_ns = queries.timestamp_period_ns; // 值复制进回调后不再借用ECS资源。
        let timestamp_supported = queries.timestamp_query_set.is_some(); // 发布采样时的真实时间戳支持状态。
        let pipeline_statistics_supported = queries.pipeline_statistics_query_set.is_some(); // 发布采样时的真实统计支持状态。
        let callback_slot = Arc::clone(slot); // 回调完成后负责将槽位归还FREE状态。
        let callback_buffer = slot.buffer.clone(); // 强句柄保证映射与解析期间缓冲存活。
        let map_request_buffer = callback_buffer.clone(); // 单独句柄发起map并把另一份移入静态回调。
        let callback_shared = (*shared).clone(); // 回调只持有Arc内部共享状态，不借用渲染世界。
        map_request_buffer
            .slice(0..resolved_bytes)
            .map_async(MapMode::Read, move |result| {
                if result.is_ok() {
                    let Ok(mapped) = callback_buffer.slice(0..resolved_bytes).get_mapped_range() else {
                        // 映射回调成功后，WGPU 30 的范围借用仍可能失败。
                        callback_buffer.unmap();
                        callback_shared.scene_errors.fetch_add(1, Ordering::Relaxed);
                        callback_slot.state.store(SLOT_FREE, Ordering::Release);
                        return;
                    };
                    let stats = decode_gpu_grass_timing(
                        &mapped,                       // 当前样本确定性稀疏查询布局。
                        write_mask,                    // 未写槽即使残留旧字节也绝不解析。
                        sequence,                      // 保留本次GPU提交顺序。
                        timestamp_period_ns,           // 把设备tick换算为毫秒。
                        timestamp_supported,           // 公开当前时间戳能力。
                        pipeline_statistics_supported, // 公开当前统计能力。
                    ); // 完成最近GPU草地分阶段样本解析。
                    drop(mapped); // WebGPU要求解除映射前释放所有映射视图。
                    callback_buffer.unmap(); // 归还缓冲为下一次COPY_DST使用状态。
                    callback_shared.publish(stats); // 仅当序号更新时替换主世界候选快照。
                } else {
                    callback_shared.scene_errors.fetch_add(1, Ordering::Relaxed); // 映射失败只影响F3诊断。
                }
                callback_slot.state.store(SLOT_FREE, Ordering::Release); // 成功或失败后均归还固定槽。
            }); // map_async立即返回，本系统不调用同步poll或等待。
    }
    let snapshot = shared.snapshot(); // 只发布已经由异步回调完成的最近样本。
    if let Some(mut stats) = main_world.get_resource_mut::<GpuBladeGpuTimingStats>() {
        *stats = snapshot; // 主世界资源已存在时原地覆盖固定值类型。
    } else {
        main_world.insert_resource(snapshot); // 独立RenderApp装配时允许首次自动创建资源。
    }
}

/// 安装GPU草地查询、相机后解析、异步映射与主世界发布链路。
pub(crate) fn install_gpu_grass_timing(render_app: &mut SubApp) {
    render_app
        .init_resource::<GpuBladeTimingShared>() // 无查询能力时也向F3发布明确支持标记。
        .add_systems(RenderStartup, init_gpu_grass_timing_resources) // 渲染设备建立后一次性创建固定查询与十九点五KiB staging。
        .add_systems(ExtractSchedule, begin_scenes_and_publish_gpu_grass_timing) // 帧首发起非阻塞映射并发布已完成样本。
        .add_systems(
            RenderGraph,
            copy_gpu_grass_timing_readback
                .in_set(RenderGraphSystems::Render)
                .after(camera_driver),
        ); // 相机后、Submit集合前录入解析；下一帧Extract发起映射时该槽已经真实提交。
}

#[cfg(test)]
mod tests {
    // 纯布尔回归无需创建 GPU 设备即可锁定查询所有权规则。
    use super::should_enable_pipeline_statistics; // 直接验证 RenderStartup 使用的唯一决策函数。

    #[test]
    fn bevy_render_diagnostics_reserves_the_only_pipeline_statistics_scope() {
        // 防止再次生成 wgpu 嵌套查询校验错误。
        assert!(should_enable_pipeline_statistics(true, false)); // 没有外层诊断时保留既有草地着色器调用量统计。
        assert!(!should_enable_pipeline_statistics(true, true)); // Bevy 外层统计存在时草地必须让出统计查询。
        assert!(!should_enable_pipeline_statistics(false, false)); // 设备不支持时不得创建伪查询或CPU替代值。
    } // 完成三种设备与插件组合的所有权断言。
}
