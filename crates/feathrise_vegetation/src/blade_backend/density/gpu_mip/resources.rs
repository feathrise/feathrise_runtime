//! RenderApp候选Mip管线、准备作业和异步回调共享资源。

use std::collections::VecDeque; // 回调完成顺序可能与取消后的新批次交错。
use std::sync::{
    Arc,
    Mutex,            // RenderApp和映射回调共享作业与很短结果队列。
    atomic::AtomicU8, // 单作业无锁经历READY到映射完成状态。
};

use bevy::prelude::Resource; // 管线、当前作业和共享队列都注册到渲染世界。
use bevy::render::render_resource::{
    BindGroup, BindGroupLayoutDescriptor, Buffer, CachedComputePipelineId, Texture,
}; // 每级绑定、回读缓冲、候选纹理和异步管线缓存句柄。

use crate::VegetationLayerId; // 最近作业键包含层身份，同票据的下一层仍需运行。

use super::super::gpu_compose::VegetationGpuCompetitionInput; // 同组串行层共享一次正式输入GPU缓冲。
use super::super::resources::{VegetationGpuMipJob, VegetationGpuMipResult};
use super::plan::GpuMipReadbackPlan; // RenderGraph复制和回调解码共享同一不可变规划。 // 跨世界身份与完成结果保持单一定义。

pub(super) const JOB_READY: u8 = 0; // Prepare已经完整创建资源，RenderGraph尚未录入命令。
pub(super) const JOB_RECORDING: u8 = 1; // 根图节点独占作业并正在录入Compute与复制。
pub(super) const JOB_COPY_RECORDED: u8 = 2; // 命令已提交边界，下一ExtractSchedule可发起映射。
pub(super) const JOB_MAPPING: u8 = 3; // map_async已发起，禁止再次复制或映射。
pub(super) const JOB_COMPLETE: u8 = 4; // 回调已发布成功或失败结果。

/// 正式GPU Mip固定计算管线。
#[derive(Resource)]
pub(super) struct VegetationGpuMipPipeline {
    pub(super) bind_group_layout: BindGroupLayoutDescriptor, // 两个非重叠Mip视图和一个尺寸uniform。
    pub(super) pipeline: CachedComputePipelineId, // 异步编译后的`downsample_density`入口。
}

/// 一次相邻Mip调度已经准备好的GPU对象。
pub(super) struct VegetationGpuMipDispatch {
    pub(super) bind_group: BindGroup, // 当前父级只读、相邻子级只写和专用uniform。
    pub(super) uniform: Buffer,       // 显式保留十六字节uniform直到GPU作业生命周期结束。
    pub(super) workgroups_x: u32,     // 覆盖目标宽且由WGSL裁掉尾线程。
    pub(super) workgroups_y: u32,     // 覆盖目标高且Z固定为一。
}

/// 同一批次同一竞争组可跨目标层复用的只读GPU输入缓冲。
pub(super) struct VegetationGpuCompetitionBuffers {
    pub(super) ticket: u64, // 新批次即使Arc地址复用也必须重建正式输入。
    pub(super) input: Arc<VegetationGpuCompetitionInput>, // Arc身份用于同组层快速复用。
    pub(super) layers: Buffer, // 每层优先级与权重描述。
    pub(super) tiles: Buffer, // 每个提交块的输出原点和样本起点。
    pub(super) samples: Buffer, // 每纹素逐层实际与所有权需求。
    pub(super) contexts: Buffer, // 每纹素稳定选择值与足迹影响。
}

/// 一次基础级GPU竞争已经准备好的目标层对象。
pub(super) struct VegetationGpuComposeDispatch {
    pub(super) bind_group: BindGroup, // 四个只读缓冲、Mip0写视图和当前目标uniform。
    pub(super) uniform: Buffer,       // 显式保留三十二字节uniform直到命令录入结束。
    pub(super) workgroups_x: u32,     // 一个64纹素块固定八个X工作组。
    pub(super) workgroups_y: u32,     // 一个64纹素块固定八个Y工作组。
    pub(super) workgroups_z: u32,     // 每个提交块使用一个Z切片。
}

/// 一个候选层在渲染世界中的完整不可变作业。
pub(super) struct VegetationGpuMipPreparedJob {
    pub(super) identity: VegetationGpuMipJob, // 结果原样携带票据、版本和层。
    pub(super) texture: Texture,              // Compute写入和随后全Mip复制使用同一GpuImage纹理。
    pub(super) previous_texture: Option<Texture>, // 局部提交先复制上一有效候选基础级。
    pub(super) compose: Option<VegetationGpuComposeDispatch>, // 正式作业先在GPU解析基础级；旧Mip烟雾为None。
    pub(super) readback: Buffer,                              // 动态大小COPY_DST与MAP_READ缓冲。
    pub(super) readback_plan: GpuMipReadbackPlan,             // 复制区域与回调去填充完全同源。
    pub(super) dispatches: Vec<VegetationGpuMipDispatch>,     // 1×1候选合法地为空。
    pub(super) state: AtomicU8, // READY→RECORDING→COPY_RECORDED→MAPPING→COMPLETE。
}

/// RenderApp当前候选作业与已经处理过的请求键。
#[derive(Default, Resource)]
pub(super) struct VegetationGpuMipRenderState {
    pub(super) current: Option<Arc<VegetationGpuMipPreparedJob>>, // 回调可独立延长作业生命周期。
    pub(super) last_key: Option<(u64, VegetationLayerId)>, // 同票据的不同层分别且只执行一次。
    pub(super) competition: Option<VegetationGpuCompetitionBuffers>, // 同组串行层复用大存储缓冲并在请求结束后释放。
}

/// 映射回调和ExtractSchedule共享的短完成队列。
#[derive(Clone, Default, Resource)]
pub(super) struct VegetationGpuMipShared {
    completed: Arc<Mutex<VecDeque<VegetationGpuMipResult>>>, // 锁不包围任何GPU等待或大字节比较。
}

impl VegetationGpuMipShared {
    /// 从映射回调追加一份完整结果。
    pub(super) fn publish(&self, result: VegetationGpuMipResult) {
        self.completed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(result); // 临界区只移动一个Arc字节载荷或短错误字符串。
    }

    /// 在ExtractSchedule一次取走当前全部已完成结果。
    pub(super) fn drain(&self) -> VecDeque<VegetationGpuMipResult> {
        let mut completed = self
            .completed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()); // 过去回调panic也恢复通道。
        std::mem::take(&mut *completed) // 交换空队列后立即释放锁，不在主世界写入期间持有。
    }
}
