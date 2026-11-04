//! 调用侧显式提供布局、颜色和阴影的通用按钮实体树。

use crate::{ButtonShellStyle, TextButtonStyle};
use bevy::prelude::*;
use bevy::ui::FocusPolicy;

pub fn spawn_custom_text_button<C: Component>(
    // 生成自定义尺寸/颜色的文本按钮，适合业务 UI 的紧凑按钮、筛选标签、弹窗按钮。
    parent: &mut ChildSpawnerCommands, // 父 UI 节点的 children spawner。
    marker: C,                         // 业务 marker，由调用侧决定。
    font: Handle<Font>,                // 文本字体句柄。
    label: impl Into<String>,          // 按钮文本。
    style: TextButtonStyle,
    shadow: TextShadow, // 上层注入主题，底层不依赖游戏色板。            // 自定义按钮样式。
) {
    let label = label.into(); // 提前转成 String，方便移动到文本子实体中。
    let normal = style.colors.normal; // 初始背景色。
    if let Some(border_color) = style.border_color {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                BorderColor::all(border_color),
                style.colors,
            ))
            .with_children(|btn| {
                btn.spawn((
                    Text::new(label),
                    FocusPolicy::Pass,
                    TextFont {
                        font: font.into(),
                        font_size: style.font_size.into(),
                        ..default()
                    },
                    TextColor(style.text_color),
                    shadow,
                ));
            });
    } else {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                style.colors,
            ))
            .with_children(|btn| {
                btn.spawn((
                    Text::new(label),
                    FocusPolicy::Pass,
                    TextFont {
                        font: font.into(),
                        font_size: style.font_size.into(),
                        ..default()
                    },
                    TextColor(style.text_color),
                    shadow,
                ));
            });
    }
}

pub fn spawn_custom_button<B: Bundle>(
    parent: &mut ChildSpawnerCommands,
    marker: B,
    style: ButtonShellStyle,
    children: impl FnOnce(&mut ChildSpawnerCommands),
) {
    let normal = style.colors.normal;
    if let Some(border_color) = style.border_color {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                BorderColor::all(border_color),
                style.colors,
            ))
            .with_children(children);
    } else {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                style.colors,
            ))
            .with_children(children);
    }
}

pub fn spawn_custom_text_button_bundle<B: Bundle>(
    // 生成自定义文本按钮，marker 可以是单个 Component，也可以是多个 Component 组成的 Bundle。
    parent: &mut ChildSpawnerCommands, // 父 UI 节点的 children spawner。
    marker: B,                         // 业务 marker bundle。
    font: Handle<Font>,                // 文本字体句柄。
    label: impl Into<String>,          // 按钮文本。
    style: TextButtonStyle,
    shadow: TextShadow, // 上层注入主题，底层不依赖游戏色板。            // 自定义按钮样式。
) {
    let label = label.into();
    let normal = style.colors.normal;
    if let Some(border_color) = style.border_color {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                BorderColor::all(border_color),
                style.colors,
            ))
            .with_children(|btn| {
                btn.spawn((
                    Text::new(label),
                    FocusPolicy::Pass,
                    TextFont {
                        font: font.into(),
                        font_size: style.font_size.into(),
                        ..default()
                    },
                    TextColor(style.text_color),
                    shadow,
                ));
            });
    } else {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                style.colors,
            ))
            .with_children(|btn| {
                btn.spawn((
                    Text::new(label),
                    FocusPolicy::Pass,
                    TextFont {
                        font: font.into(),
                        font_size: style.font_size.into(),
                        ..default()
                    },
                    TextColor(style.text_color),
                    shadow,
                ));
            });
    }
}

pub fn spawn_custom_text_button_with_text_marker<C: Component, T: Component>(
    // 生成自定义文本按钮，并给内部文字实体挂 marker，供业务系统更新按钮文案。
    parent: &mut ChildSpawnerCommands, // 父 UI 节点的 children spawner。
    marker: C,                         // 业务按钮 marker。
    text_marker: T,                    // 业务文字 marker。
    font: Handle<Font>,                // 文本字体句柄。
    label: impl Into<String>,          // 按钮文本。
    style: TextButtonStyle,
    shadow: TextShadow, // 上层注入主题，底层不依赖游戏色板。            // 自定义按钮样式。
) {
    let label = label.into(); // 提前转成 String，方便移动到文本子实体中。
    let normal = style.colors.normal; // 初始背景色。
    if let Some(border_color) = style.border_color {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                BorderColor::all(border_color),
                style.colors,
            ))
            .with_children(|btn| {
                btn.spawn((
                    text_marker,
                    Text::new(label),
                    FocusPolicy::Pass,
                    TextFont {
                        font: font.into(),
                        font_size: style.font_size.into(),
                        ..default()
                    },
                    TextColor(style.text_color),
                    shadow,
                ));
            });
    } else {
        parent
            .spawn((
                marker,
                Button,
                Interaction::default(),
                style.node,
                BackgroundColor(normal),
                style.colors,
            ))
            .with_children(|btn| {
                btn.spawn((
                    text_marker,
                    Text::new(label),
                    FocusPolicy::Pass,
                    TextFont {
                        font: font.into(),
                        font_size: style.font_size.into(),
                        ..default()
                    },
                    TextColor(style.text_color),
                    shadow,
                ));
            });
    }
}
