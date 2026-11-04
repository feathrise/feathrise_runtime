//! 显式近场落点的不可变均匀网格索引与配置代次失效管理。

use std::collections::BTreeMap; // 有序层表和单元表让重建及测试结果不依赖哈希随机种子。

use bevy::prelude::{Vec2, Vec3Swizzles}; // 近场粗筛只使用世界X/Z二维位置。

use crate::{
    // 索引只读取内容无关的公共层配置与显式落点。
    ExplicitVegetationPlacement, // 单元桶保存该稳定落点在原清单中的下标。
    VegetationCandidateSource,   // 只有ExplicitPlacements需要额外空间索引。
    VegetationLayerConfig,       // 每层退出半径决定均匀网格单元尺寸。
    VegetationLayerId,           // 稳定层身份索引对应层的只读空间结构。
    VegetationRealization,       // 只有HybridNearField层参与近场候选查询。
}; // 完成空间索引依赖导入。

type SpatialCell = (i32, i32); // 二元组按X/Z稳定排序且无需依赖IVec2的有序实现。

/// 与一代层配置严格对应的全部显式近场空间索引。
#[derive(Clone, Debug, Default)] // 运行资源可克隆并从空配置安全启动。
pub(super) struct ExplicitPlacementSpatialIndexes {
    // 索引集合不包含活动布局或观察者等逐帧状态。
    configuration_generation: u64, // 只有set_configuration推进该身份。
    layers: BTreeMap<VegetationLayerId, ExplicitPlacementSpatialIndex>, // 每个显式近场层独立使用自己的退出半径。
    #[cfg(test)] // 重建计数只用于证明源字段更新不会重复构造空间桶。
    rebuild_count: u64, // 单测可区分“代次未变”与“错误地按同代重建”。
} // 完成配置级空间索引集合定义。

impl ExplicitPlacementSpatialIndexes {
    // 索引生命周期集中处理，系统无需分别判断层增删。
    /// 配置代次变化时一次重建全部显式近场层索引。
    pub(super) fn rebuild_if_needed(
        // 返回值让回归测试确认密度源更新不会触发重建。
        &mut self,                     // 原地替换索引集合，避免活动查询观察到半套新配置。
        configuration_generation: u64, // 当前VegetationLayoutState层配置身份。
        layers: &[VegetationLayerConfig], // 同一代已经通过公共配置校验的完整层表。
    ) -> bool {
        // true只表示本次确实采用了一代新空间拓扑。
        if self.configuration_generation == configuration_generation {
            // 密度源变化不改变显式落点拓扑。
            return false; // 保留现有桶和分配，避免无意义全量重建。
        } // 完成空间拓扑稳定快速路径。
        let mut rebuilt = BTreeMap::new(); // 新代索引在局部完整构造后一次采用。
        for layer in layers {
            // 每层配置彼此独立且层ID已由验证保证唯一。
            let VegetationRealization::HybridNearField { exit_radius, .. } = layer.realization
            else {
                // 非近场实现不会产生代理查询。
                continue; // GPUOnly和Entities层不占用近场索引内存。
            }; // 完成近场实现筛选。
            let VegetationCandidateSource::ExplicitPlacements(placements) = &layer.source else {
                // 程序网格已有整数世界格索引。
                continue; // DenseGrid和JitteredGrid继续按观察者附近格子生成。
            }; // 完成显式候选源筛选。
            rebuilt.insert(
                // 同一层只写入一份完整不可变索引。
                layer.id, // 稳定层ID用于查询时与层配置对应。
                ExplicitPlacementSpatialIndex::new(placements, exit_radius), // 退出半径保证粗筛覆盖滞回外圈。
            ); // 完成当前显式近场层索引登记。
        } // 完成全部层空间结构构建。
        self.layers = rebuilt; // 局部完整结果一次替换，查询不会看到部分新代。
        self.configuration_generation = configuration_generation; // 最后提交代次，标记桶与当前层配置一致。
        #[cfg(test)] // 正式构建不携带测试计数状态。
        {
            // 测试构建只在真实重建分支推进计数。
            self.rebuild_count = self.rebuild_count.wrapping_add(1); // 允许回归精确观察重建次数。
        } // 完成测试重建计数更新。
        true // 调用方可验证这次确实发生了重建。
    } // 完成配置级索引重建。

    /// 返回一层已经与当前配置对齐的显式空间索引。
    pub(super) fn get(
        // 缺失明确表示该层不是显式近场源。
        &self,                    // 查询期间全部worker只共享不可变引用。
        layer: VegetationLayerId, // 当前待处理层稳定身份。
    ) -> Option<&ExplicitPlacementSpatialIndex> {
        // 返回值生命周期受运行资源约束。
        self.layers.get(&layer) // BTreeMap读取可安全跨ComputeTaskPool线程共享。
    } // 完成单层空间索引查询。

    #[cfg(test)] // 代次只作为内部回归证据，不扩展公共API。
    pub(super) fn configuration_generation(&self) -> u64 {
        // 测试确认源字段更新不会重建索引。
        self.configuration_generation // 返回当前已采用索引对应的层配置代次。
    } // 完成测试代次查询。

    #[cfg(test)] // 重建次数只作为内部性能回归证据。
    pub(super) fn rebuild_count(&self) -> u64 {
        // 测试直接核对是否发生额外全量构造。
        self.rebuild_count // 返回自空运行资源以来的真实索引采用次数。
    } // 完成测试重建次数查询。
} // 完成配置级索引集合实现。

/// 单个显式近场层的均匀网格；桶值只保存原清单下标。
#[derive(Clone, Debug)] // 不可变桶允许运行资源克隆并供多个worker只读共享。
pub(super) struct ExplicitPlacementSpatialIndex {
    // 单元尺寸固定后查询只访问观察者外圈覆盖的少量桶。
    cell_size: f32, // 使用该层exit_radius时通常最多访问每个观察者周围九个单元。
    cells: BTreeMap<SpatialCell, Vec<usize>>, // 每个落点恰好属于一个floor世界单元。
} // 完成单层均匀网格定义。

impl ExplicitPlacementSpatialIndex {
    // 构建、估算和查询共同复用相同单元坐标口径。
    fn new(
        // 构造只在受检配置代次变化时执行，不进入玩家逐帧移动路径。
        placements: &[ExplicitVegetationPlacement], // 原清单顺序决定稳定候选下标。
        cell_size: f32,                             // 公共验证已保证exit_radius有限且严格大于零。
    ) -> Self {
        // 返回完整不可变单层空间索引。
        debug_assert!(cell_size.is_finite() && cell_size > 0.0); // 独立函数仍记录其受检配置前置条件。
        let mut cells = BTreeMap::<SpatialCell, Vec<usize>>::new(); // 桶按世界单元稳定排序。
        for (placement_index, placement) in placements.iter().enumerate() {
            // 每个显式落点只登记一次原清单下标。
            let world_xz = placement.transform.translation.xz(); // 高度不影响平面近场半径。
            let cell = spatial_cell(world_xz, cell_size); // floor映射保证负坐标边界连续。
            cells.entry(cell).or_default().push(placement_index); // 桶内保留原清单升序，避免复制Transform。
        } // 完成全部显式落点分桶。
        Self { cell_size, cells } // 一次返回完整索引供配置级集合采用。
    } // 完成单层索引构造。

    /// 收集任一观察者退出半径矩形覆盖的候选下标并稳定去重。
    pub(super) fn query_indices(
        // 粗筛只保证不漏点，最终仍执行原圆形距离精筛。
        &self,              // 多个layer worker只读共享已完成索引。
        observers: &[Vec2], // 所有观察者共同决定候选桶并集。
        radius: f32,        // 使用层exit_radius覆盖新进入和已活动两种滞回半径。
    ) -> Vec<usize> {
        // 下标随后通过deterministic_candidate恢复原候选语义。
        let mut indices = Vec::new(); // 多观察者重叠桶可能暂时产生重复下标。
        self.visit_observer_cells(observers, radius, |bucket| {
            // 只遍历空间窗口覆盖的现存桶。
            indices.extend_from_slice(bucket); // 复制usize而不复制完整Transform或候选值。
        }); // 完成全部观察者桶收集。
        indices.sort_unstable(); // 原清单下标排序让观察者顺序不影响精筛顺序。
        indices.dedup(); // 重叠观察者和重叠单元只精筛一次同一显式落点。
        indices // 返回附近候选的稳定升序原清单下标。
    } // 完成显式候选空间查询。

    /// 估算本层粗筛产生的候选检查上界，用于避免轻任务支付并行调度成本。
    pub(super) fn estimated_candidate_checks(
        // 估算允许重复计入重叠观察者，形成保守上界。
        &self,              // 只读取各查询桶长度，不生成候选或修改缓存。
        observers: &[Vec2], // 当前稳定排序的全部观察者位置。
        radius: f32,        // 与真正查询相同的退出半径。
    ) -> usize {
        // 饱和计数防止异常大输入发生usize回绕。
        let mut checks = 0usize; // 空观察者或空索引自然得到零工作量。
        self.visit_observer_cells(observers, radius, |bucket| {
            // 重用真实单元覆盖算法避免估算漏算。
            checks = checks.saturating_add(bucket.len()); // 重叠桶重复计数只会更保守地触发并行。
        }); // 完成候选检查数估算。
        checks // 返回无需全量扫描即可取得的近场工作上界。
    } // 完成单层工作量估算。

    fn visit_observer_cells(
        // 查询和估算共享同一空间窗口遍历，防止边界口径漂移。
        &self,                           // 只读均匀网格桶。
        observers: &[Vec2],              // 任一观察者覆盖即可保留候选。
        radius: f32,                     // 合法配置下为有限正退出半径。
        mut visit: impl FnMut(&[usize]), // 调用者选择收集下标或仅累计长度。
    ) {
        // 无返回值，所有结果通过局部闭包处理。
        for observer in observers {
            // 多观察者分别覆盖自己的世界窗口。
            let (min_cell, max_cell) = spatial_cell_bounds(*observer, radius, self.cell_size); // 直接由observer±radius求闭区间，完整覆盖单元边界。
            for cell_z in min_cell.1..=max_cell.1 {
                // Z轴按稳定升序遍历闭区间。
                for cell_x in min_cell.0..=max_cell.0 {
                    // X轴按稳定升序遍历闭区间。
                    if let Some(bucket) = self.cells.get(&(cell_x, cell_z)) {
                        // 空单元无需建立占位Vec。
                        visit(bucket); // 只把现存桶交给调用者处理。
                    } // 完成当前单元可选桶访问。
                } // 完成当前Z行全部X单元。
            } // 完成当前观察者全部Z行。
        } // 完成全部观察者空间窗口。
    } // 完成共享单元遍历。
} // 完成单层均匀网格实现。

fn spatial_cell(world_xz: Vec2, cell_size: f32) -> SpatialCell {
    // 统一世界点到整数单元映射。
    let cell = (world_xz / cell_size).floor().as_ivec2(); // floor避免负坐标向零截断造成边界漏点。
    (cell.x, cell.y) // Vec2的Y轴在植被平面语义中对应世界Z。
} // 完成世界点单元映射。

fn spatial_cell_bounds(
    // 查询矩形直接映射为整数闭区间，边界候选不会被遗漏。
    observer: Vec2, // 当前观察者世界X/Z位置。
    radius: f32,    // 当前层最大近场退出半径。
    cell_size: f32, // 索引构建时锁定的同层单元边长。
) -> (SpatialCell, SpatialCell) {
    // 返回最小与最大闭区间单元坐标。
    let radius_vector = Vec2::splat(radius); // 两轴使用相同世界半径形成粗筛正方形。
    let min_cell = spatial_cell(observer - radius_vector, cell_size); // 包含左下边界所在单元。
    let max_cell = spatial_cell(observer + radius_vector, cell_size); // 包含右上边界所在单元。
    (min_cell, max_cell) // 精确圆形距离会随后剔除正方形四角误入候选。
} // 完成查询窗口单元范围计算。
