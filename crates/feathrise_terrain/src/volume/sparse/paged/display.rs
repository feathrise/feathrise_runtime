//! 精细驻留外仍显示版本绑定的分层代理；密度卸载不等于三维地形消失。
mod build;
mod cache;
mod select;
#[cfg(test)]
mod tests;
use super::{residency::*, store::Store, types::*};
use bevy::tasks::AsyncComputeTaskPool;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// 在单个IO任务中准备完整显示集合；计算阶段await计算池，禁止在主帧同步等待。
pub async fn load_display_volume_at_foci(
    root: &Path,
    volume: &PagedVolume,
    foci: &[[f32; 3]],
    radius: i32,
) -> Result<ResidentVolume, String> {
    load_display_volume_at_foci_with_progress(root, volume, foci, radius, None).await
}

/// progress仅用于后台活动时钟；并不表示窗口已取得碰撞/显示采用信用。
pub async fn load_display_volume_at_foci_with_progress(
    root: &Path,
    volume: &PagedVolume,
    foci: &[[f32; 3]],
    radius: i32,
    progress: Option<Arc<AtomicU64>>,
) -> Result<ResidentVolume, String> {
    let mut input = load_resident_volume_at_foci(root, volume, foci, radius)?;
    build::prepare(
        root,
        volume.meshes,
        0,
        0,
        volume.cell_size,
        progress.as_deref(),
    )
    .await?;
    let mut store = Store::display(root);
    let mut rectangles: Vec<_> = input
        .columns
        .iter()
        .map(|&p| [p, [p[0] + 1, p[1] + 1]])
        .collect();
    select::append(
        root,
        &mut store,
        volume.meshes,
        0,
        0,
        volume.cell_size,
        &input.keys,
        &mut input.blocks,
        &mut rectangles,
    )?;
    let extent = volume.extent();
    // 精细块和互斥的粗节点同时聚合；旧窗口只有在这份完整集合共同Ready后退休。
    AsyncComputeTaskPool::get()
        .spawn(async move {
            let footprint = cache::merge_rectangles(rectangles, 32)
                .ok_or("当前驻留窗口超过32个覆盖矩形预算")?;
            input.columns.clear();
            let mut result = build_resident_volume(input)?;
            result.footprint = footprint
                .into_iter()
                .map(|r| r.map(|p| p.map(|v| v as f32 * extent)))
                .collect();
            Ok(result)
        })
        .await
}

fn pulse(progress: Option<&AtomicU64>) {
    if let Some(p) = progress {
        p.fetch_add(1, Ordering::Relaxed);
    }
}
