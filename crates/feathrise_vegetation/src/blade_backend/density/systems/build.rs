//! CPU参考选择与未公开GPU候选批次构建。

use std::collections::BTreeMap; // 批次按稳定层ID选择串行GPU作业。
use std::sync::Arc; // 紧凑图集和完整比较字节只共享强引用。

use bevy::prelude::{Assets, Handle, Image}; // 全部普通值成功后才把候选加入图片资产表。

use crate::{
    ActiveVegetationLayout, VegetationCompetitionGroupId, VegetationDirtyTile, VegetationLayerId,
    VegetationLayoutCommit, VegetationLayoutState,
}; // CPU参考读取活动快照；GPU输入另读取同代正式配置和源字段。

use super::super::atlas::{
    ResolvedDensityAtlas, build_layer_density_atlas, full_layer_update_tiles,
    patch_layer_density_atlas,
}; // 完整重建和脏块局部重建必须产生完全相同的CPU权威。
use super::super::gpu_compose::{VegetationGpuCompetitionInput, build_gpu_competition_input}; // 同组多个目标层共享未决正式输入，最终密度只在GPU生成。
use super::super::resources::{
    CachedBladeDensity, PendingBladeDensity, PendingBladeDensityBatch, PendingBladeDensityState,
    VegetationBladeDensityCache, VegetationGpuBaseJob,
}; // 构造尚未公开的整批候选状态。

/// 一层在写入Assets表前已经完整验证的候选值。
struct BuiltBladeDensityCandidate {
    layer: VegetationLayerId,                  // 稳定层ID决定批次BTreeMap键。
    atlas: Arc<ResolvedDensityAtlas>,          // 紧凑CPU参考供当前比较和下一版本局部重建。
    expected: Arc<[u8]>,                       // 基础级至1×1的完整RGBA字节权威。
    image: Image,                              // 基础级和子Mip均为零的未公开GPU竞争候选。
    input: Arc<VegetationGpuCompetitionInput>, // 当前组尚未解析的正式GPU输入。
    target_layer_index: u32,                   // 当前层在稳定组描述中的索引。
    previous_image: Option<Handle<Image>>,     // 局部更新先在GPU复制上一有效基础级。
}

/// 后台纯CPU阶段建立的完整跨层候选，尚未登记任何图片资产。
pub(in crate::blade_backend::density) struct BuiltBladeDensityBatch {
    ticket: u64,                                 // 启动CPU任务时已经消费的唯一票据。
    revision: u64,                               // 候选只允许登记到同一活动布局版本。
    configuration_generation: u64,               // GPU在途期间继续核对生成布局的配置代次。
    candidates: Vec<BuiltBladeDensityCandidate>, // 所有可能失败的CPU步骤已经全部成功。
}

impl BuiltBladeDensityBatch {
    /// 登记Assets前防御核对后台产物的完整主世界身份。
    pub(in crate::blade_backend::density) fn matches(
        &self,
        ticket: u64,                   // 当前协调器已经消费的票据。
        revision: u64,                 // 当前活动布局版本。
        configuration_generation: u64, // 当前正式输入代次。
    ) -> bool {
        self.ticket == ticket
            && self.revision == revision
            && self.configuration_generation == configuration_generation // 三项都一致才允许登记未公开Image。
    }
}

/// 一次原子密度候选构建所需的只读身份和共享资源。
pub(in crate::blade_backend::density) struct PendingBladeDensityBatchBuild<'a> {
    pub(in crate::blade_backend::density) ticket: u64, // 新批次稳定票据。
    pub(in crate::blade_backend::density) active: &'a ActiveVegetationLayout, // 当前完整活动快照。
    pub(in crate::blade_backend::density) commit: &'a VegetationLayoutCommit, // 同版本脏块清单。
    pub(in crate::blade_backend::density) layout: &'a VegetationLayoutState, // 正式配置和源字段用于GPU输入编码。
    pub(in crate::blade_backend::density) bound_layers: &'a [VegetationLayerId], // 全部显式绑定层必须一起成功。
    pub(in crate::blade_backend::density) cache: &'a VegetationBladeDensityCache, // 上一有效图集只读参与局部重建。
}

/// 从CPU参考建立一个可在线程池运行、尚未登记资产的完整候选值。
pub(in crate::blade_backend::density) fn build_pending_batch_cpu(
    build: PendingBladeDensityBatchBuild<'_>, // 具名上下文避免位置参数错配。
) -> Result<BuiltBladeDensityBatch, (VegetationLayerId, String)> {
    let PendingBladeDensityBatchBuild {
        ticket,
        active,
        commit,
        layout,
        bound_layers,
        cache,
    } = build; // 后续算法仍使用短局部名表达同一批次的不变量。
    let mut built = Vec::with_capacity(bound_layers.len()); // 先在普通值中完成全部可能失败步骤。
    let mut group_inputs =
        BTreeMap::<VegetationCompetitionGroupId, Arc<VegetationGpuCompetitionInput>>::new(); // 同组正式层共享一次可能较大的源采样编码。
    for layer in bound_layers {
        let reference = resolve_gpu_reference_plan(active, commit, *layer, cache)
            .map_err(|reason| (*layer, reason))?; // 任一层失败前不向Assets表留下部分批次。
        let expected = reference
            .atlas
            .rgba_mip_bytes()
            .map(Arc::<[u8]>::from)
            .map_err(|reason| (*layer, reason))?; // 主世界保留全部Mip字节作为GPU回读权威。
        let image = reference
            .atlas
            .to_gpu_competition_candidate_image()
            .map_err(|reason| (*layer, reason))?; // CPU参考基础级也不进入正式候选上传。
        let group = layout
            .layers()
            .iter()
            .find(|configured| configured.id == *layer)
            .map(|configured| configured.competition.group)
            .ok_or_else(|| {
                (
                    *layer,
                    format!(
                        "vegetation GPU competition target layer {:?} is not configured",
                        layer
                    ),
                )
            })?;
        let width = reference
            .atlas
            .base_width()
            .map_err(|reason| (*layer, reason))?;
        let height = reference
            .atlas
            .base_height()
            .map_err(|reason| (*layer, reason))?;
        let input = if let Some(input) = group_inputs.get(&group) {
            if !input.matches_plan(width, height, &reference.update_tiles) {
                return Err((
                    *layer,
                    format!(
                        "vegetation GPU competition group {:?} bound layers use different atlas or dirty plans",
                        group
                    ),
                )); // 大输入只能在输出几何和提交块完全相同时复用。
            }
            input.clone() // 后续同组层只克隆Arc，不重复编码全部正式纹素。
        } else {
            let input = build_gpu_competition_input(
                layout,
                &reference.atlas,
                *layer,
                &reference.update_tiles,
            )
            .map_err(|reason| (*layer, reason))?;
            group_inputs.insert(group, input.clone()); // 首层成功后供同组后续层共享。
            input // 当前层使用刚建立的同一Arc。
        };
        let target_layer_index = input
            .target_layer_index(*layer)
            .map_err(|reason| (*layer, reason))?; // 错组或缺层在GPU资源创建前原子拒绝。
        built.push(BuiltBladeDensityCandidate {
            layer: *layer,                            // 保留当前稳定键。
            atlas: Arc::new(reference.atlas),         // 后续字段从同一CPU权威图集派生。
            expected,                                 // 比较字节不依赖Image提取后被取走的数据。
            image,                                    // 此时尚未加入资产表且全部级为零。
            input,                                    // 仍未执行组内竞争的共享正式输入。
            target_layer_index,                       // WGSL只写当前串行层。
            previous_image: reference.previous_image, // `None`表示整图从零基础级构建。
        });
    }

    Ok(BuiltBladeDensityBatch {
        ticket,                                                      // 纯CPU产物携带启动时票据。
        revision: active.revision,                                   // 纯CPU产物携带活动布局版本。
        configuration_generation: commit.configuration_generation(), // 纯CPU产物携带正式提交代次。
        candidates: built, // 此时没有任何Image进入主世界Assets。
    })
}

/// 在主世界为仍属当前身份的完整CPU产物登记图片并组装GPU批次。
pub(in crate::blade_backend::density) fn register_built_batch(
    built: BuiltBladeDensityBatch, // 过期结果在调用本函数前直接Drop，不污染Assets。
    started_update: u64,           // GPU超时从CPU完成并登记资产后开始计算。
    images: &mut Assets<Image>,    // 只有本函数可以修改主世界图片资产表。
) -> PendingBladeDensityBatch {
    let mut layers = BTreeMap::new(); // 资产加入后构造稳定候选映射。
    for candidate in built.candidates {
        let world_min = candidate.atlas.world_min; // 在克隆Arc前读取同源世界下界。
        let world_max = candidate.atlas.world_max; // 世界上界只在整批成功时写入窗口。
        let image = images.add(candidate.image); // 每层候选使用独立句柄，不覆盖当前有效GPU纹理。
        layers.insert(
            candidate.layer,
            PendingBladeDensity {
                cached: CachedBladeDensity {
                    revision: built.revision, // 候选只可能成为完成CPU构建时的活动版本。
                    image,                    // GPU原地生成子Mip的同一图片最终直接成为活动图。
                    atlas: candidate.atlas,   // 保留CPU紧凑链供下一版本脏块重建。
                    world_min,                // 与候选基础级像素范围同源。
                    world_max,                // 与候选基础级像素范围同源。
                },
                expected: candidate.expected, // 成功前保留逐字节比较权威。
                base: VegetationGpuBaseJob::Competition {
                    input: candidate.input, // RenderApp创建只读存储缓冲并执行正式竞争。
                    target_layer_index: candidate.target_layer_index, // 当前串行层稳定索引。
                    previous_image: candidate.previous_image, // 局部提交先复制旧基础级，整图不复制。
                },
                state: PendingBladeDensityState::Waiting, // 初始尚未向RenderApp发布。
            },
        ); // 每层恰有一份未公开候选。
    }
    PendingBladeDensityBatch {
        ticket: built.ticket,                                     // 串行层共享批次票据。
        revision: built.revision,                                 // 最终采用前再次核对活动版本。
        configuration_generation: built.configuration_generation, // 配置热更会取消未公开旧代候选。
        layers,                                                   // BTreeMap保证下一层选择稳定。
        started_update,                                           // 记录候选CPU构建完成的Update。
        failure: None, // 首个GPU错误或字节差异才写入原因。
    }
}

/// 一层CPU权威与GPU基础级更新计划。
struct ResolvedGpuReferencePlan {
    atlas: ResolvedDensityAtlas,            // P5.2a完整CPU链只用于回读比较。
    update_tiles: Vec<VegetationDirtyTile>, // 完整驻留块或当前提交块。
    previous_image: Option<Handle<Image>>,  // 局部更新从上一已验证GPU图复制基础级。
}

/// 为正式GPU候选选择局部提交或安全整图更新计划。
fn resolve_gpu_reference_plan(
    active: &ActiveVegetationLayout,     // 当前活动布局完整快照。
    commit: &VegetationLayoutCommit,     // 当前原子提交的脏块全集。
    layer: VegetationLayerId,            // 正在构建的绑定层。
    cache: &VegetationBladeDensityCache, // 上一有效GPU图和CPU参考可能可局部复用。
) -> Result<ResolvedGpuReferencePlan, String> {
    let previous = cache.layers.get(&layer); // 成功前只读上一有效候选。
    if let Some(previous) = previous
        && previous.revision.wrapping_add(1) == active.revision
        && commit.revision() == active.revision
        && let Some(atlas) =
            patch_layer_density_atlas(previous.atlas.as_ref(), active, layer, commit.dirty_tiles())?
    {
        let update_tiles = commit
            .dirty_tiles()
            .iter()
            .copied()
            .filter(|tile| atlas.tile_origin_pixels(tile.0).is_some())
            .collect::<Vec<_>>(); // 只把当前图集内提交块编码为基础级Z切片。
        return Ok(ResolvedGpuReferencePlan {
            atlas,                                        // CPU局部参考已经传播全部受影响父Mip。
            update_tiles,                                 // GPU只重算相同提交块的基础级。
            previous_image: Some(previous.image.clone()), // 未变基础级来自上一已验证GPU纹理。
        });
    }
    let atlas = build_layer_density_atlas(active, layer)?; // 首次、遗漏代次或范围改变都整图回退。
    let update_tiles = full_layer_update_tiles(active, layer)
        .into_iter()
        .filter(|tile| atlas.tile_origin_pixels(tile.0).is_some())
        .collect::<Vec<_>>(); // 候选初始全零，因此只调度真正驻留目标层的块。
    Ok(ResolvedGpuReferencePlan {
        atlas,                // 完整CPU链仍只用于最终逐字节权威。
        update_tiles,         // GPU从零基础级生成全部驻留块。
        previous_image: None, // 范围改变时禁止复制错位旧图。
    })
}

/// 为一层选择相邻版本局部参考重建或安全整图回退。
pub(super) fn resolve_reference_atlas(
    active: &ActiveVegetationLayout,     // 当前活动布局完整快照。
    commit: &VegetationLayoutCommit,     // 当前原子提交的脏块全集。
    layer: VegetationLayerId,            // 正在构建的绑定层。
    cache: &VegetationBladeDensityCache, // 上一有效图集可能跨GPU失败版本而不相邻。
) -> Result<ResolvedDensityAtlas, String> {
    let previous = cache.layers.get(&layer); // 成功构造前只读上一有效资产。
    if let Some(previous) = previous
        && previous.revision.wrapping_add(1) == active.revision
        && commit.revision() == active.revision
    {
        return match patch_layer_density_atlas(
            previous.atlas.as_ref(),
            active,
            layer,
            commit.dirty_tiles(),
        )? {
            Some(resolved) => Ok(resolved), // 范围稳定时只重建提交块及受影响父Mip纹素。
            None => build_layer_density_atlas(active, layer), // 范围改变时禁止把局部字节写入旧分配。
        };
    }
    build_layer_density_atlas(active, layer) // 首次绑定、遗漏代次或提交不匹配时完整构建。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接构造纯CPU产物并验证唯一Assets登记边界。
    use crate::VegetationCompetitionGroupId; // 最小GPU输入仍携带稳定竞争组身份。
    use crate::blade_backend::density::gpu_compose::{
        GPU_COMPETITION_MODE_ADDITIVE, VegetationGpuCompetitionInput,
    }; // 候选只需一个合法但不执行的最小正式GPU输入。

    #[test]
    fn built_cpu_batch_does_not_touch_assets_until_main_world_registration() {
        let layer = VegetationLayerId(71); // 任意稳定测试层。
        let active = ActiveVegetationLayout {
            revision: 4, // 纯CPU产物锁定第四版。
            ..Default::default()
        };
        let atlas = build_layer_density_atlas(&active, layer)
            .expect("empty layer should produce a safe atlas"); // 1×1零图足以验证资产生命周期。
        let expected = Arc::<[u8]>::from(
            atlas
                .rgba_mip_bytes()
                .expect("safe atlas bytes should build"),
        ); // CPU完整链在登记前已经完成。
        let image = atlas
            .to_gpu_competition_candidate_image()
            .expect("safe candidate image should build"); // Image仍只是普通未登记值。
        let input = Arc::new(VegetationGpuCompetitionInput {
            group: VegetationCompetitionGroupId(1), // 最小稳定组身份。
            width: 1,                               // 安全候选基础级宽。
            height: 1,                              // 安全候选基础级高。
            tiles: Arc::from([]),                   // 空布局无需调度块。
            layer_ids: Arc::from([layer]),          // 当前测试候选位于索引零。
            layer_bytes: Arc::from([0_u8; 16]),     // 最小一个层描述。
            tile_bytes: Arc::from([0_u8; 16]),      // 空作业仍保持绑定最小尺寸。
            sample_bytes: Arc::from([0_u8; 8]),     // 空作业不读取占位样本。
            context_bytes: Arc::from([0_u8; 8]),    // 空作业不读取占位上下文。
            mode: GPU_COMPETITION_MODE_ADDITIVE,    // 使用无额外参数的叠加模式。
            mode_value: 0.0,                        // 叠加模式固定零。
        });
        let built = BuiltBladeDensityBatch {
            ticket: 9,                   // 模拟启动时已消费票据。
            revision: active.revision,   // 绑定当前活动版本。
            configuration_generation: 3, // 绑定任意稳定配置代次。
            candidates: vec![BuiltBladeDensityCandidate {
                layer,                  // 候选层身份。
                atlas: Arc::new(atlas), // 保存CPU权威图集。
                expected,               // 保存完整比较字节。
                image,                  // 保存未登记Image普通值。
                input,                  // 保存正式GPU输入。
                target_layer_index: 0,  // 单层组目标索引为零。
                previous_image: None,   // 首次候选不复制旧图。
            }],
        };
        let mut images = Assets::<Image>::default(); // 创建空主世界资产表。
        assert!(images.is_empty()); // 构造完整CPU产物没有隐式登记任何Image。
        assert!(built.matches(9, 4, 3)); // 当前票据、版本和配置代可以进入登记边界。
        assert!(!built.matches(9, 4, 4)); // 过期配置代结果必须在登记前被拒绝。
        assert!(images.is_empty()); // 身份核对和丢弃过期产物都不修改Assets。
        let batch = register_built_batch(built, 12, &mut images); // 只有主世界显式调用才登记资产。
        assert_eq!(images.len(), 1); // 单层候选恰好新增一个未公开Image。
        assert_eq!(batch.started_update, 12); // GPU超时从登记帧而非CPU启动帧开始。
        assert_eq!(batch.configuration_generation, 3); // 后续GPU在途继续保留同代身份。
    }
}
