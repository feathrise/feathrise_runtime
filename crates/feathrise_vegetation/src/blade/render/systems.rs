//! GPU草地渲染帧级辅助系统。
//! 负责把完整RenderApp帧统计发布回主世界，并在高度或植被密度纹理未就绪时抑制首次错误绘制或保留旧结果。
use bevy::{
    prelude::*, // Query、Res和ResMut等ECS常用类型。
    render::{
        MainWorld,                  // ExtractSchedule内唯一允许写回主世界资源的官方通道。
        render_asset::RenderAssets, // 判断高度与密度纹理是否已经上传GPU。
        texture::GpuImage,          // 以GPU图片资产存在性作为地表输入就绪条件。
    },
};

use crate::components::{
    GpuBladeClipmapHeight,   // 分页输入必须保持场景、高度与视图修订一致。
    GpuBladeDensityMap,      // 可选密度图片句柄决定是否临时保留全部候选。
    GpuBladeHeightMap,       // 可选高度图片句柄决定正式高度输入是否已经就绪。
    GpuBladePatch,           // 只修改渲染世界副本的本帧绘制抑制状态。
    GpuBladePreparedBuffers, // 已完整生成过的持久输出允许资源热重载期间继续绘制。
};
use crate::resources::{
    GpuBladeRenderFrameCounters, // 取走上一帧准备、排队与实绘累计值。
    GpuBladeRenderStats,         // 主世界F3读取的不可变帧快照。
};
/// 叶片输入等待系统读取的单窗组件集合。
type GpuBladeInputReadinessQuery<'a> = (
    &'a mut GpuBladePatch,             // 只修改本帧提取副本中的等待或驻留标志。
    Option<&'a GpuBladeClipmapHeight>, // 已采用分页快照优先于旧整图接口。
    Option<&'a GpuBladeHeightMap>,     // 可选高度图片句柄决定正式高度输入是否已经就绪。
    Option<&'a GpuBladeDensityMap>,    // 可选密度图片句柄决定是否临时保留全部候选。
    Option<&'a crate::components::GpuBladeVolumeSource>, // 三维源也必须等待真实GPU图片，不能用零图清掉旧草。
    Option<&'a mut GpuBladePreparedBuffers>, // 区分首次未初始化与可安全复用的旧GPU结果。
); // 具名别名保持系统签名可读，并让四项资源契约集中维护。

/// 只有高度或植被密度输入缺失且旧输出已经生成完成时，才允许进入驻留保留路径。
fn should_retain_resident_output(inputs_are_ready: bool, output_is_valid: bool) -> bool {
    !inputs_are_ready && output_is_valid // 首次加载没有有效输出时必须继续保持零草，禁止读取未初始化缓冲。
}

/// 把上一完整RenderApp帧的草地指标复制回主世界供F3读取。 // 一帧延迟避免面板读到准备一半的计数。
pub(super) fn publish_gpu_grass_render_stats(
    mut main_world: ResMut<MainWorld>, // Bevy官方RenderApp到主世界回写入口。
    frame_counters: Res<GpuBladeRenderFrameCounters>, // 取走上一帧准备、排队与实绘累计值。
) {
    let snapshot = frame_counters.snapshot_and_reset(); // 原子取走全部字段并为当前渲染帧清零。
    if let Some(mut stats) = main_world.get_resource_mut::<GpuBladeRenderStats>() {
        *stats = snapshot; // 主世界F3后续只读取该不可变值快照。
    } // 逻辑测试或插件拆装时允许资源暂时不存在。
}

/// 任一必需叶片输入图片尚未进入GPU资产表时，只修改渲染世界副本：首次等待跳过，已有结果继续驻留。
pub(super) fn suppress_grass_until_gpu_inputs_are_ready(
    images: Res<RenderAssets<GpuImage>>, // PrepareAssets完成后可准确判断本帧是否已有真实GPU纹理。
    render_stats: Res<GpuBladeRenderFrameCounters>, // 记录提取、高度等待、密度等待和完整地表就绪窗口。
    mut grass_windows: Query<GpuBladeInputReadinessQuery<'_>>, // 逐窗口核对两类输入并读取历史输出有效性。
) {
    for (mut patch, clipmap_height, height_map, density_map, volume_source, mut prepared_buffers) in
        &mut grass_windows
    {
        if let Some(height) = clipmap_height {
            // 同一快照同时驱动高度和保守剔除包围。
            if !patch.uses_volume_source() {
                patch.set_height_field(
                    height.world_min,
                    height.world_max,
                    height.height_min,
                    height.height_max,
                ); // 三维路径的height_world保存跨界R8映射，不能被分页显示范围覆盖。
            }
            if let Some(buffers) = prepared_buffers.as_deref_mut() {
                // 旧几何视图或旧高度不得借驻留复用泄露。
                if buffers.clipmap_revision != Some(height.revision)
                    || buffers.clipmap_missing_pages != height.missing_pages
                    || !height.ready
                {
                    // 未就绪也不能继续显示不再匹配地形的草。
                    buffers.output_valid = false; // 本帧准备成功后才重新发布有效输出。
                } // 完成原子身份校验。
            } // 完成历史输出失效处理。
        } // 完成分页高度包围更新。
        let gpu_height_is_ready = if let Some(height) = clipmap_height {
            // 显式分页分支不查询 GLB 高度缓存。
            height.ready
                && images.get(&height.atlas).is_some()
                && images.get(&height.page_table).is_some() // 数组和页表缺一不可。
        } else {
            !patch.uses_height_field()
                || height_map
                    .and_then(|height_map| images.get(&height_map.image))
                    .is_some()
        }; // GLB 分支维持既有高度缓存门。
        let gpu_density_is_ready = !patch.uses_density_field()
            || density_map
                .and_then(|density_map| images.get(&density_map.image))
                .is_some(); // 同时覆盖缺少密度组件、异步加载和GPU上传尚未完成三种情况。
        render_stats.record_input_window(gpu_height_is_ready, gpu_density_is_ready); // 每个提取窗口只在本阶段记录一次叶片输入状态。
        let gpu_volume_is_ready =
            volume_source.is_none_or(|source| images.get(&source.0).is_some());
        let inputs_are_ready = gpu_height_is_ready && gpu_density_is_ready && gpu_volume_is_ready;
        let output_is_valid = prepared_buffers
            .as_deref()
            .is_some_and(|buffers| buffers.output_valid); // 仅完整生成过的持久缓冲可安全继续绘制。
        if !inputs_are_ready {
            if should_retain_resident_output(inputs_are_ready, output_is_valid) {
                patch.retain_resident_output_for_render_frame(); // 热重载保留旧真实地表结果，Prepare将跳过清零与fallback重算。
            } else {
                patch.suppress_draw_for_render_frame(); // 首次没有合法输出时仍禁止闪现全密度草或错误固定平面草。
            } // 完成有无驻留输出两条安全等待路径。
        } // 两张必需纹理都就绪后，下一帧主世界提取会自动恢复真实实例数并开始准确绘制。
    }
}

#[cfg(test)]
mod tests {
    use super::should_retain_resident_output; // 验证首次加载与热重载必须走不同安全路径。

    #[test] // 地表等待时只能复用真正完整的历史GPU结果。
    fn resident_output_retention_never_uses_uninitialized_buffers() {
        assert!(should_retain_resident_output(false, true)); // 热重载期间已有结果应继续显示。
        assert!(!should_retain_resident_output(false, false)); // 首次加载不得把未初始化缓冲当作旧草。
        assert!(!should_retain_resident_output(true, true)); // 输入就绪后应正常重算而不是永久冻结旧窗口。
    }
}
