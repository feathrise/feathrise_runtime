// 传送门 — ECS 系统

use bevy::prelude::*; // Query、Res、ResMut、NextState 等

use crate::constants::{PORTAL_EDGE_BEAM_HEIGHT, PORTAL_VISUAL_RADIUS}; // 边缘光幕使用统一正式规格。
use crate::mesh::build_portal_edge_beam_mesh; // 正式网格与F12预览共享顶部渐隐构造器。
use crate::resources::{PortalLabelFont, PortalVisualMeshes}; // 字体由应用在启动前明确提供。

/// 启动时只建立一次全部传送点共享的固定网格，场景热更新不再按传送点重复分配几何资产。
pub(super) fn setup_portal_visual_meshes(
    mut commands: Commands,           // 共享句柄通过资源提供给正式传送图生成系统。
    mut meshes: ResMut<Assets<Mesh>>, // 五个小型程序网格只在启动阶段登记一次。
    label_font: Res<PortalLabelFont>, // 只消费已经提供的句柄，不选择游戏目录或主动加载原字体。
    existing: Option<Res<PortalVisualMeshes>>, // 多插件测试或热重建不得覆盖仍被实体使用的共享句柄。
) {
    if existing.is_some() {
        return; // 重复安装PortalPlugin时保持第一套稳定网格身份。
    }
    commands.insert_resource(PortalVisualMeshes {
        base: meshes.add(Mesh::from(Circle::new(PORTAL_VISUAL_RADIUS))), // 原深色圆盘继续表达实际触发范围。
        arrow_head: meshes.add(Mesh::from(Triangle2d::new(
            Vec2::new(0.0, 1.8),  // 箭头尖端沿局部正Y，铺地后对应场景北向。
            Vec2::new(-1.2, 0.0), // 左侧基点保持原路标宽度。
            Vec2::new(1.2, 0.0),  // 右侧基点与左侧对称。
        ))),
        arrow_shaft: meshes.add(Mesh::from(Rectangle::new(0.6, 1.6))), // 杆身保持原有紧凑比例。
        edge_ring: meshes.add(Mesh::from(Torus::new(2.2, PORTAL_VISUAL_RADIUS))), // 外缘光环贴近实际传送范围边界。
        edge_beam_wall: meshes.add(build_portal_edge_beam_mesh(
            PORTAL_VISUAL_RADIUS,    // 光幕严格沿实际触发边缘生成。
            PORTAL_EDGE_BEAM_HEIGHT, // 提高后的十四米顶部通过顶点Alpha渐隐。
        )),
        label_font: label_font.0.clone(), // 固定网格与应用字体共同持有稳定资产身份。
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_portal_startup_needs_no_player_session_or_asset_server() {
        let font = bevy::asset::uuid_handle!("b94ec067-8e3e-4ca7-abdf-d4aa27507e26");
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .insert_resource(PortalLabelFont(font.clone()))
            .add_plugins(crate::PortalPlugin);
        app.update();
        let initial = app.world().resource::<PortalVisualMeshes>().base.id();
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 5);
        assert_eq!(app.world().resource::<PortalVisualMeshes>().label_font, font);
        app.update();
        assert_eq!(app.world().resource::<PortalVisualMeshes>().base.id(), initial);
        assert!(!app.world().contains_resource::<AssetServer>());
    }
}
