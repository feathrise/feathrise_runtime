// 传送门 — ECS 系统

use bevy::prelude::*; // Query、Res、ResMut、NextState 等

use crate::constants::{PORTAL_EDGE_BEAM_HEIGHT, PORTAL_VISUAL_RADIUS}; // 边缘光幕使用统一正式规格。
use crate::mesh::build_portal_edge_beam_mesh; // 正式网格与F12预览共享顶部渐隐构造器。
use crate::resources::{PortalLabelFont, PortalVisualMeshes}; // 字体由应用在启动前明确提供。

/// 启动时只建立一次全部传送点共享的固定网格，场景热更新不再按传送点重复分配几何资产。
pub(super) fn setup_portal_visual_meshes(
    mut commands: Commands,           // 共享句柄通过资源提供给正式传送图生成系统。
    mut meshes: ResMut<Assets<Mesh>>, // 五个小型程序网格只在启动阶段登记一次。
    label_font: Option<Res<PortalLabelFont>>, // 等应用完成PreStartup注入，不选择游戏目录或主动加载原字体。
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
        // 应用未提供字体时才在此回退；显式提供的默认句柄也必须保留，兼容中立工程。
        label_font: label_font.map(|font| font.0.clone()).unwrap_or_default(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_portal_startup_can_use_default_font_without_a_provider() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>().add_plugins(crate::PortalPlugin);
        assert!(!app.world().contains_resource::<PortalLabelFont>());
        app.update();
        assert_eq!(app.world().resource::<PortalVisualMeshes>().label_font, Handle::<Font>::default());
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 5);
        assert!(!app.world().contains_resource::<AssetServer>());
    }

    #[test]
    fn application_font_from_prestartup_reaches_shared_visuals() {
        let font = bevy::asset::uuid_handle!("96030658-569c-4e6c-b6b4-982f38dca548");
        let expected = font.clone();
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .add_plugins(crate::PortalPlugin)
            .add_systems(PreStartup, move |mut commands: Commands, supplied: Option<Res<PortalLabelFont>>| {
                // 应用通常只在尚未提供字体时加载自己的字体，SDK不能用默认占位阻止这个分支。
                if supplied.is_none() {
                    commands.insert_resource(PortalLabelFont(font.clone()));
                }
            });
        app.update();
        assert_eq!(app.world().resource::<PortalVisualMeshes>().label_font, expected);
    }

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
