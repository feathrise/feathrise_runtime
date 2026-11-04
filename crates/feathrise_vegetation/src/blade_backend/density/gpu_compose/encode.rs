//! 主世界正式竞争输入的紧凑存储缓冲编码。

use std::sync::Arc; // 大输入在同组多个候选层之间只保留一份不可变字节。

use crate::layout::{VegetationCompetitionInputLayer, encode_competition_tile_inputs}; // 每块编码复用CPU合成器的正式源采样与稀疏足迹准备。
use crate::{
    VegetationCompetitionMode, VegetationDirtyTile, VegetationLayerId, VegetationLayoutState,
}; // 冷路径读取当前受检配置、源字段和稳定提交块。

use super::super::atlas::ResolvedDensityAtlas; // 图集提供提交块到基础级像素原点的唯一映射。
use super::{
    GPU_COMPETITION_MAX_BUFFER_BYTES, GPU_COMPETITION_MAX_LAYERS, GPU_COMPETITION_MODE_ADDITIVE,
    GPU_COMPETITION_MODE_PRIORITY, GPU_COMPETITION_MODE_WEIGHTED, VegetationGpuCompetitionInput,
}; // 打包遵守固定后端预算和WGSL模式ABI。

/// 为一个竞争组建立可由多个目标层共享的正式GPU输入。
pub(in crate::blade_backend::density) fn build_gpu_competition_input(
    layout: &VegetationLayoutState, // 当前配置与源字段必须仍对应已发布活动revision。
    atlas: &ResolvedDensityAtlas,   // 候选图集固定输出尺寸和块像素映射。
    target_layer: VegetationLayerId, // 用于找到当前组并验证目标索引。
    update_tiles: &[VegetationDirtyTile], // 完整加载块或`VegetationLayoutCommit`局部块。
) -> Result<Arc<VegetationGpuCompetitionInput>, String> {
    let target_config = layout
        .layers()
        .iter()
        .find(|layer| layer.id == target_layer)
        .ok_or_else(|| {
            format!(
                "vegetation GPU competition target layer {:?} is not configured",
                target_layer
            )
        })?; // 已移除层不能消费旧组输入并生成看似合法的图。
    let target_group = target_config.competition.group; // 同组所有绑定层共享未决需求。
    let group = *layout
        .groups()
        .iter()
        .find(|group| group.id == target_group)
        .ok_or_else(|| {
            format!(
                "vegetation GPU competition target layer {:?} references missing group {:?}",
                target_layer, target_group
            )
        })?; // 配置损坏时旧活动图片继续托底。
    let mut layers = layout
        .layers()
        .iter()
        .filter(|layer| layer.competition.group == target_group)
        .map(|layer| VegetationCompetitionInputLayer {
            layer: layer.id,                        // 稳定ID稍后决定目标GPU索引。
            priority: layer.competition.priority,   // 数值语义原样进入GPU描述。
            weight: layer.competition.weight.get(), // 受检正权重保留f32位。
        })
        .collect::<Vec<_>>();
    layers.sort_by_key(|layer| layer.layer); // 与CPU`resolve_competition`平局顺序完全相同。
    if layers.is_empty() || layers.len() > GPU_COMPETITION_MAX_LAYERS {
        return Err(format!(
            "vegetation GPU competition group {:?} has {} layers; supported range is 1..={}",
            target_group,
            layers.len(),
            GPU_COMPETITION_MAX_LAYERS
        )); // 公共总层数不受限，但单个GPU竞争组必须服从固定缓冲和位掩码预算。
    }
    if !layers.iter().any(|layer| layer.layer == target_layer) {
        return Err(format!(
            "vegetation GPU competition target layer {:?} disappeared from group {:?}",
            target_layer, target_group
        )); // 防御配置在构建阶段发生不一致借用。
    }
    let (groups, configured_layers, sources) = layout.inputs(); // 全部块只读取同一输入代次切片。
    let width = atlas.base_width()?; // 输出候选基础级宽度已经由CPU参考验证。
    let height = atlas.base_height()?; // 非正方形图集保留独立高度。
    let mut layer_bytes = Vec::with_capacity(layers.len() * 16); // 每层固定一个`vec4<u32>`。
    for layer in &layers {
        push_u32(&mut layer_bytes, layer.priority as u32); // 二补码位由WGSL`bitcast<i32>`恢复。
        push_u32(&mut layer_bytes, layer.weight.to_bits()); // WGSL`bitcast<f32>`恢复受检权重。
        push_u32(&mut layer_bytes, 0); // 第三个标量保留给后续正式输入扩展。
        push_u32(&mut layer_bytes, 0); // 第四个标量补齐十六字节数组步长。
    }
    let mut tile_bytes = Vec::with_capacity(update_tiles.len() * 16); // 每块固定一个`vec4<u32>`。
    let per_tile_samples = crate::layout::VEGETATION_TILE_TEXEL_COUNT
        .checked_mul(layers.len())
        .ok_or_else(|| "vegetation GPU competition per-tile sample count overflowed".to_owned())?;
    let sample_capacity = update_tiles
        .len()
        .checked_mul(per_tile_samples)
        .and_then(|count| count.checked_mul(8))
        .ok_or_else(|| "vegetation GPU competition sample byte capacity overflowed".to_owned())?;
    let context_capacity = update_tiles
        .len()
        .checked_mul(crate::layout::VEGETATION_TILE_TEXEL_COUNT)
        .and_then(|count| count.checked_mul(8))
        .ok_or_else(|| "vegetation GPU competition context byte capacity overflowed".to_owned())?;
    let mut sample_bytes = Vec::with_capacity(sample_capacity); // 每需求固定两个小端f32。
    let mut context_bytes = Vec::with_capacity(context_capacity); // 每纹素固定选择值和足迹影响。
    for (tile_index, tile) in update_tiles.iter().copied().enumerate() {
        let origin = atlas.tile_origin_pixels(tile.0).ok_or_else(|| {
            format!(
                "vegetation GPU competition tile {:?} lies outside candidate {}x{}",
                tile, width, height
            )
        })?; // 错位提交不能写入候选或污染未变区域。
        let sample_base = tile_index
            .checked_mul(crate::layout::VEGETATION_TILE_TEXEL_COUNT)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                "vegetation GPU competition tile sample base overflowed u32".to_owned()
            })?; // WGSL索引按纹素而不是按字节计数。
        push_u32(&mut tile_bytes, origin[0]); // 当前块基础级左上X。
        push_u32(&mut tile_bytes, origin[1]); // 当前块基础级左上Y。
        push_u32(&mut tile_bytes, sample_base); // 当前块第一个纹素在上下文数组中的索引。
        push_u32(&mut tile_bytes, 0); // 保留标志位并维持十六字节数组步长。
        let encoded = encode_competition_tile_inputs(
            tile,               // 复用当前稳定提交块坐标。
            atlas.texel_size(), // 直接复用CPU参考保存的活动布局纹素尺度。
            target_group,       // 只编码当前组未决需求。
            groups,             // 当前输入代次完整组表。
            configured_layers,  // 当前输入代次完整层表。
            sources,            // 当前输入代次完整源字段。
        )
        .map_err(|error| error.to_string())?;
        if encoded.group != group || encoded.layers != layers {
            return Err(format!(
                "vegetation GPU competition tile {:?} configuration changed while encoding",
                tile
            )); // 同一共享缓冲内禁止混合不同模式、层序或权重。
        }
        if encoded.samples.len() != per_tile_samples
            || encoded.contexts.len() != crate::layout::VEGETATION_TILE_TEXEL_COUNT
        {
            return Err(format!(
                "vegetation GPU competition tile {:?} encoded {} samples and {} contexts",
                tile,
                encoded.samples.len(),
                encoded.contexts.len()
            )); // 不完整块不能进入GPU越界索引。
        }
        for sample in encoded.samples {
            push_f32(&mut sample_bytes, sample[0]); // 实际密度保持CPU采样后的精确f32位。
            push_f32(&mut sample_bytes, sample[1]); // 所有权密度与实际密度保持正交。
        }
        for context in encoded.contexts {
            push_f32(&mut context_bytes, context[0]); // 稳定世界选择值无需WGSL复刻64位哈希。
            push_f32(&mut context_bytes, context[1]); // 稀疏足迹影响仍由GPU在竞争后乘入。
        }
    }
    ensure_non_empty_storage_bytes(&mut tile_bytes, 16); // 空零图仍需满足绑定布局最小一个UVec4。
    ensure_non_empty_storage_bytes(&mut sample_bytes, 8); // 空零图仍需满足绑定布局最小一个Vec2。
    ensure_non_empty_storage_bytes(&mut context_bytes, 8); // 空零图不调度，但绑定组保持完全合法。
    for (label, length) in [
        ("layers", layer_bytes.len()),     // 层描述通常只有几十字节。
        ("tiles", tile_bytes.len()),       // 图集8192边最多一万六千多个块。
        ("samples", sample_bytes.len()),   // 正式三层2048²约96MiB。
        ("contexts", context_bytes.len()), // 正式2048²约32MiB。
    ] {
        if length > GPU_COMPETITION_MAX_BUFFER_BYTES {
            return Err(format!(
                "vegetation GPU competition {label} buffer {length} bytes exceeds fixed {} byte budget",
                GPU_COMPETITION_MAX_BUFFER_BYTES
            )); // 超预算候选原子拒绝并继续使用上一有效图片。
        }
    }
    let (mode, mode_value) = match group.mode {
        VegetationCompetitionMode::Additive => (GPU_COMPETITION_MODE_ADDITIVE, 0.0), // 叠加模式没有额外标量。
        VegetationCompetitionMode::WeightedPartition { capacity } => {
            (GPU_COMPETITION_MODE_WEIGHTED, capacity) // GPU执行同优先级封顶水填充。
        }
        VegetationCompetitionMode::PriorityOverride { claim_threshold } => {
            (GPU_COMPETITION_MODE_PRIORITY, claim_threshold) // GPU按声明密度选择最高优先级所有者。
        }
    };
    Ok(Arc::new(VegetationGpuCompetitionInput {
        group: target_group,            // 后续同组绑定层可复用本输入。
        width,                          // 候选尺寸进入Prepare资源核对。
        height,                         // 候选尺寸进入Prepare资源核对。
        tiles: Arc::from(update_tiles), // 稳定块序列用于共享计划核对。
        layer_ids: Arc::from(layers.iter().map(|layer| layer.layer).collect::<Vec<_>>()), // 小ID数组不进入GPU，仅供目标索引查找。
        layer_bytes: Arc::from(layer_bytes), // 大输入之后只通过Arc跨世界克隆。
        tile_bytes: Arc::from(tile_bytes),   // 每块GPU输出原点和样本起点。
        sample_bytes: Arc::from(sample_bytes), // 尚未执行组内竞争的正式需求。
        context_bytes: Arc::from(context_bytes), // 稳定选择值与足迹影响。
        mode,                                // WGSL稳定模式常量。
        mode_value,                          // 容量、阈值或零。
    }))
}

/// 追加一个小端u32并保持WGSL标量位表示。
fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes()); // WebGPU主机共享缓冲按本项目小端Windows目标编码。
}

/// 追加一个小端f32并保持CPU采样位表示。
fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_bits().to_le_bytes()); // 不经过文本或半精度量化。
}

/// 为空作业补一个布局允许的最小存储元素。
fn ensure_non_empty_storage_bytes(bytes: &mut Vec<u8>, minimum: usize) {
    if bytes.is_empty() {
        bytes.resize(minimum, 0); // tile_count为零时Shader不会读取占位字节。
    }
}
