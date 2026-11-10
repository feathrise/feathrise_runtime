//! 活动布局、暂存版本和归一化源字段。

use std::cmp::Ordering; // IVec2本身没有全序，脏块包装按Y/X实现确定性顺序。
use std::collections::{BTreeMap, BTreeSet}; // 稀疏块和层值按稳定ID排序便于逐字节回归。
use std::sync::Arc; // 源像素和未变布局块以强引用共享，后台批次不在帧关键路径复制上百MiB数据。

use bevy::prelude::{IVec2, Rect, Resource, Vec2}; // 布局在世界X/Z平面按64纹素块组织。

use crate::{
    VegetationClaimSampling, VegetationCompetitionGroup, VegetationConfigError,
    VegetationLayerConfig, VegetationLayerId,
}; // 状态保存待解析的公共配置并返回明确错误。

pub(crate) const VEGETATION_TILE_SIDE: usize = 64; // 固定64×64纹素脏块是公共性能契约。
pub(crate) const VEGETATION_TILE_TEXEL_COUNT: usize = VEGETATION_TILE_SIDE * VEGETATION_TILE_SIDE; // 每层每块固定4096个归一化值。
const CLAIM_OVERLAP_EPSILON_METERS: f64 = 0.0001; // 与离线草图生成器共享米制容差，避免边界浮点误差漏掉相交采样单元。

/// 一个稀疏脏块坐标。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VegetationDirtyTile(pub IVec2); // 允许负坐标覆盖世界原点两侧。

impl PartialOrd for VegetationDirtyTile {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other)) // 整数块坐标具有稳定全序。
    }
}

impl Ord for VegetationDirtyTile {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0.y, self.0.x).cmp(&(other.0.y, other.0.x)) // 行优先处理脏块并支持负坐标。
    }
}

/// 单层在一个脏块上的解析结果。
#[derive(Clone, Debug, PartialEq)]
pub struct VegetationResolvedLayer {
    pub layer: VegetationLayerId, // 无限层ID与值数组一一关联。
    pub density: Vec<f32>,        // 行优先64×64归一化分配场。
}

/// 一个已完成竞争与跨组足迹排斥的布局块。
#[derive(Clone, Debug, PartialEq)]
pub struct VegetationResolvedTile {
    pub coordinate: VegetationDirtyTile, // 后端据此映射固定世界纹素。
    pub world_bounds: Rect,              // 块覆盖的世界X/Z闭开范围。
    pub layers: Vec<VegetationResolvedLayer>, // 层ID稳定升序，便于GPU批量上传。
}

/// 全部完成后一次切换的当前有效稀疏布局。
#[derive(Clone, Debug, Resource, PartialEq)]
pub struct ActiveVegetationLayout {
    pub revision: u64,   // 只有整批脏块全部成功后递增。
    pub texel_size: f32, // 由最高精度输入字段确定的世界米制布局分辨率。
    pub tiles: BTreeMap<VegetationDirtyTile, Arc<VegetationResolvedTile>>, // 未变块在异步staging中只克隆Arc，不再深拷贝六层完整密度。
}

impl Default for ActiveVegetationLayout {
    fn default() -> Self {
        Self {
            revision: 0,            // 尚无成功解析布局。
            texel_size: 1.0,        // 空状态使用安全正值，首次配置后由源字段收紧。
            tiles: BTreeMap::new(), // 没有消费者注册的内容块。
        }
    }
}

impl ActiveVegetationLayout {
    /// 采样当前有效布局中的一层；未驻留块或缺失层返回零。
    pub fn sample(&self, layer: VegetationLayerId, world_xz: Vec2) -> f32 {
        let tile_world_size = VEGETATION_TILE_SIDE as f32 * self.texel_size; // 与合成器使用同一固定块尺度。
        if !tile_world_size.is_finite() || tile_world_size <= 0.0 {
            return 0.0; // 损坏快照不得让后端读取越界数组。
        }
        let tile_coordinate = (world_xz / tile_world_size).floor().as_ivec2(); // floor保持负坐标块连续。
        let Some(tile) = self.tiles.get(&VegetationDirtyTile(tile_coordinate)) else {
            return 0.0; // 稀疏未驻留块没有已解析候选。
        };
        let local = (world_xz - tile.world_bounds.min) / self.texel_size; // 映射到块内纹素坐标。
        let x = local.x.floor() as i32;
        let y = local.y.floor() as i32;
        if x < 0 || y < 0 || x >= VEGETATION_TILE_SIDE as i32 || y >= VEGETATION_TILE_SIDE as i32 {
            return 0.0; // 边界浮点误差不允许越界读取相邻块。
        }
        let index = y as usize * VEGETATION_TILE_SIDE + x as usize; // 行优先索引与GPU上传一致。
        tile.layers
            .iter()
            .find(|resolved| resolved.layer == layer)
            .and_then(|resolved| resolved.density.get(index))
            .copied()
            .unwrap_or(0.0) // 层未参与该版本时返回零。
    }
}

/// 最近一次成功原子发布的布局版本及其完整脏块集合。
///
/// GPU 后端只能在 `revision` 与 [`ActiveVegetationLayout::revision`] 相同时消费该快照；
/// 失败批次不会覆盖它，因此异步上传可以持续使用上一份完整活动布局。
#[derive(Clone, Debug, Default, Resource, PartialEq, Eq)]
pub struct VegetationLayoutCommit {
    revision: u64,                         // 与同次活动布局原子切换的版本号。
    configuration_generation: u64,         // 生成本次活动布局的完整配置与源字段代次。
    dirty_tiles: Vec<VegetationDirtyTile>, // 按Y/X稳定排序保存本批次全部块，而非处理结束后的空集合。
}

impl VegetationLayoutCommit {
    /// 返回该提交对应的活动布局版本。
    pub fn revision(&self) -> u64 {
        self.revision // 后端必须先核对版本，禁止把遗漏代次的局部列表应用到旧图集。
    }

    /// 返回生成该活动布局的配置与源字段代次。
    pub fn configuration_generation(&self) -> u64 {
        self.configuration_generation // 后端等待同代提交，禁止把新候选配置套到上一活动布局。
    }

    /// 返回本次原子提交覆盖的稳定脏块序列。
    pub fn dirty_tiles(&self) -> &[VegetationDirtyTile] {
        &self.dirty_tiles // 只读切片避免后端改变核心发布顺序。
    }

    pub(crate) fn publish(
        &mut self,
        revision: u64,                         // 与即将可见活动布局一致的版本号。
        configuration_generation: u64,         // 锁定完成全部脏块时使用的配置与源代次。
        dirty_tiles: Vec<VegetationDirtyTile>, // 保留本批次完整稳定块集合。
    ) {
        self.revision = revision; // 先保存与即将可见活动布局一致的版本身份。
        self.configuration_generation = configuration_generation; // 再绑定该布局实际消费的配置代次。
        self.dirty_tiles = dirty_tiles; // 再替换完整批次，失败批次不会调用本方法。
    }
}

/// 一层已经在CPU可见的归一化密度输入。
#[derive(Clone, Debug, PartialEq)]
pub struct VegetationSourceField {
    pub world_bounds: Rect, // 数组覆盖的世界X/Z范围。
    pub width: u32,         // 水平样本数必须大于零。
    pub height: u32,        // 垂直样本数必须大于零。
    pub values: Arc<[f32]>, // 行优先、世界min到max的零至一源数据；后台worker只克隆引用。
}

impl VegetationSourceField {
    /// 创建一个受检密度字段；资产未上传时消费者应暂不调用本方法。
    pub fn new(
        world_bounds: Rect,
        width: u32,
        height: u32,
        values: Vec<f32>,
    ) -> Result<Self, VegetationConfigError> {
        if width == 0
            || height == 0
            || values.len() != width as usize * height as usize
            || !world_bounds.min.is_finite()
            || !world_bounds.max.is_finite()
            || world_bounds.width() <= 0.0
            || world_bounds.height() <= 0.0
            || values
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(VegetationConfigError::InvalidValue {
                context: "vegetation source field requires non-empty bounds, matching dimensions and finite 0..=1 values".to_owned(),
            }); // 源数据非法时活动布局不得被部分覆盖。
        }
        Ok(Self {
            world_bounds,              // 保留项目声明的世界映射。
            width,                     // 保留最高精度输入的实际横向尺寸。
            height,                    // 保留最高精度输入的实际纵向尺寸。
            values: Arc::from(values), // 所有值已受检，转为不可变切片供多worker零深拷贝共享。
        })
    }

    /// 创建覆盖整个范围的常量源，适合程序化示例和显式落点层。
    pub fn constant(world_bounds: Rect, value: f32) -> Result<Self, VegetationConfigError> {
        Self::new(world_bounds, 1, 1, vec![value]) // 常量仍走完整归一化与范围校验。
    }

    /// 返回单个源纹素中更精细的世界边长。
    pub(crate) fn finest_texel_size(&self) -> f32 {
        let x_spacing = if self.width > 1 {
            self.world_bounds.width() / self.width.saturating_sub(1) as f32 // 端点式数值图首尾像素分别对应世界min/max。
        } else {
            f32::INFINITY // 常量单列不强迫布局获得虚假的横向精度。
        };
        let y_spacing = if self.height > 1 {
            self.world_bounds.height() / self.height.saturating_sub(1) as f32
        } else {
            f32::INFINITY // 常量单行不强迫布局获得虚假的纵向精度。
        };
        let finest = x_spacing.min(y_spacing); // 非方形输入选择最高精度有效轴。
        if finest.is_finite() {
            finest // 至少一个轴拥有两个以上端点样本。
        } else {
            self.world_bounds.width().min(self.world_bounds.height()) // 1×1常量源使用完整范围的较短边作为安全粗精度。
        }
    }

    /// 以最近邻采样世界点；范围外密度为零。
    pub(crate) fn sample(&self, world_xz: Vec2) -> f32 {
        if !self.world_bounds.contains(world_xz) {
            return 0.0; // 不把边缘纹素无限延伸到场景外。
        }
        let uv =
            (world_xz - self.world_bounds.min) / (self.world_bounds.max - self.world_bounds.min); // 世界min/max映射到首尾源样本。
        let x = (uv.x * self.width.saturating_sub(1) as f32)
            .round()
            .clamp(0.0, self.width.saturating_sub(1) as f32) as usize; // 与GPU `round(uv * (size - 1))`逐点一致。
        let y = (uv.y * self.height.saturating_sub(1) as f32)
            .round()
            .clamp(0.0, self.height.saturating_sub(1) as f32) as usize; // 行方向与世界Z同向且不翻转。
        self.values[y * self.width as usize + x] // 行优先取回归一化值。
    }

    /// 按层声明的所有权口径采样，不改变实际渲染密度的逐点采样结果。
    pub(crate) fn sample_claim(
        &self,
        world_xz: Vec2,
        sampling: VegetationClaimSampling,
        output_cell_size: f32,
    ) -> f32 {
        if sampling == VegetationClaimSampling::Point
            || !output_cell_size.is_finite()
            || output_cell_size <= 0.0
        {
            return self.sample(world_xz); // 显式候选及默认层继续使用既有最近邻点采样。
        }
        let Some(x_range) = overlapping_sample_range(
            world_xz.x,
            output_cell_size,
            self.world_bounds.min.x,
            self.world_bounds.max.x,
            self.width,
        ) else {
            return 0.0; // 输出单元与源字段横向完全分离时没有所有权声明。
        };
        let Some(y_range) = overlapping_sample_range(
            world_xz.y,
            output_cell_size,
            self.world_bounds.min.y,
            self.world_bounds.max.y,
            self.height,
        ) else {
            return 0.0; // 输出单元与源字段纵向完全分离时没有所有权声明。
        };
        let mut claim = 0.0_f32; // 任一相交源采样单元的非零值都可以声明当前输出单元。
        for y in y_range {
            for x in x_range.clone() {
                claim = claim.max(self.values[y * self.width as usize + x]); // 取最大值保留归一化声明强度并避免扩大实际密度。
            }
        }
        claim // 竞争解析器只把该值用于空间所有权。
    }
}

/// 返回与一个方形输出单元相交的端点式源采样单元索引闭区间。
fn overlapping_sample_range(
    output_center: f32,
    output_cell_size: f32,
    source_min: f32,
    source_max: f32,
    source_count: u32,
) -> Option<std::ops::RangeInclusive<usize>> {
    let center = f64::from(output_center); // f64中计算大场景索引，降低边界量化误差。
    let output_half = f64::from(output_cell_size) * 0.5; // 输出纹素按中心和完整边长参与相交。
    let minimum = f64::from(source_min); // 公共范围先提升精度再执行端点式映射。
    let maximum = f64::from(source_max); // 最大端点对应最后一个源样本中心。
    if source_count == 1 {
        let overlaps = center + output_half + CLAIM_OVERLAP_EPSILON_METERS >= minimum
            && center - output_half - CLAIM_OVERLAP_EPSILON_METERS <= maximum;
        return overlaps.then_some(0..=0); // 常量轴代表其完整声明范围，而不是一个无限采样点。
    }
    let spacing = (maximum - minimum) / f64::from(source_count - 1); // 端点式源图的相邻采样中心间距。
    let source_index = (center - minimum) / spacing; // 输出中心在连续源索引空间的位置。
    let overlap_radius =
        ((spacing + f64::from(output_cell_size)) * 0.5 + CLAIM_OVERLAP_EPSILON_METERS) / spacing; // 两单元半尺寸之和决定轴向相交范围。
    let raw_start = (source_index - overlap_radius).ceil() as i64; // 首个可能相交源单元。
    let raw_end = (source_index + overlap_radius).floor() as i64; // 最后一个可能相交源单元。
    let last = i64::from(source_count - 1); // 转为有符号索引以正确拒绝范围外输出单元。
    if raw_end < 0 || raw_start > last {
        return None; // 输出单元没有触及任何有限源单元。
    }
    let start = raw_start.clamp(0, last) as usize; // 边缘相交钳制到首个真实样本。
    let end = raw_end.clamp(0, last) as usize; // 边缘相交钳制到最后真实样本。
    (start <= end).then_some(start..=end) // 防御退化浮点范围并返回稳定闭区间。
}

/// 待处理布局的暂存状态；对外不可见以保证原子切换。
#[derive(Clone, Debug)]
pub(crate) struct PendingVegetationLayout {
    pub generation: u64,                      // 配置或源字段改变时废弃旧暂存结果。
    pub requested_revision: u64,              // 整批成功后的活动版本号。
    pub staging: ActiveVegetationLayout,      // 从上一有效版本克隆并局部覆写。
    pub dirty: BTreeSet<VegetationDirtyTile>, // 稳定坐标顺序逐帧消费。
    pub committed_dirty: Vec<VegetationDirtyTile>, // 保留批次原始全集供成功时发布，不能读取已经被消费为空的集合。
}

/// 布局核心的配置、输入、脏块队列和错误快照。
#[derive(Clone, Debug, Resource)]
pub struct VegetationLayoutState {
    pub tile_budget_per_frame: usize, // 每帧最多解析的64×64块数，零表示暂停。
    groups: Vec<VegetationCompetitionGroup>, // 当前待生效竞争组配置。
    layers: Vec<VegetationLayerConfig>, // 当前待生效无限层配置。
    sources: BTreeMap<VegetationLayerId, VegetationSourceField>, // 已上传可见的层密度源。
    generation: u64,                  // 配置或源数据每次改变递增。
    configuration_generation: u64,    // 只有组与层配置替换时递增，空间索引无需响应密度源像素换版。
    pending_dirty: BTreeSet<VegetationDirtyTile>, // 尚未开始或合并进当前批次的脏块。
    pub(crate) pending: Option<PendingVegetationLayout>, // 处理中保持活动布局不变。
    pub last_error: Option<VegetationConfigError>, // 最近一次原子拒绝原因。
}

impl Default for VegetationLayoutState {
    fn default() -> Self {
        Self {
            tile_budget_per_frame: 4,       // 正式默认平摊局部变更，避免单帧长尖峰。
            groups: Vec::new(),             // 空配置不会制造任何布局层。
            layers: Vec::new(),             // 消费者显式注册内容层。
            sources: BTreeMap::new(),       // 资产未上传前没有可解析输入。
            generation: 0,                  // 初始配置代次。
            configuration_generation: 0,    // 初始层配置代次与空空间索引身份一致。
            pending_dirty: BTreeSet::new(), // 尚无事件请求。
            pending: None,                  // 尚无暂存版本。
            last_error: None,               // 空状态不是错误。
        }
    }
}

impl VegetationLayoutState {
    /// 原子替换待生效组与层配置；真正活动版本仅在脏块全部完成后切换。
    pub fn set_configuration(
        &mut self,
        groups: Vec<VegetationCompetitionGroup>,
        layers: Vec<VegetationLayerConfig>,
    ) -> Result<(), VegetationConfigError> {
        crate::competition::validate_configuration(&groups, &layers)?; // 非法配置在写入状态前拒绝。
        self.groups = groups; // 保存完整竞争组表。
        self.layers = layers; // 保存完整正交层表。
        self.generation = self.generation.wrapping_add(1); // 让处理中旧暂存版本安全失效。
        self.configuration_generation = self.configuration_generation.wrapping_add(1); // 让近场显式落点索引只在层配置真实替换时失效。
        self.last_error = None; // 成功配置清除旧诊断。
        Ok(())
    }

    /// 注册或替换一层已经上传的基础密度源。
    pub fn set_source_field(&mut self, layer: VegetationLayerId, source: VegetationSourceField) {
        self.sources.insert(layer, source); // 相同层只保留最新完整源字段。
        self.generation = self.generation.wrapping_add(1); // 现有暂存块不能混用旧源。
        self.last_error = None; // 新完整输入已取代上一失败代次，等待本代重新受检。
    }

    /// 移除尚未就绪或已经卸载的一层源；活动布局继续保留上一版本。
    pub fn remove_source_field(&mut self, layer: VegetationLayerId) {
        if self.sources.remove(&layer).is_some() {
            self.generation = self.generation.wrapping_add(1); // 只有真实变更才失效暂存版本。
        }
    }

    /// 返回当前配置的只读层表。
    pub fn layers(&self) -> &[VegetationLayerConfig] {
        &self.layers // 后端适配器无需复制配置。
    }

    /// 返回当前配置的只读竞争组表。
    pub fn groups(&self) -> &[VegetationCompetitionGroup] {
        &self.groups // 诊断和离线验证可展示明确模式。
    }

    /// 返回一层已就绪基础密度输入。
    pub fn source_field(&self, layer: VegetationLayerId) -> Option<&VegetationSourceField> {
        self.sources.get(&layer) // None明确表示尚未上传或已卸载。
    }

    pub(crate) fn queue_dirty_rect(&mut self, min_tile: IVec2, max_tile: IVec2) {
        let min = min_tile.min(max_tile); // 调用者坐标顺序颠倒时仍解析同一区域。
        let max = min_tile.max(max_tile); // 闭区间包括四周边界块。
        for y in min.y..=max.y {
            for x in min.x..=max.x {
                self.pending_dirty
                    .insert(VegetationDirtyTile(IVec2::new(x, y))); // BTreeSet自动合并重叠请求。
            }
        }
    }

    pub(crate) fn take_pending_dirty(&mut self) -> BTreeSet<VegetationDirtyTile> {
        std::mem::take(&mut self.pending_dirty) // 新批次一次取得当前全部请求。
    }

    pub(crate) fn requeue_pending_batch(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false; // 没有处理中批次时不改变新消息队列。
        };
        self.pending_dirty.extend(pending.committed_dirty); // 失效批次必须按原始全集重算，不能只保留尚未完成部分。
        true // 调用方据此同步取消对应后台worker。
    }

    pub(crate) fn finest_texel_size(&self) -> Result<f32, VegetationConfigError> {
        let mut finest = f32::INFINITY; // 从全部当前层已就绪输入中选择最高精度。
        for layer in &self.layers {
            let Some(source) = self.sources.get(&layer.id) else {
                return Err(VegetationConfigError::InvalidValue {
                    context: format!("vegetation layer {:?} source field is not ready", layer.id),
                }); // 资产未上传时保留上一有效布局。
            };
            finest = finest.min(source.finest_texel_size()); // 越小的世界纹素精度越高。
        }
        if self.layers.is_empty() {
            Ok(1.0) // 空布局使用安全默认分辨率。
        } else if finest.is_finite() && finest > 0.0 {
            Ok(finest) // 所有字段构造已保证正范围和正尺寸。
        } else {
            Err(VegetationConfigError::InvalidValue {
                context: "vegetation layout could not determine a finite texel size".to_owned(),
            })
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation // 系统用代次识别处理中配置改变。
    }

    pub(crate) fn configuration_generation(&self) -> u64 {
        self.configuration_generation // 近场索引不把独立密度源上传误判为空间拓扑变化。
    }

    pub(crate) fn inputs(
        &self,
    ) -> (
        &[VegetationCompetitionGroup],
        &[VegetationLayerConfig],
        &BTreeMap<VegetationLayerId, VegetationSourceField>,
    ) {
        (&self.groups, &self.layers, &self.sources) // 合成器只读一次当前一致输入集。
    }

    pub(crate) fn cloned_inputs(
        &self,
    ) -> (
        Vec<VegetationCompetitionGroup>,
        Vec<VegetationLayerConfig>,
        BTreeMap<VegetationLayerId, VegetationSourceField>,
    ) {
        (
            self.groups.clone(),  // 竞争组数量很小，启动异步批次时锁定一次即可。
            self.layers.clone(),  // 显式落点与层规则保持启动代快照。
            self.sources.clone(), // 字段像素由Arc共享，克隆不会复制整张密度图。
        )
    }
}

#[cfg(test)]
mod tests; // 源字段采样与共享快照回归独立维护，资源声明文件保持聚焦。
