//! 叶片密度候选批次、GPU作业和版本化回读的跨系统状态。

use std::collections::{BTreeMap, BTreeSet, VecDeque}; // 稳定层顺序保证GPU串行作业和最终原子发布可重复。
use std::sync::Arc; // CPU参考Mip和回读字节在状态转换时只克隆引用。

use bevy::prelude::{Handle, Image, Resource, Vec2}; // 缓存图片句柄、世界映射和主世界资源。
use bevy::render::extract_resource::ExtractResource; // 每帧只提取当前一个轻量GPU候选作业。

use crate::VegetationLayerId; // 所有候选、结果和拒绝记录都以公共层ID关联。

use super::atlas::ResolvedDensityAtlas; // 活动缓存保留紧凑CPU完整Mip供下一版本局部重建。
use super::gpu_compose::VegetationGpuCompetitionInput; // 正式候选携带尚未解析的共享组输入而不是CPU基础级答案。

/// 仅请求GPU逐字节校验、但不绑定任何可见叶片窗口的层集合。
///
/// 地图迁移可以先提交正式源与竞争作业，完整回读通过后只进入内部缓存；
/// 未挂[`super::VegetationBladeLayer`]的旧窗口继续使用已经验收的直接密度资产。
#[derive(Clone, Debug, Default, Resource)]
pub struct VegetationBladeValidationLayers {
    layers: BTreeSet<VegetationLayerId>, // 稳定集合与可见绑定层合并后共同参与跨层原子候选门。
}

impl VegetationBladeValidationLayers {
    /// 原子替换当前只校验层集合。
    pub fn replace(
        &mut self,
        layers: impl IntoIterator<Item = VegetationLayerId>, // 消费地图可传数组、向量或稳定集合。
    ) {
        self.layers = layers.into_iter().collect(); // BTreeSet同时去重并固定GPU串行顺序。
    }

    /// 清除地图会话留下的只校验层。
    pub fn clear(&mut self) {
        self.layers.clear(); // 已通过内部缓存可继续存在，但不再阻止其他地图绑定集合。
    }

    /// 返回当前稳定只校验层迭代器。
    pub(super) fn iter(&self) -> impl Iterator<Item = VegetationLayerId> + '_ {
        self.layers.iter().copied() // 主状态机只读取副本并与实体绑定层合并。
    }
}

/// 一层共享的已上传图片句柄与世界映射。
#[derive(Clone)]
pub(super) struct CachedBladeDensity {
    pub(super) revision: u64,                    // 图片像素对应的活动布局版本。
    pub(super) image: Handle<Image>,             // 同层全部LOD窗口共享一份GPU上传。
    pub(super) atlas: Arc<ResolvedDensityAtlas>, // 保留紧凑R8完整Mip供下一版本只重建脏区。
    pub(super) world_min: Vec2,                  // `GpuBladePatch`密度世界映射下界。
    pub(super) world_max: Vec2,                  // `GpuBladePatch`密度世界映射上界。
}

/// 跨布局版本复用当前有效图片，并记录已拒绝的候选版本。
#[derive(Clone, Default, Resource)]
pub(super) struct VegetationBladeDensityCache {
    pub(super) layers: BTreeMap<VegetationLayerId, CachedBladeDensity>, // 每个无限层最多一张当前有效共享图。
    pub(super) rejected: BTreeSet<(u64, VegetationLayerId)>, // 同一失败版本与层只发布一次拒绝消息。
}

/// 主世界提交给RenderApp的一次完整Mip候选作业。
#[derive(Clone, Debug)]
pub(super) struct VegetationGpuMipJob {
    pub(super) ticket: u64,                // 单调票据拒绝取消批次留下的迟到回调。
    pub(super) revision: u64,              // 结果必须仍对应当前活动布局版本。
    pub(super) layer: VegetationLayerId,   // 一个作业只生成一层完整Mip以限制在途显存。
    pub(super) image: Handle<Image>,       // 候选图片只上传合法基础级，其余级由GPU覆盖。
    pub(super) width: u32,                 // 基础级宽度用于验证RenderAsset与回读计划。
    pub(super) height: u32,                // 基础级高度用于构造相邻Mip链。
    pub(super) mip_level_count: u32,       // 候选必须从基础级完整收敛到1×1。
    pub(super) base: VegetationGpuBaseJob, // CPU基础级冒烟或正式GPU竞争二选一。
}

/// 候选基础级在RenderGraph中的来源。
#[derive(Clone, Debug)]
pub(super) enum VegetationGpuBaseJob {
    #[allow(dead_code)] // P5.2b2a隔离烟雾仍需覆盖“上传基础级后只生成Mip”的旧闭环。
    Uploaded, // P5.2b2a兼容烟雾：CPU只上传基础级，GPU仍负责全部子Mip。
    Competition {
        input: Arc<VegetationGpuCompetitionInput>, // 同组多个目标层共享未决正式输入。
        target_layer_index: u32,                   // WGSL按稳定组索引只写当前串行候选层。
        previous_image: Option<Handle<Image>>, // 局部提交先从上一有效GPU图复制基础级；整图加载为None。
    },
}

/// 当前最多一个待提取GPU作业。
#[derive(Clone, Debug, Default, ExtractResource, Resource)]
pub(super) struct VegetationGpuMipRequest {
    pub(super) job: Option<VegetationGpuMipJob>, // `None`表示稳定版本或正在等待主世界决定下一层。
}

/// RenderApp回调发布给主世界的版本化结果载荷。
#[derive(Clone, Debug)]
pub(super) enum VegetationGpuMipOutcome {
    Complete(Arc<[u8]>), // 成功载荷是去除行填充后的全部RGBA8 Mip紧凑字节。
    Failed(String),      // 映射、解码或渲染资源验证失败都保留可读原因。
}

/// 一次异步回读的完整身份与结果。
#[derive(Clone, Debug)]
pub(super) struct VegetationGpuMipResult {
    pub(super) ticket: u64,                      // 主世界只接受当前批次的票据。
    pub(super) revision: u64,                    // 活动布局换版后旧回调不得倒退缓存。
    pub(super) layer: VegetationLayerId,         // 当前串行层必须与结果层完全一致。
    pub(super) outcome: VegetationGpuMipOutcome, // 成功字节或明确失败二选一。
}

/// RenderApp在ExtractSchedule向主世界追加结果的短队列。
#[derive(Default, Resource)]
pub(super) struct VegetationGpuMipInbox {
    pub(super) results: VecDeque<VegetationGpuMipResult>, // 通常只有一个结果，队列仍能容纳取消后的并发迟到回调。
}

/// 一个候选层在原子批次中的主世界阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PendingBladeDensityState {
    Waiting,   // CPU参考和候选图片已经建立，尚未提取到RenderApp。
    Requested, // 当前作业已经发布，等待非阻塞GPU回读。
    Validated, // 全部Mip逐RGBA字节与CPU参考完全一致。
}

/// 一层尚未公开的候选图片与CPU字节权威。
pub(super) struct PendingBladeDensity {
    pub(super) cached: CachedBladeDensity, // 只有整个批次成功后才移入活动缓存。
    pub(super) expected: Arc<[u8]>,        // P5.2a完整CPU Mip是无容差逐字节权威。
    pub(super) base: VegetationGpuBaseJob, // 正式运行只携带未决竞争输入和可选上一GPU图。
    pub(super) state: PendingBladeDensityState, // 串行调度和原子提交的唯一阶段。
}

/// 同一活动版本全部绑定层的原子候选批次。
pub(super) struct PendingBladeDensityBatch {
    pub(super) ticket: u64,                   // 批次内所有串行层共享一个票据。
    pub(super) revision: u64,                 // 只有该版本仍活动时才允许最终发布。
    pub(super) configuration_generation: u64, // 配置热更后旧代GPU结果不得继续采用。
    pub(super) layers: BTreeMap<VegetationLayerId, PendingBladeDensity>, // 稳定ID顺序决定下一GPU作业。
    pub(super) started_update: u64, // 有界等待避免设备丢失后永久悬挂。
    pub(super) failure: Option<String>, // 任一层失败都会取消整批并保留旧缓存。
}

impl PendingBladeDensityBatch {
    /// 返回下一层尚未提交的候选。
    pub(super) fn next_waiting_layer(&self) -> Option<VegetationLayerId> {
        self.layers.iter().find_map(|(layer, pending)| {
            (pending.state == PendingBladeDensityState::Waiting).then_some(*layer) // BTreeMap保证跨运行稳定串行顺序。
        })
    }

    /// 当前批次是否已经逐层全部通过。
    pub(super) fn is_complete(&self) -> bool {
        self.layers.values().all(|pending| {
            pending.state == PendingBladeDensityState::Validated // 禁止部分成功候选提前切换任何窗口。
        })
    }

    /// 返回批次稳定层身份，用于检测绑定集合变化。
    pub(super) fn layer_ids(&self) -> Vec<VegetationLayerId> {
        self.layers.keys().copied().collect() // 层数很小，换版时分配一次便于明确比较。
    }
}

/// 主世界候选批次协调器。
#[derive(Default, Resource)]
pub(super) struct VegetationGpuMipCoordinator {
    pub(super) next_ticket: u64,  // 每次新批次递增，零票据保留为无作业。
    pub(super) update_index: u64, // 只用于有界等待，不参与布局内容版本。
    pub(super) pending: Option<PendingBladeDensityBatch>, // 任一时刻最多保留一批候选纹理。
}

/// 当前应用是否拥有可执行候选Mip的RenderApp。
#[derive(Clone, Copy, Debug, Default, Resource)]
pub(super) struct VegetationGpuMipRuntimeAvailable(pub(super) bool); // 无RenderApp逻辑测试继续走完整CPU图片回退。
