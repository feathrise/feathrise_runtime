//! 可见叶片窗口采用活动解析密度的只读状态与同步系统。
//! 【功能设计】状态只观察真正写入窗口的版本身份；候选验证中或失败时始终报告保留原直接密度/上一有效密度。

use std::collections::BTreeSet; // 同层多个LOD窗口只计一次绑定层身份。

use bevy::prelude::{Query, Res, ResMut, Resource}; // PostUpdate只读窗口版本并发布一份轻量主世界快照。

use crate::{ActiveVegetationLayout, VegetationLayerId}; // 当前活动版本是采用统计的唯一比较基准。

use super::async_build::VegetationBladeDensityCpuBuild; // CPU候选在途也属于保持托底的验证阶段。
use super::components::{VegetationBladeLayer, VegetationBladeLayoutRevision}; // 公开绑定和私有已采用身份共同证明窗口状态。
use super::resources::{VegetationBladeDensityCache, VegetationGpuMipCoordinator}; // 候选在途与拒绝记录区分等待、校验和失败托底。

/// 可见叶片窗口采用活动解析密度的阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VegetationBladeAdoptionPhase {
    #[default]
    Unbound, // 当前世界没有显式绑定通用植被层的可见叶片窗口。
    FallbackRetained,        // 窗口仍保留进入时直接密度资产或上一已验证版本。
    Validating,              // 当前活动版本正在执行跨层GPU逐字节候选门，窗口尚未切换。
    CurrentLayoutAdopted,    // 全部可见绑定窗口已经采用当前活动版本与正确层身份。
    RejectedKeepingFallback, // 当前候选已拒绝，全部未采用窗口继续使用安全托底密度。
}

/// 可见叶片窗口对当前活动布局的只读采用快照。
///
/// 【功能设计】该资源不驱动迁移，也不等同于项目侧最终画面验收状态；它只回答当前帧有多少
/// 显式绑定窗口真正持有当前`revision + layer`身份，供F3、严格烟雾和地图适配器观察。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Resource)]
pub struct VegetationBladeAdoptionStatus {
    phase: VegetationBladeAdoptionPhase, // 当前可见窗口采用阶段。
    active_revision: u64,                // 统计时观察到的活动布局版本。
    bound_layer_count: usize,            // 去重后的可见绑定层数，不包含只校验层。
    bound_window_count: usize,           // 显式绑定层组件的可见叶片窗口总数。
    adopted_window_count: usize,         // 已采用当前版本且层身份匹配的窗口数。
}

impl VegetationBladeAdoptionStatus {
    /// 返回当前可见采用阶段。
    pub const fn phase(&self) -> VegetationBladeAdoptionPhase {
        self.phase // 消费者不能直接改写后端证明状态。
    }

    /// 返回本快照比较的活动布局版本。
    pub const fn active_revision(&self) -> u64 {
        self.active_revision // 零表示核心尚未发布任何有效布局。
    }

    /// 返回去重后的可见绑定层数。
    pub const fn bound_layer_count(&self) -> usize {
        self.bound_layer_count // 同层四个LOD窗口只计一层。
    }

    /// 返回显式绑定通用植被层的可见窗口总数。
    pub const fn bound_window_count(&self) -> usize {
        self.bound_window_count // 不包含`VegetationBladeValidationLayers`中的后台层。
    }

    /// 返回已经采用当前活动版本与正确层身份的窗口数。
    pub const fn adopted_window_count(&self) -> usize {
        self.adopted_window_count // 旧版本缓存不能冒充当前采用。
    }

    /// 当前全部可见绑定窗口是否已经采用活动解析布局。
    pub const fn is_current_layout_fully_adopted(&self) -> bool {
        matches!(
            self.phase,
            VegetationBladeAdoptionPhase::CurrentLayoutAdopted
        ) // 空窗口不会被误报为全部采用。
    }
}

/// 在Update延迟命令应用后统计真正写入窗口的采用身份。
pub(super) fn sync_blade_adoption_status(
    active: Res<ActiveVegetationLayout>, // 只接受当前原子发布的活动版本。
    cache: Res<VegetationBladeDensityCache>, // 当前版本拒绝记录用于解释为何继续托底。
    coordinator: Res<VegetationGpuMipCoordinator>, // 当前批次存在时报告逐字节校验中。
    cpu_build: Res<VegetationBladeDensityCpuBuild>, // 大地图候选登记Assets前也报告验证中。
    windows: Query<(
        &VegetationBladeLayer,                  // 读取窗口要求的正式层身份。
        Option<&VegetationBladeLayoutRevision>, // 读取后端真正写入的版本与层身份。
    )>, // 不读取纹理像素或Patch大对象。
    mut status: ResMut<VegetationBladeAdoptionStatus>, // 只在快照变化时触发资源变更检测。
) {
    let mut bound_layers = BTreeSet::<VegetationLayerId>::new(); // 稳定去重同层全部LOD窗口。
    let mut bound_window_count = 0_usize; // 本帧显式绑定窗口计数。
    let mut adopted_window_count = 0_usize; // 本帧当前版本采用计数。
    for (binding, applied) in &windows {
        bound_window_count = bound_window_count.saturating_add(1); // 极端世界规模下仍不允许调试计数溢出。
        bound_layers.insert(binding.0); // 公开层绑定决定候选和状态归属。
        if applied.is_some_and(|applied| {
            applied.revision == active.revision && applied.layer == binding.0 // 版本和层必须同时匹配。
        }) {
            adopted_window_count = adopted_window_count.saturating_add(1); // 只有真正写入当前身份的窗口计为采用。
        }
    }
    let rejected = bound_layers
        .iter()
        .any(|layer| cache.rejected.contains(&(active.revision, *layer))); // 任一可见层失败都会保留旧密度。
    let gpu_validating = coordinator.pending.as_ref().is_some_and(|batch| {
        batch.revision == active.revision
            && bound_layers
                .iter()
                .all(|layer| batch.layers.contains_key(layer)) // 当前批次必须覆盖全部可见层才解释为本组校验。
    });
    let validating = gpu_validating
        || cpu_build.covers_visible_layers(active.revision, bound_layers.iter().copied()); // CPU与GPU两阶段都继续显示安全托底且不冒充已采用。
    let next = VegetationBladeAdoptionStatus {
        phase: classify_adoption_phase(
            active.revision,
            bound_window_count,
            adopted_window_count,
            validating,
            rejected,
        ), // 固定优先级避免拒绝与旧在途状态产生歧义。
        active_revision: active.revision, // F3可把采用计数与具体布局代次一起记录。
        bound_layer_count: bound_layers.len(), // 验证层集合不计入可见采用分母。
        bound_window_count,               // 保留真实窗口分母。
        adopted_window_count,             // 保留当前版本真实窗口分子。
    };
    if *status != next {
        *status = next; // 稳定帧不制造资源变更噪声。
    }
}

/// 按固定优先级把可见计数和候选门状态归并为公开阶段。
fn classify_adoption_phase(
    active_revision: u64,        // 零版本不能宣称采用成功。
    bound_window_count: usize,   // 零窗口优先归为未绑定。
    adopted_window_count: usize, // 只有全分母匹配才是当前布局采用。
    validating: bool,            // 当前版本完整可见层批次是否在途。
    rejected: bool,              // 当前版本任一可见层是否已拒绝。
) -> VegetationBladeAdoptionPhase {
    if bound_window_count == 0 {
        return VegetationBladeAdoptionPhase::Unbound; // 后台只校验层不冒充可见迁移。
    }
    if active_revision != 0 && adopted_window_count == bound_window_count {
        return VegetationBladeAdoptionPhase::CurrentLayoutAdopted; // 全窗真实身份优先于残留诊断记录。
    }
    if rejected {
        return VegetationBladeAdoptionPhase::RejectedKeepingFallback; // 明确解释本版本为何不再继续候选。
    }
    if validating {
        return VegetationBladeAdoptionPhase::Validating; // 候选仍不可见，窗口保持安全托底。
    }
    VegetationBladeAdoptionPhase::FallbackRetained // 布局未就绪、候选未建立或晚创建窗都先保留旧密度。
}

#[cfg(test)]
mod tests; // 阶段分类和真实窗口计数回归独立维护。
