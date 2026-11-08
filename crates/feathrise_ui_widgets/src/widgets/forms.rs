//! 旧式数字字段的通用实体树；所属 UI 库注入主题。

use crate::InteractionButton;
use bevy::prelude::*;
use bevy::ui::{FocusPolicy, RelativeCursorPosition}; // 可选拖动行为按字段内相对位置计算物理像素位移。

use crate::NumericInputPalette;
use crate::components::{NumericInputDisplayText, NumericInputFieldButton};

pub fn spawn_numeric_field_with_palette(
    parent: &mut ChildSpawnerCommands,
    root: Entity,
    font: Handle<Font>,
    value: impl Into<String>,
    width: f32,
    height: f32,
    font_size: f32,
    palette: NumericInputPalette,
    text_color: Color,
    shadow: TextShadow,
) {
    parent
        .spawn((
            NumericInputFieldButton { root },
            palette, // 禁用和焦点刷新也保留自己的主题。
            InteractionButton,
            Interaction::default(),
            RelativeCursorPosition::default(), // 非可拖动字段只多维护一个轻量坐标；带ScrubConfig的根才真正消费拖动。
            Node {
                width: Val::Px(width),
                height: Val::Px(height),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(palette.normal_bg),
            BorderColor::all(palette.normal_border),
        ))
        .with_children(|input| {
            input.spawn((
                NumericInputDisplayText { root },
                Text::new(value.into()),
                FocusPolicy::Pass,
                TextFont {
                    font: font.into(),
                    font_size: font_size.into(),
                    ..default()
                },
                TextColor(text_color),
                shadow,
            ));
        });
}
