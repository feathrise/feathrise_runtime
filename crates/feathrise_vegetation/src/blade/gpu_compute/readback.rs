//! GPU草地间接状态的三缓冲异步回读与F3真实存活统计。
//! 诊断链路只拷贝每个草窗80字节；所有槽位都忙时丢弃样本，绝不等待GPU。

use std::sync::{
    // 回读映射回调可在渲染线程之外完成。
    Arc,   // 让渲染世界、RenderGraph节点和异步回调共享槽位生命周期。
    Mutex, // 每次完成只锁一份几十字节统计，不在主渲染热路径等待GPU。
    atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering}, // 槽位状态机与累计诊断均使用无锁原子量。
}; // 完成跨线程基础类型导入。

use bevy::app::SubApp; // 把回读资源、系统和根图调度安装到RenderApp。
use bevy::prelude::*; // Commands、Res和Resource等ECS基础类型。
use bevy::render::render_resource::{
    // 创建可映射staging缓冲并记录GPU拷贝。
    Buffer,
    BufferDescriptor,
    BufferUsages,
    MapMode, // `draw_state` COPY_SRC到MAP_READ槽位的最小API集合。
}; // 完成GPU资源类型导入。
use bevy::render::renderer::{RenderContext, RenderDevice, RenderGraph}; // 分配三缓冲并从根调度命令编码器录入拷贝。
use bevy::render::{ExtractSchedule, MainWorld, RenderStartup}; // 下一帧发起映射并将已完成样本发布回主世界。

use crate::blade::VegetationRenderGraphSet; // 诊断拷贝固定排在Compute之后、所有相机之前。
use crate::components::{GpuBladePatch, GpuBladePreparedBuffers}; // 只回读当前活动草窗已准备的八十字节状态。
use crate::constants::{
    // 回读字段和状态步长必须与编码端引用同一份WGSL ABI常量。
    GPU_GRASS_DRAW_STATE_ACTIVE_TILES_OFFSET, // 活跃粗瓦片数量字段偏移。
    GPU_GRASS_DRAW_STATE_BYTES,               // 每个草窗聚合状态固定八十字节。
    GPU_GRASS_DRAW_STATE_CANDIDATE_COUNT_OFFSET, // 候选实例数量字段偏移。
    GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET,     // 紧凑实例缓冲容量字段偏移。
    GPU_GRASS_DRAW_STATE_DENSITY_SURVIVORS_OFFSET, // 密度阶段存活数量字段偏移。
    GPU_GRASS_DRAW_STATE_DISTANCE_SURVIVORS_OFFSET, // 距离阶段存活数量字段偏移。
    GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET,  // 当前原型真实索引数量字段偏移。
    GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET, // 最终实绘实例数量字段偏移。
    GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET,    // 近中远LOD编号字段偏移。
    GPU_GRASS_DRAW_STATE_OVERFLOW_INSTANCES_OFFSET, // 容量溢出实例数量字段偏移。
    GPU_GRASS_DRAW_STATE_SCANNED_CANDIDATES_OFFSET, // 实际扫描候选数量字段偏移。
    GPU_GRASS_DRAW_STATE_SURFACE_SURVIVORS_OFFSET, // 合法地表存活数量字段偏移。
    GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET,  // 粗瓦片总数量字段偏移。
    GPU_GRASS_DRAW_STATE_VEGETATION_PROFILE_OFFSET, // 普通草、麦田等稳定内容编号字段偏移。
    GPU_GRASS_DRAW_STATE_VERTEX_COUNT_OFFSET, // 当前原型真实顶点数量字段偏移。
};
use crate::diagnostics::GpuBladeReadbackConfig; // 正式默认四帧一次，运动烟雾可逐帧尝试采样。
use crate::resources::{GpuBladeGpuReadbackStats, GpuBladeVegetationReadbackStats}; // 向主世界F3发布聚合与分内容GPU样本。
use crate::types::{GPU_GRASS_LOD_CAPACITY, GPU_GRASS_PROFILE_CAPACITY}; // draw state中的两位profile和两位LOD必须落在固定四乘四诊断范围。

const READBACK_SLOT_COUNT: usize = 3; // 三缓冲允许GPU和CPU映射存在两帧以上重叠。
const MAX_READBACK_WINDOWS: usize = 32; // 每槽可容纳常见多profile窗口及后续扩展，总大小仍只有2.5KiB。
const SLOT_FREE: u8 = 0; // 槽位可被当前RenderGraph帧选中。
const SLOT_COPY_RECORDED: u8 = 1; // 拷贝命令已经录入上一帧命令缓冲。
const SLOT_MAPPING: u8 = 2; // `map_async`已发起，GPU完成前不得复用该槽位。
const SLOT_RESERVED: u8 = 3; // 当前图节点正在录入对该槽的拷贝。
const READBACK_BUFFER_BYTES: u64 = GPU_GRASS_DRAW_STATE_BYTES * MAX_READBACK_WINDOWS as u64; // 单槽按固定草窗数一次分配。

/// 一个可独立在GPU拷贝、异步映射和再利用之间转换的槽位。
struct GpuBladeReadbackSlot {
    buffer: Buffer,               // 只有COPY_DST与MAP_READ用途，永不作为渲染或计算输入。
    state: AtomicU8,              // 通过FREE→RESERVED→COPY_RECORDED→MAPPING→FREE禁止映射中覆写。
    copied_windows: AtomicU32,    // 回调只解析本帧确实拷贝的连续80字节记录。
    truncated_windows: AtomicU32, // 超过固定槽容量的草窗显式暴露，不让F3误以为是零存活。
    sequence: AtomicU64,          // 并发回调用序号拒绝较旧槽位覆盖较新统计。
    source_revision: AtomicU64, // 与本槽GPU copy一起冻结的作者输入身份。
}

/// 渲染世界持有的固定三缓冲环。
#[derive(Resource)]
struct GpuBladeReadbackRing {
    slots: Vec<Arc<GpuBladeReadbackSlot>>, // Arc保证映射回调完成前GPU缓冲不会被释放。
    cursor: AtomicUsize,                   // 每帧从下一个槽开始寻找，避免总是偏向槽零。
}

impl GpuBladeReadbackRing {
    /// 尝试取得一个不在GPU或映射中的槽位。
    fn try_reserve(&self) -> Option<Arc<GpuBladeReadbackSlot>> {
        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % self.slots.len(); // 轮转起点使三槽获得均匀复用。
        for offset in 0..self.slots.len() {
            // 最多扫描三个原子状态，没有锁或等待。
            let slot = &self.slots[(start + offset) % self.slots.len()]; // 按环形下标读取候选槽位。
            if slot // 只有完全FREE的槽位才能被当前帧占用。
                .state // 读取槽位原子状态。
                .compare_exchange(
                    // 一次CAS避免回调恰好释放时的竞争。
                    SLOT_FREE,         // 预期槽位已经解除映射。
                    SLOT_RESERVED,     // 立即标记为当前节点私有。
                    Ordering::AcqRel,  // 同时看到上一次unmap并发布本次占用。
                    Ordering::Acquire, // 失败后继续查看下一槽。
                ) // 完成槽位状态转换尝试。
                .is_ok()
            // CAS成功表示该槽已由当前帧独占。
            {
                return Some(Arc::clone(slot)); // 返回可移入节点和后续回调的强引用。
            }
        }
        None // GPU延迟超过三槽时由调用方丢弃诊断样本。
    }
}

/// 最新完整样本与累计通道故障的跨回调共享状态。
#[derive(Resource, Clone, Default)]
struct GpuBladeReadbackShared {
    latest: Arc<Mutex<GpuBladeGpuReadbackStats>>, // 只有映射完成和ExtractSchedule各锁一次小结构。
    next_sequence: Arc<AtomicU64>,                // 序号在录入GPU拷贝时分配，与回调完成顺序无关。
    dropped_samples: Arc<AtomicU64>,              // 槽位都忙时只增加诊断计数而不卡帧。
    map_errors: Arc<AtomicU64>,                   // 映射失败后释放槽位并保留上一份有效数据。
    render_frames: Arc<AtomicU64>, // 按提取配置计数；正式每四帧，运动烟雾逐帧尝试诊断拷贝。
}

impl GpuBladeReadbackShared {
    /// 发布一份已解析样本，较旧并发回调不得倒退F3。
    fn publish(&self, stats: GpuBladeGpuReadbackStats) {
        let mut latest = self // 对几十字节值类型执行极短临界区。
            .latest // 取得共享最新样本容器。
            .lock() // 锁只竞争CPU回调与下一帧发布，不会等待GPU。
            .unwrap_or_else(|poisoned| poisoned.into_inner()); // 即使过去回调panic也恢复诊断通道。
        if stats.sample_sequence > latest.sample_sequence {
            // 三槽可能不按提交顺序完成映射。
            *latest = stats; // 只接受序号更新的完整样本。
        }
    }

    /// 生成对主世界不可变发布的快照。
    fn snapshot(&self) -> GpuBladeGpuReadbackStats {
        let mut snapshot = *self // 先复制最新完整GPU样本。
            .latest // 取得共享快照容器。
            .lock() // 只读极短临界区，不访问GPU缓冲。
            .unwrap_or_else(|poisoned| poisoned.into_inner()); // 保持诊断系统可恢复。
        snapshot.dropped_samples = self.dropped_samples.load(Ordering::Relaxed); // 累计值可比数据样本更新。
        snapshot.map_errors = self.map_errors.load(Ordering::Relaxed); // 发布最新映射错误计数。
        snapshot // 返回F3可以直接复制的值类型。
    }
}

/// Compute收尾后把活动草窗状态拷贝到一个可用staging槽。
fn copy_gpu_grass_readback(
    mut render_context: RenderContext, // 复用当前帧根命令编码器录入小块拷贝。
    grass_windows: Query<(&GpuBladePatch, &GpuBladePreparedBuffers)>, // 票据取实际完成输出，不能借新ECS输入冒充旧驻留缓冲。
    ring: Option<Res<GpuBladeReadbackRing>>, // RenderStartup尚未创建三缓冲时允许安全快返。
    shared: Res<GpuBladeReadbackShared>,     // 读取序号、采样节流和丢样计数。
    config: Option<Res<GpuBladeReadbackConfig>>, // 首帧尚未提取配置时使用正式默认采样间隔。
) {
    let Some(ring) = ring else {
        return; // RenderStartup尚未创建staging缓冲时安全跳过。
    };
    let render_frame = shared.render_frames.fetch_add(1, Ordering::Relaxed); // 每次根图运行只执行一次无锁帧计数。
    let frame_interval = config
        .as_deref()
        .copied()
        .unwrap_or_default()
        .frame_interval(); // 无提取配置的首帧仍使用正式四帧默认值。
    if !render_frame.is_multiple_of(frame_interval) {
        return; // 非采样帧完全不录入COPY命令，也不占用三缓冲槽位。
    }
    let Some(slot) = ring.try_reserve() else {
        shared.dropped_samples.fetch_add(1, Ordering::Relaxed); // 槽都忙时只丢弃F3样本，草地计算与绘制照常继续。
        return; // 明确禁止同步poll、map或等待。
    };
    let sequence = shared.next_sequence.fetch_add(1, Ordering::Relaxed) + 1; // 为本次GPU拷贝分配单调序号。
    let mut copied_windows = 0_u32; // 记录已紧密排列在staging前缀的状态数。
    let mut truncated_windows = 0_u32; // 记录超过固定诊断容量的活动草窗数。
    let mut source_revision = None;
    for (patch, prepared) in &grass_windows {
        // Bevy调度负责更新查询原型；系统不分配Vec，直接从渲染ECS录入拷贝。
        if patch.instance_count() == 0 {
            continue; // 植被高度或密度输入未就绪而被钳制的草窗不能回读上一帧旧状态。
        }
        if copied_windows as usize >= MAX_READBACK_WINDOWS {
            truncated_windows = truncated_windows.saturating_add(1); // 额外草窗只影响诊断完整性，不影响实际draw。
            continue; // 防止写越staging槽位尾部。
        }
        source_revision = merge_source_revision(source_revision, Some(prepared.source_revision), prepared.output_valid);
        let destination_offset = u64::from(copied_windows) * GPU_GRASS_DRAW_STATE_BYTES; // 每个状态按严格80字节连续排列。
        render_context.command_encoder().copy_buffer_to_buffer(
            // 只录入GPU端小块拷贝，不发起CPU映射。
            &prepared.draw_state, // Compute和Finalize已完成写入的COPY_SRC状态。
            0,                    // 每个草窗状态都从自身缓冲字节零开始。
            &slot.buffer,         // 当前独占的MAP_READ staging槽位。
            destination_offset,   // 不同草窗写入不重叠的80字节片段。
            GPU_GRASS_DRAW_STATE_BYTES, // 间接参数、三阶段计数和容量诊断一次拷全。
        ); // 完成当前草窗GPU状态拷贝录入。
        copied_windows = copied_windows.saturating_add(1); // 下一草窗紧接当前状态写入。
    }
    slot.copied_windows.store(copied_windows, Ordering::Relaxed); // 映射回调只解析有效staging前缀。
    slot.truncated_windows
        .store(truncated_windows, Ordering::Relaxed); // 保留本次诊断截断数。
    slot.sequence.store(sequence, Ordering::Relaxed); // 发布与当前拷贝对应的序号。
    slot.source_revision.store(source_revision.unwrap_or(0), Ordering::Relaxed);
    slot.state.store(SLOT_COPY_RECORDED, Ordering::Release); // 下一帧ExtractSchedule看到该状态后才发起map_async。
    // 诊断拷贝录入完成，相机驱动可继续消费同一间接状态。
}

/// 多窗必须同一完整输入修订；任一缺票据/混合/未就绪都清零，不能拿旧GPU计数确认新草稿。
fn merge_source_revision(previous: Option<u64>, revision: Option<u64>, output_valid: bool) -> Option<u64> {
    let revision = revision.filter(|_|output_valid).unwrap_or(0);
    Some(previous.map_or(revision,|previous|if previous == revision {revision} else {0}))
}

#[cfg(test)]
mod source_revision_tests {
    use super::merge_source_revision;
    #[test]
    fn current_complete_same_revision_is_required_for_gpu_ack() {
        assert_eq!(merge_source_revision(None,Some(7),true),Some(7));
        assert_eq!(merge_source_revision(Some(7),Some(7),true),Some(7));
        assert_eq!(merge_source_revision(Some(7),Some(8),true),Some(0));
        assert_eq!(merge_source_revision(Some(7),Some(7),false),Some(0));
        assert_eq!(merge_source_revision(Some(7),None,true),Some(0));
        assert_eq!(merge_source_revision(Some(0),Some(7),true),Some(0));
    }
}

/// RenderStartup一次性创建三个小型映射槽位。
fn init_gpu_grass_readback_ring(mut commands: Commands, render_device: Res<RenderDevice>) {
    let slots = (0..READBACK_SLOT_COUNT) // 固定三槽避免运行时增长和显存抖动。
        .map(|_| {
            Arc::new(GpuBladeReadbackSlot {
                buffer: render_device.create_buffer(&BufferDescriptor {
                    label: Some("gpu_grass_draw_state_readback"), // 图形调试器中明确识别草地诊断槽位。
                    size: READBACK_BUFFER_BYTES, // 十二窗只用960B，预留到32窗也仅2.5KiB。
                    usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ, // WebGPU规范要求先拷贝到staging再异步映射。
                    mapped_at_creation: false, // 首帧也通过map_async状态机使用同一路径。
                }),
                state: AtomicU8::new(SLOT_FREE), // 启动时所有槽均可立即录入GPU拷贝。
                copied_windows: AtomicU32::new(0), // 未录入前没有有效状态记录。
                truncated_windows: AtomicU32::new(0), // 启动时不存在诊断截断。
                sequence: AtomicU64::new(0),     // 零序号不会覆盖任何有效样本。
                source_revision: AtomicU64::new(0),
            }) // 完成单个回读槽位创建。
        }) // 为三个槽位执行相同一次性分配。
        .collect::<Vec<_>>(); // 将固定小集合保存为环形索引集。
    commands.insert_resource(GpuBladeReadbackRing {
        slots,                       // 持有三个可并行在途槽位。
        cursor: AtomicUsize::new(0), // 首帧从槽零开始，之后环形轮转。
    }); // 将三缓冲插入渲染世界。
}

/// 帧首对上一帧已提交槽发起非阻塞映射，同时发布已完成样本。
fn begin_maps_and_publish_gpu_grass_stats(
    mut main_world: ResMut<MainWorld>, // 只在ExtractSchedule通过Bevy官方跨世界入口发布F3资源。
    ring: Res<GpuBladeReadbackRing>,   // 扫描三个槽的COPY_RECORDED状态。
    shared: Res<GpuBladeReadbackShared>, // 为回调提供共享发布容器与映射错误计数。
) {
    for slot in &ring.slots {
        // 三槽中可能同时有多个GPU完成顺序不同的样本。
        if slot // 只有上一帧RenderGraph已经录入的槽可以发起映射。
            .state // 读取当前槽位状态。
            .compare_exchange(
                // CAS避免回调释放与帧首扫描交叉。
                SLOT_COPY_RECORDED, // 上一帧命令缓冲已提交。
                SLOT_MAPPING,       // 从现在起禁止RenderGraph再次选中该槽。
                Ordering::AcqRel,   // 获取节点发布的元数据并发布映射占用。
                Ordering::Acquire,  // 不符合时直接检查下一槽。
            ) // 完成槽位映射状态转换。
            .is_err()
        // FREE、RESERVED或MAPPING都不发起重复映射。
        {
            continue; // 本槽当前无新GPU拷贝可读。
        }
        let copied_windows = slot.copied_windows.load(Ordering::Relaxed); // 读取staging有效记录数。
        let truncated_windows = slot.truncated_windows.load(Ordering::Relaxed); // 读取本样本诊断截断数。
        let sequence = slot.sequence.load(Ordering::Relaxed); // 读取用于拒绝过时回调的序号。
        let source_revision = slot.source_revision.load(Ordering::Relaxed);
        let callback_slot = Arc::clone(slot); // 回调完成unmap后需将该槽设回FREE。
        let callback_buffer = slot.buffer.clone(); // 强句柄保证异步映射期间缓冲存活。
        let map_request_buffer = callback_buffer.clone(); // 单独句柄发起map，原句柄移入`'static`回调。
        let callback_shared = (*shared).clone(); // 回调只持有Arc内部状态，不借用ECS资源。
        map_request_buffer // 对整个2.5KiB槽位发起异步读映射。
            .slice(..) // 映射范围与缓冲大小一致且满足WebGPU对齐。
            .map_async(MapMode::Read, move |result| {
                if result.is_ok() {
                    // GPU已完成对该槽的所有拷贝后才会进入成功分支。
                    let Ok(mapped) = callback_buffer.slice(..).get_mapped_range() else {
                        // 映射回调成功后，WGPU 30 的范围借用仍可能失败。
                        callback_buffer.unmap();
                        callback_shared.map_errors.fetch_add(1, Ordering::Relaxed);
                        callback_slot.state.store(SLOT_FREE, Ordering::Release);
                        return;
                    };
                    let mut stats = decode_gpu_grass_stats(
                        // 仅解析本帧有效的80字节前缀。
                        &mapped,           // 读取staging小缓冲的映射字节。
                        copied_windows,    // 忽略槽位尾部上一次残留数据。
                        truncated_windows, // 把诊断容量不足显式放入快照。
                        sequence,          // 保留GPU拷贝提交顺序。
                    ); // 完成当前槽位GPU计数解析。
                    stats.source_revision = source_revision;
                    drop(mapped); // WebGPU要求在unmap前释放所有映射视图。
                    callback_buffer.unmap(); // 解除CPU映射后该缓冲才能再次作为COPY_DST。
                    callback_shared.publish(stats); // 序号较新时原子替换F3最新样本。
                } else {
                    callback_shared.map_errors.fetch_add(1, Ordering::Relaxed); // 失败只记录诊断，不丢弃上一份有效样本。
                }
                callback_slot.state.store(SLOT_FREE, Ordering::Release); // 成功unmap或失败结束后都归还三缓冲槽位。
            }); // `map_async`立即返回，帧首系统不调用任何同步poll。
    }

    let snapshot = shared.snapshot(); // 异步回调可能滞后数帧，这里只发布已完成的最新样本。
    if let Some(mut stats) = main_world.get_resource_mut::<GpuBladeGpuReadbackStats>() {
        *stats = snapshot; // 已存在时原地替换值类型快照。
    } else {
        main_world.insert_resource(snapshot); // 无RenderApp测试之外的首次Extract自动建立F3公开资源。
    }
}

/// 按WGSL `GrassDrawState`偏移解析并汇总多个草窗。
fn decode_gpu_grass_stats(
    bytes: &[u8],           // staging缓冲映射字节。
    copied_windows: u32,    // 实际拷贝的连续状态数。
    truncated_windows: u32, // 本样本未容纳的额外草窗数。
    sequence: u64,          // 样本提交序号。
) -> GpuBladeGpuReadbackStats {
    let available_windows = bytes.len() / GPU_GRASS_DRAW_STATE_BYTES as usize; // 损坏或短缓冲时不读越边界。
    let safe_windows = (copied_windows as usize).min(available_windows); // 双重上限保护所有后续偏移读取。
    let mut stats = GpuBladeGpuReadbackStats {
        sample_sequence: sequence,            // 每份完整解析样本保留提交顺序。
        sampled_windows: safe_windows as u32, // 只声明实际安全解析的草窗数。
        truncated_windows,                    // 保留诊断容量不足提示。
        ..default()                           // 各阶段计数在下方以u64饱和累加。
    };
    for window_index in 0..safe_windows {
        // 逐草窗读取固定80字节ABI。
        let base = window_index * GPU_GRASS_DRAW_STATE_BYTES as usize; // 定位当前草窗状态起始字节。
        let rendered_instances = u64::from(read_u32(
            bytes,                                             // 从当前映射槽位读取状态字段。
            base + GPU_GRASS_DRAW_STATE_INSTANCE_COUNT_OFFSET, // 定位当前圈实际间接实例数。
        )); // 完成最终实绘数量解码。
        let lod_index = read_u32(bytes, base + GPU_GRASS_DRAW_STATE_LOD_INDEX_OFFSET) as usize; // 读取近中远零至二级几何与材质档。
        let profile_index =
            read_u32(bytes, base + GPU_GRASS_DRAW_STATE_VEGETATION_PROFILE_OFFSET) as usize; // 读取普通草、麦田等稳定内容profile。
        let vertex_count = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_VERTEX_COUNT_OFFSET,
        )); // 读取当前窗口真实原型顶点数，禁止F3套用普通草常量。
        let index_count = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_INDEX_COUNT_OFFSET,
        )); // 索引数沿用间接绘制状态首字段，和真正draw完全一致。
        let overflow_instances = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_OVERFLOW_INSTANCES_OFFSET,
        )); // 定位容量钳制数量。
        let distance_survivors = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_DISTANCE_SURVIVORS_OFFSET,
        )); // 定位距离阶段存活数量。
        let density_survivors = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_DENSITY_SURVIVORS_OFFSET,
        )); // 定位密度阶段存活数量。
        let surface_survivors = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_SURFACE_SURVIVORS_OFFSET,
        )); // 定位合法地表最终存活数量。
        let compact_capacity =
            u64::from(read_u32(bytes, base + GPU_GRASS_DRAW_STATE_CAPACITY_OFFSET)); // 定位当前窗口二十字节紧凑输出容量。
        let candidate_count = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_CANDIDATE_COUNT_OFFSET,
        )); // 定位本窗口GPU候选上限。
        let total_tiles = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_TOTAL_TILES_OFFSET,
        )); // 定位当前窗口粗瓦片总数。
        let active_tiles = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_ACTIVE_TILES_OFFSET,
        )); // 定位GPU粗判后的活跃瓦片数量。
        let scanned_candidates = u64::from(read_u32(
            bytes,
            base + GPU_GRASS_DRAW_STATE_SCANNED_CANDIDATES_OFFSET,
        )); // 定位实际启动的细分类候选线程数。

        stats.rendered_instances = stats.rendered_instances.saturating_add(rendered_instances); // 汇总全部内容最终实绘量。
        if let Some(lod_instances) = stats.rendered_instances_by_lod.get_mut(lod_index) {
            *lod_instances = lod_instances.saturating_add(rendered_instances); // 分档实例数用于F3计算真实几何预算。
        } // 未定义LOD只进入总实绘量，不允许诊断数组越界。
        stats.overflow_instances = stats.overflow_instances.saturating_add(overflow_instances); // 汇总全部内容容量溢出诊断。
        stats.distance_survivors = stats.distance_survivors.saturating_add(distance_survivors); // 汇总全部内容距离阶段存活数量。
        stats.density_survivors = stats.density_survivors.saturating_add(density_survivors); // 汇总全部内容密度阶段存活数量。
        stats.surface_survivors = stats.surface_survivors.saturating_add(surface_survivors); // 汇总全部内容地表阶段存活数量。
        stats.compact_capacity = stats.compact_capacity.saturating_add(compact_capacity); // 汇总全部内容紧凑实例缓冲容量。
        stats.candidate_count = stats.candidate_count.saturating_add(candidate_count); // 汇总全部内容候选数量。
        stats.total_tiles = stats.total_tiles.saturating_add(total_tiles); // 汇总全部内容粗瓦片总数。
        stats.active_tiles = stats.active_tiles.saturating_add(active_tiles); // 汇总全部内容活跃瓦片数量。
        stats.scanned_candidates = stats.scanned_candidates.saturating_add(scanned_candidates); // 汇总全部内容实际扫描候选数量。

        if profile_index < GPU_GRASS_PROFILE_CAPACITY {
            aggregate_profile_window(
                &mut stats.by_profile[profile_index],
                lod_index,
                rendered_instances,
                vertex_count,
                index_count,
                overflow_instances,
                distance_survivors,
                density_survivors,
                surface_survivors,
                compact_capacity,
                candidate_count,
                total_tiles,
                active_tiles,
                scanned_candidates,
            ); // 只有已定义profile进入分内容统计，损坏编号仍保留在安全的聚合总计中。
        }
    }
    stats // 返回可跨世界发布的完整汇总。
}

/// 把一个窗口的真实GPU状态累加到对应叶片内容，不依赖普通草几何常量。
#[allow(clippy::too_many_arguments)]
fn aggregate_profile_window(
    profile: &mut GpuBladeVegetationReadbackStats,
    lod_index: usize,
    rendered_instances: u64,
    vertex_count: u64,
    index_count: u64,
    overflow_instances: u64,
    distance_survivors: u64,
    density_survivors: u64,
    surface_survivors: u64,
    compact_capacity: u64,
    candidate_count: u64,
    total_tiles: u64,
    active_tiles: u64,
    scanned_candidates: u64,
) {
    profile.sampled_windows = profile.sampled_windows.saturating_add(1); // 每个合法profile窗口只计一次。
    profile.rendered_instances = profile
        .rendered_instances
        .saturating_add(rendered_instances); // 累加当前内容真实间接实例。
    profile.overflow_instances = profile
        .overflow_instances
        .saturating_add(overflow_instances); // 当前内容容量不足不能被其他内容掩盖。
    profile.distance_survivors = profile
        .distance_survivors
        .saturating_add(distance_survivors);
    profile.density_survivors = profile.density_survivors.saturating_add(density_survivors);
    profile.surface_survivors = profile.surface_survivors.saturating_add(surface_survivors);
    profile.compact_capacity = profile.compact_capacity.saturating_add(compact_capacity);
    profile.candidate_count = profile.candidate_count.saturating_add(candidate_count);
    profile.total_tiles = profile.total_tiles.saturating_add(total_tiles);
    profile.active_tiles = profile.active_tiles.saturating_add(active_tiles);
    profile.scanned_candidates = profile
        .scanned_candidates
        .saturating_add(scanned_candidates);
    if lod_index < GPU_GRASS_LOD_CAPACITY {
        profile.rendered_instances_by_lod[lod_index] =
            profile.rendered_instances_by_lod[lod_index].saturating_add(rendered_instances); // 四级实例身份由窗口状态直接提供。
        profile.submitted_vertices_by_lod[lod_index] = profile.submitted_vertices_by_lod[lod_index]
            .saturating_add(rendered_instances.saturating_mul(vertex_count)); // 顶点预算使用当前GLB切片真实数量。
        profile.submitted_indices_by_lod[lod_index] = profile.submitted_indices_by_lod[lod_index]
            .saturating_add(rendered_instances.saturating_mul(index_count)); // 索引预算与间接draw状态完全同源。
    }
}

/// 从小端WGSL状态字节读取一个u32。
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let Some(raw) = bytes.get(offset..offset + 4) else {
        return 0; // 损坏映射长度时返回零而不让诊断回调panic。
    };
    u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) // WebGPU当前支持平台与CPU编码统一使用小端布局。
}

/// 安装GPU草地无阻塞异步回读链路。
pub(super) fn install_gpu_grass_readback(render_app: &mut SubApp) {
    render_app
        .init_resource::<GpuBladeReadbackShared>() // 无草地场景也保持稳定零值快照。
        .add_systems(RenderStartup, init_gpu_grass_readback_ring) // 渲染设备建立后一次性创建三个2.5KiB槽。
        .add_systems(ExtractSchedule, begin_maps_and_publish_gpu_grass_stats) // 帧首发起map_async并发布已完成样本。
        .add_systems(
            RenderGraph,
            copy_gpu_grass_readback.in_set(VegetationRenderGraphSet::DiagnosticReadback),
        ); // 每个采样帧最多录入一次全部内容窗口的状态拷贝，多相机不会重复。
}

#[cfg(test)]
mod tests;
