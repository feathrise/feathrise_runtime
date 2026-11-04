//! 独立世界 UI phase 复用 Bevy PBR 材质与 GPU batching，仅改变绘制目标。
use bevy::{
    core_pipeline::core_3d::Transparent3d,
    ecs::entity::EntityHash,
    math::FloatOrd,
    prelude::*,
    render::{
        render_phase::{
            CachedRenderPipelinePhaseItem, DrawFunctionId, PhaseItem, PhaseItemExtraIndex,
            SortedPhaseItem,
        },
        render_resource::CachedRenderPipelineId,
        sync_world::MainEntity,
        view::ExtractedView,
    },
}; // 所有管线和实例数据仍由公开 Bevy 接口管理。
use indexmap::IndexMap; // Bevy的SortedPhaseItem直接采用indexmap，没有从platform重导出。
use std::ops::Range; // 保留标准 phase 的批次范围类型。

pub(crate) struct WorldUi3d {
    // 用独立 phase 类型获得独立实例批次和间接绘制缓冲。
    pub(crate) item: Transparent3d, // 原透明项的实体、排序信息、材质 pipeline 和 mesh 状态。
    pub(crate) original_draw_function: DrawFunctionId, // 下一帧 queue 之前归还原 phase 时恢复其函数身份。
    pub(crate) original_pipeline: CachedRenderPipelineId, // 普通回退与下一帧归还必须恢复抖动管线。
} // WorldUi3d 结束。

impl PhaseItem for WorldUi3d {
    // 不改变 DrawMaterial 所需的实体与批次契约。
    fn entity(&self) -> Entity {
        self.item.entity.0
    } // 真实渲染实体。
    fn main_entity(&self) -> MainEntity {
        self.item.entity.1
    } // 真实业务实体。
    fn draw_function(&self) -> DrawFunctionId {
        self.item.draw_function
    } // 新 phase 的同一公开 DrawMaterial。
    fn batch_range(&self) -> &Range<u32> {
        &self.item.batch_range
    } // 由该 phase 自己的 batching 写入。
    fn batch_range_mut(&mut self) -> &mut Range<u32> {
        &mut self.item.batch_range
    } // 保留健康条 GPU instancing。
    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.item.extra_index.clone()
    } // CPU 或间接 GPU 绘制的原生索引类型。
    fn batch_range_and_extra_index_mut(&mut self) -> (&mut Range<u32>, &mut PhaseItemExtraIndex) {
        (&mut self.item.batch_range, &mut self.item.extra_index)
    } // 独立批次准备使用自身索引。
} // PhaseItem 实现结束。

impl SortedPhaseItem for WorldUi3d {
    // 与主透明 phase 保持相同的背向前距离规则。
    type SortKey = FloatOrd; // 浮点排序遵循 Bevy 语义。
    fn sort_key(&self) -> FloatOrd {
        FloatOrd(self.item.distance)
    } // 包含现有血条和文字 depth_bias。
    fn sort(items: &mut IndexMap<(Entity, MainEntity), Self, EntityHash>) {
        items.sort_by_key(|_, item| item.sort_key());
    } // 仅排序指定的世界 UI，不扫描水或粒子。
    fn recalculate_sort_keys(
        items: &mut IndexMap<(Entity, MainEntity), Self, EntityHash>,
        view: &ExtractedView,
    ) {
        // 使用同一玩法相机距离。
        let rangefinder = view.rangefinder3d(); // 复用 Bevy 相机变换和远近规则。
        for item in items.values_mut() {
            item.item.distance = item.item.sorting_info.sort_distance(&rangefinder);
        } // 最多处理固定预算内的 UI 项。
    } // 距离计算结束。
    fn indexed(&self) -> bool {
        self.item.indexed
    } // 保留网格索引模式。
} // SortedPhaseItem 实现结束。

impl CachedRenderPipelinePhaseItem for WorldUi3d {
    // 无需复制或修改 PBR 管线描述。
    fn cached_pipeline(&self) -> CachedRenderPipelineId {
        self.item.pipeline
    } // 使用原材质已完成的同一 HDR/单采样 pipeline。
} // 缓存管线访问结束。
