//! 无RenderApp逻辑测试使用的CPU完整Mip安全回退。

use std::sync::Arc; // 成功CPU图集保留紧凑参考链供下一版本局部重建。

use bevy::prelude::{Assets, Commands, Image, MessageWriter, Query}; // 回退只维护主世界图片、窗口和拒绝消息。

use crate::{
    ActiveVegetationLayout, VegetationLayerId, VegetationLayoutCommit, VegetationLayoutRejected,
}; // 回退仍遵守活动版本和原子提交边界。

use super::super::resources::{CachedBladeDensity, VegetationBladeDensityCache}; // 成功前保留上一有效缓存。
use super::build::resolve_reference_atlas; // 与GPU候选使用同一CPU完整Mip参考路径。
use super::{BladeDensityWindowQuery, apply_current_cache}; // 成功后沿用同一窗口原子采用逻辑。

/// 无RenderApp逻辑测试使用CPU完整Mip图片保持既有确定性行为。
pub(super) fn sync_cpu_density_fallback(
    commands: &mut Commands,                 // 每层完整CPU构建成功后写窗口。
    active: &ActiveVegetationLayout,         // 当前活动版本。
    commit: &VegetationLayoutCommit,         // 相邻版本脏块清单。
    images: &mut Assets<Image>,              // 回退路径可原句柄更新以保持轻量。
    cache: &mut VegetationBladeDensityCache, // 成功前保留上一完整图。
    rejected: &mut MessageWriter<VegetationLayoutRejected>, // 首次转换失败发布诊断。
    windows: &mut Query<BladeDensityWindowQuery<'_>>, // 逐窗口复用同层缓存。
) {
    for (_, binding, _, _) in windows.iter_mut() {
        if cache
            .layers
            .get(&binding.0)
            .is_some_and(|cached| cached.revision == active.revision)
        {
            continue; // 同层多个窗口和稳定帧只构建一次完整图片。
        }
        if cache.rejected.contains(&(active.revision, binding.0)) {
            continue; // 同一失败版本不重复构建。
        }
        match resolved_cpu_density_for_layer(active, commit, binding.0, images, cache) {
            Ok(()) => {} // 成功缓存稍后统一应用全部窗口。
            Err(reason) => {
                if cache.rejected.insert((active.revision, binding.0)) {
                    rejected.write(VegetationLayoutRejected { reason }); // 失败时窗口及上一有效图片保持不变。
                }
            }
        }
    }
    apply_current_cache(commands, active.revision, cache, windows); // 只有完整CPU图成功的层会满足当前版本门槛。
}

/// 构造并提交一层CPU完整Mip回退图片。
fn resolved_cpu_density_for_layer(
    active: &ActiveVegetationLayout,         // 当前活动布局。
    commit: &VegetationLayoutCommit,         // 当前版本提交脏块。
    layer: VegetationLayerId,                // 目标绑定层。
    images: &mut Assets<Image>,              // 成功后更新或创建当前层图片。
    cache: &mut VegetationBladeDensityCache, // 上一图集提供局部重建输入。
) -> Result<(), String> {
    let resolved = resolve_reference_atlas(active, commit, layer, cache)?; // 先在普通值中完成所有可失败计算。
    let resolved_image = resolved.to_image()?; // 无RenderApp路径上传CPU完整链。
    let previous = cache.layers.get(&layer).cloned(); // 写资产前保存上一有效句柄。
    let image = if let Some(previous) = previous.as_ref() {
        if let Some(mut existing) = images.get_mut(&previous.image) {
            *existing = resolved_image; // 逻辑回退不涉及在途GPU候选，可安全复用原句柄。
            previous.image.clone() // 同层窗口继续持有相同强句柄。
        } else {
            images.add(resolved_image) // 外部清理资产表时恢复新强句柄。
        }
    } else {
        images.add(resolved_image) // 首次成功版本建立唯一图片资产。
    };
    let world_min = resolved.world_min; // 在移入Arc前保存同源映射下界。
    let world_max = resolved.world_max; // 在移入Arc前保存同源映射上界。
    cache.layers.insert(
        layer,
        CachedBladeDensity {
            revision: active.revision, // CPU回退同样只标记完整成功版本。
            image,                     // 当前层共享图片句柄。
            atlas: Arc::new(resolved), // 保留完整紧凑链供下一版本局部更新。
            world_min,                 // 与基础级范围同源。
            world_max,                 // 与基础级范围同源。
        },
    ); // 全部字节生成和资产更新成功后才替换缓存记录。
    Ok(())
}
