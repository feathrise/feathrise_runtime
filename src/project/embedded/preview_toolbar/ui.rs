//! 可拖拽的紧凑预览HUD；直接覆盖游戏主视图，不占整行或调整游戏相机视口。

use super::components::*;
use bevy::{camera::visibility::RenderLayers, prelude::*, ui::FocusPolicy};

pub(super) const NORMAL: Color = Color::srgba(0.12, 0.14, 0.17, 0.82);
pub(super) const HOVERED: Color = Color::srgba(0.24, 0.29, 0.35, 0.96);
pub(super) const SELECTED: Color = Color::srgba(0.16, 0.36, 0.49, 0.96);

pub(super) fn spawn(mut commands: Commands) {
    // 后备相机仅在活跃主视图缺席时显示返回UI；正常预览由present把根显式绑定游戏主相机。
    // 不接管游戏HUD默认相机，也不让独立LDR目标覆盖主相机HDR画面。
    let camera = commands.spawn((ToolbarCamera, Camera2d, Camera {
        order: 1000,
        is_active: false,
        clear_color: ClearColorConfig::None,
        ..default()
    }, RenderLayers::none(), Name::new("工程预览工具条相机"))).id();
    commands.spawn((ToolbarRoot, ToolbarPointer, Interaction::None, UiTargetCamera(camera),
        Node { display: Display::None, position_type: PositionType::Absolute,
            top: px(12), left: percent(50), height: px(34), flex_shrink: 0.,
            align_items: AlignItems::Center, column_gap: px(4),
            padding: UiRect::all(px(3)), border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(7)), ..default() },
        // 用自身宽度的一半平移，初次完成文字布局时即可顶部居中，不预留整行。
        UiTransform::from_translation(Val2::percent(-50, 0)),
        GlobalZIndex(1000), BackgroundColor(Color::srgba(0.055, 0.065, 0.08, 0.82)),
        BorderColor::all(Color::srgba(0.40, 0.46, 0.53, 0.62)),
        Name::new("工程预览悬浮HUD")))
        .with_children(|toolbar| {
            // 六点抓手提供明确的移动入口；其绝对位置由hud模块维护，按钮点击不参加拖拽。
            toolbar.spawn((Interaction::None, FocusPolicy::Block, ToolbarDragHandle, ToolbarPointer,
                Node { width: px(16), height: px(26), flex_shrink: 0.,
                    flex_direction: FlexDirection::Column, row_gap: px(3),
                    align_items: AlignItems::Center, justify_content: JustifyContent::Center,
                    border_radius: BorderRadius::all(px(4)), ..default() },
                BackgroundColor(Color::NONE), Name::new("拖动预览HUD")))
                .observe(super::hud::drag_start)
                .observe(super::hud::drag)
                .observe(super::hud::drag_end)
                .with_children(|grip| {
                    for _ in 0..3 {
                        grip.spawn((Node { column_gap: px(3), ..default() }, Pickable::IGNORE))
                            .with_children(|row| {
                                for _ in 0..2 {
                                    row.spawn((Node { width: px(2), height: px(2),
                                        border_radius: BorderRadius::all(px(1)), ..default() },
                                        BackgroundColor(Color::srgb(0.59, 0.65, 0.72)), Pickable::IGNORE));
                                }
                            });
                    }
                });
            for (action, label) in [(ToolbarAction::Return, "返回"),
                (ToolbarAction::Grid, "显示网格"), (ToolbarAction::Collision, "显示碰撞网格")] {
                toolbar.spawn((Interaction::None, FocusPolicy::Block, action, ToolbarPointer,
                    Node { height: px(26), flex_shrink: 0.,
                        padding: UiRect::axes(px(9), px(0)), border: UiRect::all(px(1)),
                        align_items: AlignItems::Center, justify_content: JustifyContent::Center,
                        border_radius: BorderRadius::all(px(4)), ..default() },
                    BackgroundColor(NORMAL), BorderColor::all(Color::srgba(0.30, 0.36, 0.43, 0.42))))
                    .with_children(|button| {
                        button.spawn((Text::new(label), TextFont { font_size: px(12).into(), ..default() },
                            TextColor(Color::srgb(0.94, 0.95, 0.96)), Pickable::IGNORE));
                    });
            }
        });
}
