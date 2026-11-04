//! F12场景页地形着色设置到玩法公共地表材质的增量同步。
//! 该调试能力只改GPU受光法线，不复制地形网格，也不触碰碰撞或作者数据。

use bevy::prelude::*; // 读取临时预览资源并增量修改地表材质资产。
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial; // 三类大地图共用的最终地表材质类型。

/// 场景页地形着色选择只在本次运行生效，不写入settings.json或地形作者文件。
#[derive(Resource, Default)]
pub struct TerrainShadingPreview {
    pub flat: bool, // 关闭F12仍用于玩家视角；切图由当前地图的临时选择决定。
}

/// 在预览或地表资产真实换代时，把当前着色模式写入地形材质。
pub(crate) fn sync_large_map_shading(
    preview: Res<TerrainShadingPreview>, // 读取默认平滑或编辑器选择的平直模式。
    mut materials: ResMut<Assets<TerrainSurfaceMaterial>>, // 只修改当前资产表内的小型材质uniform。
) {
    if !preview.is_changed() && !materials.is_changed() {
        return; // 稳定帧不扫描材质表，也不制造Modified资产事件。
    } // 按钮点击、会话重置和地图材质新建都进入同一增量入口。
    let flat_shading = preview.flat; // 冻结本轮所有材质必须采用的统一模式。
    let pending = materials
        .iter() // 先只读筛选，避免iter_mut把已经一致的材质误标为变化。
        .filter_map(|(id, material)| {
            (material.extension.flat_shading_enabled() != flat_shading).then_some(id) // 只收集确实需要翻转的稳定资产ID。
        })
        .collect::<Vec<_>>(); // 只在设置或材质换代帧分配，数量等于当前小型地表材质集合。
    for id in pending {
        if let Some(mut material) = materials.get_mut(id) {
            material.extension.set_flat_shading(flat_shading); // 修改单个uniform会触发正式材质Modified事件与Clipmap外层重包装。
        } // 资产在筛选与采用间被移除时安全跳过。
    } // 完成本次全部当前会话地表材质同步。
} // 稳定帧将在资源change tick清除后回到常数快速路径。

#[cfg(test)] // 无窗口测试覆盖既有材质与按钮后恢复路径。
mod tests {
    use super::*; // 直接运行生产同步系统。
    use bevy::pbr::StandardMaterial; // 构造最小完整扩展材质。
    use feathrise_environment::terrain_surface::{
        TerrainSurfaceExtension,
        TerrainSurfaceFallbackImages, // 使用正式直通构造器验证HN-02兼容路径。
    };

    /// 构造不需要真实图片上传的最小测试材质。
    fn test_material() -> TerrainSurfaceMaterial {
        let fallback = TerrainSurfaceFallbackImages {
            zero_weight: Handle::default(),            // 测试不读取权重图。
            white_base_color_array: Handle::default(), // 测试不读取基础色数组。
            neutral_arm_array: Handle::default(),      // 测试不读取ARM数组。
            neutral_normal_array: Handle::default(),   // 测试不读取纹理法线。
            zero_index: Handle::default(),             // 测试不读取索引图。
            neutral_layer_table: Handle::default(),    // 测试不读取地表层表。
        }; // 完成最小回退绑定夹具。
        TerrainSurfaceMaterial {
            base: StandardMaterial::default(), // 基础PBR字段与着色模式同步无关。
            extension: TerrainSurfaceExtension::new_passthrough(&fallback), // 使用正式大地图直通扩展。
        } // 返回可直接登记到Assets的完整材质。
    } // 完成测试材质构造。

    #[test] // 设置变化应切换既有材质，并在关闭后恢复平滑法线。
    fn setting_change_updates_existing_terrain_material_without_mesh_work() {
        let mut app = App::new(); // 最小ECS应用不安装渲染或AssetServer。
        app.init_resource::<TerrainShadingPreview>() // 默认值从平滑模式开始。
            .insert_resource(Assets::<TerrainSurfaceMaterial>::default()) // 手工资产表足以执行生产系统。
            .add_systems(Update, sync_large_map_shading); // 注册真实增量同步入口。
        let handle = app
            .world_mut() // 在首帧前登记一份当前地图材质。
            .resource_mut::<Assets<TerrainSurfaceMaterial>>() // 取得正式材质资产表。
            .add(test_material()); // 新材质初始使用平滑模式。
        app.update(); // 首帧同步确认默认平滑，无需网格资源。
        assert!(
            !app.world() // 读取当前材质最终uniform。
                .resource::<Assets<TerrainSurfaceMaterial>>() // 只读访问资产表。
                .get(&handle) // 按稳定强句柄定位目标材质。
                .unwrap() // 测试始终保留强句柄。
                .extension // 读取公共地表扩展。
                .flat_shading_enabled() // 查询着色预览布尔槽。
        ); // 默认模式保持平滑。
        app.world_mut() // 模拟F12按钮只修改临时预览资源。
            .resource_mut::<TerrainShadingPreview>() // 取得当前会话模式。
            .flat = true; // 请求当前大地图使用平直着色。
        app.update(); // 预览change tick触发一次小型材质同步。
        assert!(
            app.world() // 读取切换后的目标材质。
                .resource::<Assets<TerrainSurfaceMaterial>>() // 只读访问资产表。
                .get(&handle) // 句柄和网格身份都未发生变化。
                .unwrap() // 材质仍由强句柄持有。
                .extension // 读取修改后的uniform扩展。
                .flat_shading_enabled() // 验证平直模式已经应用。
        ); // 开启结果正确。
        app.world_mut() // 模拟F12关闭时的预览重置。
            .resource_mut::<TerrainShadingPreview>() // 取得唯一预览来源。
            .flat = false; // 请求恢复作者平滑法线。
        app.update(); // 同一系统原位恢复材质参数。
        assert!(
            !app.world() // 最终只读核对恢复结果。
                .resource::<Assets<TerrainSurfaceMaterial>>() // 资产表仍是同一份。
                .get(&handle) // 句柄未替换，证明不需要复制网格。
                .unwrap() // 强句柄仍然有效。
                .extension // 读取最终扩展。
                .flat_shading_enabled() // 验证平直标志已经清零。
        ); // 关闭结果恢复平滑。
    } // 完成F12材质同步回归。
} // 结束大地图着色同步测试。
