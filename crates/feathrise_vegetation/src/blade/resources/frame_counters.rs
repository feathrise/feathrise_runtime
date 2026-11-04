use std::sync::atomic::{AtomicU32, Ordering};

use bevy::prelude::Resource;

use crate::types::{GPU_GRASS_LOD_CAPACITY, GPU_GRASS_PROFILE_CAPACITY};

/// 上一个完整渲染帧的 GPU 草地流水线快照。 // F3按半秒读取一次，不做GPU像素或存活草株回读。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)] // 值类型便于从RenderApp一次复制回主世界并做单元测试。
pub struct GpuBladeRenderStats {
    // 所有数量均表示CPU提交的窗口、候选或draw，不冒充shader最终保留的草株数。
    pub target_views: u32, // 带GpuBladeView标记并进入排队系统的目标视图数量。
    pub missing_phase_views: u32, // 目标视图尚未创建AlphaMask3d阶段而跳过的数量。
    pub extracted_windows: u32, // 本帧已经从主世界提取到RenderApp的草窗口数量。
    pub input_ready_windows: u32, // 高度与植被密度纹理均已进入GPU资产表的窗口数量。
    pub height_waiting_windows: u32, // 仍等待GPU高度纹理的窗口数量。
    pub density_waiting_windows: u32, // 仍等待GPU密度纹理的窗口数量。
    pub uniform_waiting_windows: u32, // 动态uniform缓冲尚未建立时无法创建绑定组的窗口数量。
    pub prepared_bind_groups: u32, // 已成功创建窗口Compute输入、紧凑输出和状态绑定组的数量。
    pub prepared_lod_mask: u32, // 已准备Compute绑定组的LOD位集合；最低三位分别代表近、中、远。
    pub prepared_profile_lod_mask: u32, // 已准备内容与LOD组合位；位号为profile乘三加LOD。
    pub resident_reused_windows: u32, // 输入或Compute管线暂不可用时继续消费上一份有效GPU输出的窗口数量。
    pub compute_pipeline_ready_stages: u32, // 四个固定Compute阶段中已经完成异步编译并可执行的数量。
    pub compute_pipeline_waiting_stages: u32, // 四个固定Compute阶段中仍在排队或创建的数量。
    pub compute_pipeline_error_stages: u32, // 四个固定Compute阶段中编译或设备创建失败的数量，非零时草地不能生成。
    pub visible_windows: u32,               // 目标视图可见实体集中识别到的草窗口数量，按视图累计。
    pub queued_draws: u32,                  // 已加入AlphaMask3d阶段的草绘制项数量，按视图累计。
    pub queued_candidates: u32, // 已排队绘制项覆盖的候选草实例总数，不是shader最终存活株数。
    pub queued_shadow_cascades: u32, // 全部GPU叶片植物LOD0加入方向光Shadow阶段的级联绘制项数量。
    pub drawn_shadow_commands: u32, // 真正编码到方向光阴影图的八桶间接命令数量。
    pub pipeline_ready_draws: u32, // 排队时专用渲染管线已经可用的绘制项数量。
    pub pipeline_ready_lod_mask: u32, // 专用渲染管线就绪的LOD位集合，重复档位不能冒充四档完整。
    pub pipeline_ready_profile_lod_mask: u32, // 管线就绪项按内容与LOD拆分后的完整性位集合。
    pub pipeline_waiting_draws: u32, // 排队时专用渲染管线仍在异步创建的绘制项数量。
    pub pipeline_error_draws: u32, // 排队时专用渲染管线已经失败的绘制项数量。
    pub missing_mesh_instances: u32, // 可见窗口尚未取得主世界网格实例而跳过的数量。
    pub missing_gpu_meshes: u32, // 可见窗口的原型网格尚未上传GPU而跳过的数量。
    pub specialization_errors: u32, // 草地管线与当前网格布局专用化失败的数量。
    pub drawn_draws: u32,       // 真正编码`draw_indexed_indirect`命令的draw数量。
    pub drawn_candidates: u32,  // 已编码间接draw对应的候选上限，仅用于对照GPU回读的最终实例数。
    pub drawn_lod_mask: u32,    // 真正编码过间接命令的LOD位集合；阶段5要求近中远三位齐全。
    pub drawn_profile_lod_mask: u32, // 真正编码过命令的内容与LOD位集合，位号固定为profile乘四加LOD。
} // 完成对F3公开的稳定渲染快照定义。

/// RenderApp 当前帧无锁计数器。 // ExtractSchedule在下一帧开头取走并清零，主世界因此只读取完整帧。
#[derive(Resource, Default)] // 原子字段均可安全零初始化，资源只存在于渲染世界。
pub(crate) struct GpuBladeRenderFrameCounters {
    // 字段保持私有，所有统计点必须通过具名方法表达语义。
    target_views: AtomicU32,                    // 当前帧目标视图数。
    missing_phase_views: AtomicU32,             // 当前帧缺少遮罩阶段的视图数。
    extracted_windows: AtomicU32,               // 当前帧提取窗口数。
    input_ready_windows: AtomicU32,             // 当前帧高度与植被密度输入就绪窗口数。
    height_waiting_windows: AtomicU32,          // 当前帧等待高度纹理窗口数。
    density_waiting_windows: AtomicU32,         // 当前帧等待密度纹理窗口数。
    uniform_waiting_windows: AtomicU32,         // 当前帧等待uniform窗口数。
    prepared_bind_groups: AtomicU32,            // 当前帧绑定组准备数。
    prepared_lod_mask: AtomicU32,               // 当前帧绑定组覆盖的LOD位集合。
    prepared_profile_lod_mask: AtomicU32,       // 当前帧绑定组覆盖的内容与LOD组合位集合。
    resident_reused_windows: AtomicU32,         // 当前帧复用驻留GPU输出的窗口数。
    compute_pipeline_ready_stages: AtomicU32,   // 当前帧已就绪的固定Compute阶段数。
    compute_pipeline_waiting_stages: AtomicU32, // 当前帧仍异步创建的固定Compute阶段数。
    compute_pipeline_error_stages: AtomicU32,   // 当前帧编译或设备创建失败的固定Compute阶段数。
    visible_windows: AtomicU32,                 // 当前帧按视图累计的可见草窗口数。
    queued_draws: AtomicU32,                    // 当前帧AlphaMask阶段草绘制项数。
    queued_candidates: AtomicU32,               // 当前帧排队候选实例总数。
    queued_shadow_cascades: AtomicU32,          // 当前帧全部GPU叶片植物LOD0方向光级联排队项数。
    drawn_shadow_commands: AtomicU32,           // 当前帧真实编码的草地阴影间接命令数。
    pipeline_ready_draws: AtomicU32,            // 当前帧管线已就绪绘制项数。
    pipeline_ready_lod_mask: AtomicU32,         // 当前帧管线就绪项覆盖的LOD位集合。
    pipeline_ready_profile_lod_mask: AtomicU32, // 当前帧管线就绪项覆盖的内容与LOD组合位集合。
    pipeline_waiting_draws: AtomicU32,          // 当前帧管线创建中绘制项数。
    pipeline_error_draws: AtomicU32,            // 当前帧管线失败绘制项数。
    missing_mesh_instances: AtomicU32,          // 当前帧缺主网格实例跳过数。
    missing_gpu_meshes: AtomicU32,              // 当前帧缺GPU网格跳过数。
    specialization_errors: AtomicU32,           // 当前帧专用化错误数。
    drawn_draws: AtomicU32,                     // 当前帧实际编码draw数。
    drawn_candidates: AtomicU32,                // 当前帧实际编码候选实例总数。
    drawn_lod_mask: AtomicU32,                  // 当前帧实际编码命令覆盖的LOD位集合。
    drawn_profile_lod_mask: AtomicU32,          // 当前帧实际编码命令覆盖的内容与LOD组合位集合。
} // 完成渲染世界内部计数器定义。

impl GpuBladeRenderFrameCounters {
    // 为各渲染阶段提供不会引入系统可变借用冲突的轻量记录接口。
    pub(crate) fn snapshot_and_reset(&self) -> GpuBladeRenderStats {
        // 在ExtractSchedule原子取走上一完整帧并同时开始新帧。
        let take = |counter: &AtomicU32| counter.swap(0, Ordering::Relaxed); // 渲染帧边界保证只需原子性而不需跨字段强排序。
        GpuBladeRenderStats {
            // 将所有内部计数映射为主世界可读值快照。
            target_views: take(&self.target_views), // 发布目标视图数并归零。
            missing_phase_views: take(&self.missing_phase_views), // 发布缺遮罩阶段数并归零。
            extracted_windows: take(&self.extracted_windows), // 发布提取窗口数并归零。
            input_ready_windows: take(&self.input_ready_windows), // 发布高度与植被密度输入就绪数并归零。
            height_waiting_windows: take(&self.height_waiting_windows), // 发布高度等待数并归零。
            density_waiting_windows: take(&self.density_waiting_windows), // 发布密度等待数并归零。
            uniform_waiting_windows: take(&self.uniform_waiting_windows), // 发布uniform等待数并归零。
            prepared_bind_groups: take(&self.prepared_bind_groups), // 发布绑定组准备数并归零。
            prepared_lod_mask: take(&self.prepared_lod_mask),       // 发布绑定组近中远身份并归零。
            prepared_profile_lod_mask: take(&self.prepared_profile_lod_mask), // 发布绑定组内容与LOD身份并归零。
            resident_reused_windows: take(&self.resident_reused_windows), // 发布本帧安全复用旧GPU结果的窗口数并归零。
            compute_pipeline_ready_stages: take(&self.compute_pipeline_ready_stages), // 发布Compute就绪阶段数并归零。
            compute_pipeline_waiting_stages: take(&self.compute_pipeline_waiting_stages), // 发布Compute等待阶段数并归零。
            compute_pipeline_error_stages: take(&self.compute_pipeline_error_stages), // 发布Compute错误阶段数并归零。
            visible_windows: take(&self.visible_windows), // 发布可见窗口数并归零。
            queued_draws: take(&self.queued_draws),       // 发布排队draw数并归零。
            queued_candidates: take(&self.queued_candidates), // 发布排队候选数并归零。
            queued_shadow_cascades: take(&self.queued_shadow_cascades), // 发布方向光级联草阴影排队项并归零。
            drawn_shadow_commands: take(&self.drawn_shadow_commands), // 发布真实草阴影间接命令数并归零。
            pipeline_ready_draws: take(&self.pipeline_ready_draws),   // 发布管线就绪数并归零。
            pipeline_ready_lod_mask: take(&self.pipeline_ready_lod_mask), // 发布管线就绪LOD身份并归零。
            pipeline_ready_profile_lod_mask: take(&self.pipeline_ready_profile_lod_mask), // 发布管线就绪内容与LOD身份并归零。
            pipeline_waiting_draws: take(&self.pipeline_waiting_draws), // 发布管线等待数并归零。
            pipeline_error_draws: take(&self.pipeline_error_draws),     // 发布管线错误数并归零。
            missing_mesh_instances: take(&self.missing_mesh_instances), // 发布缺主网格数并归零。
            missing_gpu_meshes: take(&self.missing_gpu_meshes),         // 发布缺GPU网格数并归零。
            specialization_errors: take(&self.specialization_errors),   // 发布专用化错误数并归零。
            drawn_draws: take(&self.drawn_draws),                       // 发布实际draw数并归零。
            drawn_candidates: take(&self.drawn_candidates),             // 发布实际候选数并归零。
            drawn_lod_mask: take(&self.drawn_lod_mask), // 发布实际编码的近中远身份并归零。
            drawn_profile_lod_mask: take(&self.drawn_profile_lod_mask), // 发布实际编码的内容与LOD身份并归零。
        } // 返回上一完整渲染帧快照。
    } // 完成帧边界发布。

    pub(crate) fn record_input_window(&self, height_ready: bool, density_ready: bool) {
        // 记录单个提取窗口的高度与植被密度GPU准备状态。
        self.extracted_windows.fetch_add(1, Ordering::Relaxed); // 每个查询到的渲染副本只计一次提取。
        if height_ready && density_ready {
            // 两张必需纹理同时就绪才允许本帧绘制。
            self.input_ready_windows.fetch_add(1, Ordering::Relaxed); // 记录完整叶片输入就绪窗口。
        } // 完成完整就绪分支。
        if !height_ready {
            // 单独暴露高度上传阻塞。
            self.height_waiting_windows.fetch_add(1, Ordering::Relaxed); // 记录等待高度纹理窗口。
        } // 完成高度等待分支。
        if !density_ready {
            // 单独暴露密度上传阻塞。
            self.density_waiting_windows.fetch_add(1, Ordering::Relaxed); // 记录等待密度纹理窗口。
        } // 完成密度等待分支。
    } // 完成单窗口地表统计。

    pub(crate) fn record_uniform_waiting(&self, windows: u32) {
        // 动态uniform没有binding时整批窗口只能等待。
        self.uniform_waiting_windows
            .fetch_add(windows, Ordering::Relaxed); // 累加本帧受影响窗口数。
    } // 完成uniform等待统计。

    pub(crate) fn record_prepared_bind_group(&self, vegetation_profile: u32, lod_level: u32) {
        // 记录单个窗口绑定组成功创建。
        self.prepared_bind_groups.fetch_add(1, Ordering::Relaxed); // 每次成功插入绑定组计一。
        self.prepared_lod_mask
            .fetch_or(lod_mask_bit(lod_level), Ordering::Relaxed); // 聚合数量之外保留真实LOD身份。
        self.prepared_profile_lod_mask.fetch_or(
            profile_lod_mask_bit(vegetation_profile, lod_level),
            Ordering::Relaxed,
        ); // 普通草近级和麦田近级占不同位，不能互相冒充完整窗口。
    } // 完成绑定组统计。

    pub(crate) fn record_resident_reused_window(&self) {
        // 只有至少成功生成过一次的持久缓冲才允许进入该指标，首次加载不能伪造驻留结果。
        self.resident_reused_windows.fetch_add(1, Ordering::Relaxed); // 累加本帧因纹理或Compute等待而继续绘制旧结果的窗口。
    } // 完成阶段5驻留输出复用统计。

    pub(crate) fn record_compute_pipeline_ready(&self) {
        // Prepare在覆盖旧间接状态前为每个固定Compute阶段只记录一次当前状态。
        self.compute_pipeline_ready_stages
            .fetch_add(1, Ordering::Relaxed); // 累加已经可以绑定执行的Compute阶段。
    } // 完成Compute管线就绪统计。

    pub(crate) fn record_compute_pipeline_waiting(&self) {
        // 首次加载或着色器热重载期间异步编译属于等待而不是失败。
        self.compute_pipeline_waiting_stages
            .fetch_add(1, Ordering::Relaxed); // 累加仍在排队或创建的Compute阶段。
    } // 完成Compute管线等待统计。

    pub(crate) fn record_compute_pipeline_error(&self) {
        // 编译和设备创建错误必须进入公开快照，禁止继续表现为没有草但全部指标正常。
        self.compute_pipeline_error_stages
            .fetch_add(1, Ordering::Relaxed); // 累加本帧明确失败的Compute阶段。
    } // 完成Compute管线错误统计。

    pub(crate) fn record_target_view(&self) {
        // 记录进入草地排队的玩法视图。
        self.target_views.fetch_add(1, Ordering::Relaxed); // 每个视图循环只计一次。
    } // 完成目标视图统计。

    pub(crate) fn record_missing_phase_view(&self) {
        // 记录目标视图缺少AlphaMask遮罩阶段。
        self.missing_phase_views.fetch_add(1, Ordering::Relaxed); // 定位视图阶段尚未建立的排队阻塞。
    } // 完成缺阶段统计。

    pub(crate) fn record_visible_window(&self) {
        // 记录可见实体集中识别出的草窗口。
        self.visible_windows.fetch_add(1, Ordering::Relaxed); // 多视图时按视图累计，和排队draw语义保持一致。
    } // 完成可见窗口统计。

    pub(crate) fn record_missing_mesh_instance(&self) {
        // 记录主世界Mesh3d实例数据缺失。
        self.missing_mesh_instances.fetch_add(1, Ordering::Relaxed); // 定位提取与网格同步之间的等待。
    } // 完成缺主网格统计。

    pub(crate) fn record_missing_gpu_mesh(&self) {
        // 记录原型RenderMesh尚未上传。
        self.missing_gpu_meshes.fetch_add(1, Ordering::Relaxed); // 定位草模型GPU资产准备等待。
    } // 完成缺GPU网格统计。

    pub(crate) fn record_specialization_error(&self) {
        // 记录管线与网格布局不兼容。
        self.specialization_errors.fetch_add(1, Ordering::Relaxed); // 专用化失败不会加入AlphaMask阶段。
    } // 完成专用化错误统计。

    pub(crate) fn record_queued_draw(&self, candidates: u32) {
        // 记录一个已经加入AlphaMask阶段的草绘制项。
        self.queued_draws.fetch_add(1, Ordering::Relaxed); // 累加排队draw数量。
        self.queued_candidates
            .fetch_add(candidates, Ordering::Relaxed); // 累加该窗口候选实例数量。
    } // 完成排队统计。

    pub(crate) fn record_queued_shadow_cascade(&self) {
        self.queued_shadow_cascades.fetch_add(1, Ordering::Relaxed); // 每个植物LOD0窗口在每个玩法方向光级联中只计一次阶段项。
    } // 完成近景草阴影级联排队统计。

    pub(crate) fn record_shadow_draw(&self, indirect_commands: u32) {
        self.drawn_shadow_commands
            .fetch_add(indirect_commands, Ordering::Relaxed); // 累加真正写入方向光阴影图的八桶间接命令。
    } // 完成近景草阴影真实编码统计。

    pub(crate) fn record_pipeline_ready(&self, vegetation_profile: u32, lod_level: u32) {
        // 记录排队时管线已经可执行。
        self.pipeline_ready_draws.fetch_add(1, Ordering::Relaxed); // 累加就绪绘制项。
        self.pipeline_ready_lod_mask
            .fetch_or(lod_mask_bit(lod_level), Ordering::Relaxed); // 重复窗口不会补齐缺失的近中远位。
        self.pipeline_ready_profile_lod_mask.fetch_or(
            profile_lod_mask_bit(vegetation_profile, lod_level),
            Ordering::Relaxed,
        ); // 内容级完整性与聚合LOD掩码并行保留。
    } // 完成管线就绪统计。

    pub(crate) fn record_pipeline_waiting(&self) {
        // 记录排队时管线仍在创建。
        self.pipeline_waiting_draws.fetch_add(1, Ordering::Relaxed); // 累加等待绘制项。
    } // 完成管线等待统计。

    pub(crate) fn record_pipeline_error(&self) {
        // 记录排队时管线已经失败。
        self.pipeline_error_draws.fetch_add(1, Ordering::Relaxed); // 累加失败绘制项。
    } // 完成管线错误统计。

    pub(crate) fn record_draw(
        &self,
        candidates: u32,
        indirect_commands: u32,
        vegetation_profile: u32,
        lod_level: u32,
    ) {
        // 记录真正执行到间接实例绘制命令的窗口及其深度桶命令数。
        self.drawn_draws
            .fetch_add(indirect_commands, Ordering::Relaxed); // 累加实际编码的全部近到远draw_indexed_indirect数量。
        self.drawn_candidates
            .fetch_add(candidates, Ordering::Relaxed); // 只累加对应候选上限，最终实例数由GPU状态回读提供。
        self.drawn_lod_mask
            .fetch_or(lod_mask_bit(lod_level), Ordering::Relaxed); // 只有真正编码成功的命令才补齐对应LOD位。
        self.drawn_profile_lod_mask.fetch_or(
            profile_lod_mask_bit(vegetation_profile, lod_level),
            Ordering::Relaxed,
        ); // 只有当前内容当前LOD真实编码成功才补齐严格完整性位。
    } // 完成实际绘制统计。
} // 完成渲染帧计数器实现。

/// 把LOD编号映射为无锁帧计数器中的一位；异常编号不污染近中远完整性判断。
fn lod_mask_bit(lod_level: u32) -> u32 {
    1_u32.checked_shl(lod_level).unwrap_or(0) // 正式草窗只使用零至二，同时保护未来错误输入导致移位溢出。
}

/// 把稳定内容profile和LOD映射为严格完整性位；异常输入返回零而不污染正式草麦位。
fn profile_lod_mask_bit(vegetation_profile: u32, lod_level: u32) -> u32 {
    if vegetation_profile >= GPU_GRASS_PROFILE_CAPACITY as u32
        || lod_level >= GPU_GRASS_LOD_CAPACITY as u32
    {
        return 0; // 两位profile与四级LOD之外不参与当前固定完整性契约。
    }
    1_u32
        .checked_shl(vegetation_profile * GPU_GRASS_LOD_CAPACITY as u32 + lod_level)
        .unwrap_or(0) // 四类乘四级只使用低十六位，但仍防御未来异常移位。
}
