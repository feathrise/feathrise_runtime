//! 将实际可见地形的已采用分页高度轻量同步到既有四级草窗。
use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes}; // 每个子场景借用独立状态，父图保留原资源。
use super::{components::DynamicBladeWindow, resources::DynamicVegetationState};
use crate::scenes::dynamic_scene::{
    clipmap::SparseHeightPageRuntime,
    sparse_runtime::{SparseClipmapDisplay, SparseClipmapRuntime},
}; // 唯一高度与几何所有权。
use bevy::prelude::*; // 只读地形资源和有界窗口写入。
use feathrise_environment::vegetation::blade::{GpuBladeClipmapHeight, GpuBladePatch}; // 通用植被组件不反向依赖scene。
use bevy::camera::primitives::Aabb; // CPU视锥包围也必须随完整高度页换代。
use feathrise_scene_content::ActiveDynamicScene; // 切换场景身份门。 // 窗口数量由正式作者层决定。

pub(in crate::scenes::dynamic_scene) fn sync_dynamic_blade_clipmap(
    // 不重建密度、预设或CPU散布。
    active: InstanceRes<ActiveDynamicScene>,       // 当前场景身份。
    pages: InstanceRes<SparseHeightPageRuntime>, // 已采用高度页。
    geometry: InstanceRes<SparseClipmapRuntime>, // 已安装显示几何。
    displays: InstanceQuery<&Transform, With<SparseClipmapDisplay>>, // 真实吸附中心。
    state: InstanceRes<DynamicVegetationState>,  // 新根修订也触发一次同步。
    mut windows: InstanceQuery<(&mut GpuBladeClipmapHeight, &mut GpuBladePatch, &mut Aabb, &Transform), With<DynamicBladeWindow>>, // 仅分页Blade。
    mut applied: Local<Option<([u64; 3], bool, u32, u32, Option<Entity>, [UVec4; 16])>>, // 稳定帧跳过全部窗口遍历。
    mut invalidated_root: Local<Option<Entity>>, // 缺页期间新建根仍必须撤销捕获时的旧ready。
) {
    // 一个完整轻量快照服务全部LOD。
    let height = (|| {
        // 缺失任意同代输入都必须撤销旧快照就绪。
        let map = active.scene_id.as_ref()?; // 当前场景不可缺失。
        let (display, config) = geometry.blade_geometry(map)?; // 几何身份必须匹配。
        let transform = displays.get(display).ok()?; // 正式显示实体仍存在。
        let center = Vec2::new(transform.translation.x, transform.translation.z); // 实际吸附中心。
        pages.blade_render_height(map, config, center) // 与当前屏上地形同版；新雕刻候选不能提前关闭旧草。
    })(); // 此查询只复制固定大小句柄。
    let Some(height) = height else {
        // 切换场景、失效和重建不能继续显示旧图草。
        if applied.take().is_some() || *invalidated_root != state.active_root {
            // 失效边沿或异步新根只处理一次。
            for (mut window, _, _, _) in &mut windows {
                window.ready = false;
            } // Compute在本帧禁止旧分页绘制。
            *invalidated_root = state.active_root; // 缺页期间稳定帧不再遍历。
        } // 持续等待帧保持常数开销。
        return; // 缺失资源不伪造平地。
    }; // 有效快照继续按联合身份发布。
    let identity = (
        height.revision,
        height.ready,
        state.generation,
        state.revision,
        state.active_root,
        height.missing_pages,
    ); // ready边沿也禁止旧页假平地。
    if applied.as_ref() == Some(&identity) {
        return;
    } // 稳定帧常数工作量。
    for (mut window, mut patch, mut bounds, transform) in &mut windows {
        // 仅身份边沿更新既有GPU绑定。
        sync_height_bounds(&mut patch, &mut bounds, transform.translation.y, &window, &height);
        *window = height.clone(); // 同帧完整替换页、参数与视图。
    } // 不请求任何全图高度或密度重算。
    *applied = Some(identity); // 记录此次完整发布。
} // RenderApp在后续提取阶段读取同一版本。

/// 保留预设原有叶尖/根部余量，只更新地表的真实米制区间；坐标仍在当前网格的源框架内。
fn sync_height_bounds(patch: &mut GpuBladePatch, bounds: &mut Aabb, origin_y: f32,
    old: &GpuBladeClipmapHeight, new: &GpuBladeClipmapHeight) {
    let old_min = f64::from(origin_y) + f64::from(bounds.center.y) - f64::from(bounds.half_extents.y);
    let old_max = f64::from(origin_y) + f64::from(bounds.center.y) + f64::from(bounds.half_extents.y);
    let lower_margin = (f64::from(old.height_min) - old_min).max(0.0);
    let upper_margin = (old_max - f64::from(old.height_max)).max(0.0);
    let min = (f64::from(new.height_min) - lower_margin).max(-f64::from(f32::MAX));
    let max = (f64::from(new.height_max) + upper_margin).min(f64::from(f32::MAX));
    bounds.center.y = ((min + max) * 0.5 - f64::from(origin_y)) as f32;
    bounds.half_extents.y = ((max - min) * 0.5) as f32;
    patch.set_height_field(new.world_min, new.world_max, new.height_min, new.height_max);
}
