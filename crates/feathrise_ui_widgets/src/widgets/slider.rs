//! 通用滑块结构，布局与色板由调用侧提供。

use crate::UiInteraction;
use crate::{SliderPalette, SliderTick, SliderVisual};
use bevy::prelude::*;
use bevy::ui::{FocusPolicy, RelativeCursorPosition};

pub fn spawn_slider_with_palette<T: Component, H: Component>(
    // 生成一个可自定义视觉参数的滑块轨道。
    parent: &mut ChildSpawnerCommands, // 父 UI 节点的 children spawner。
    track_marker: T,                   // 轨道业务 marker。
    thumb_marker: H,                   // 手柄业务 marker。
    font: Handle<Font>,                // 刻度文字字体。
    thumb_left: f32,                   // 手柄初始 left 偏移。
    ticks: &[SliderTick],              // 刻度列表。
    palette: SliderPalette,            // 上层提供轨道、边框、刻度和手柄主题。
    visual: SliderVisual,              // 滑块视觉参数。
) {
    parent // 在父节点下生成轨道实体。
        .spawn((
            // 轨道实体 bundle。
            track_marker,                      // 挂载业务 marker。
            UiInteraction::default(),            // 初始化交互状态，业务拖拽系统用它判断是否按下。
            RelativeCursorPosition::default(), // 记录相对游标位置，兼容当前滑块拖拽系统。
            Node {
                // 轨道布局节点。
                width: Val::Px(visual.track_width), // 设置轨道宽度。
                height: Val::Px(visual.track_height), // 设置轨道高度。
                margin: UiRect::bottom(Val::Px(6.0)), // 保留原设置页滑块底部间距。
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)), // 轨道使用大圆角。
                ..default()                                     // 其它 Node 字段保持默认。
            },
            BackgroundColor(palette.track), // 使用统一滑块轨道背景色。
            BorderColor::all(palette.border),
        ))
        .with_children(|track| {
            // 在轨道实体下生成刻度和手柄。
            for tick in ticks {
                // 遍历所有刻度配置。
                track // 在轨道下生成单个刻度容器。
                    .spawn(Node {
                        // 刻度容器用于定位和居中文本。
                        position_type: PositionType::Absolute, // 刻度相对轨道绝对定位。
                        left: Val::Px((tick.px - visual.tick_width / 2.0).max(0.0)), // 让刻度文字围绕 tick.px 居中并避免负偏移。
                        top: Val::Px(visual.tick_top), // 刻度文字位于轨道下方。
                        width: Val::Px(visual.tick_width), // 刻度文字容器宽度。
                        justify_content: JustifyContent::Center, // 刻度文字水平居中。
                        ..default()                    // 其它 Node 字段保持默认。
                    })
                    .with_children(|p| {
                        // 在刻度容器下生成文字。
                        p.spawn((
                            // 刻度文字实体。
                            Text::new(tick.label), // 设置刻度显示文本。
                            TextFont {
                                // 配置刻度字体。
                                font: font.clone().into(), // 每个刻度复用同一字体句柄。
                                font_size: visual.tick_font_size.into(), // 使用视觉参数指定的字号。
                                ..default()                // 保留其它 TextFont 默认设置。
                            },
                            TextColor(palette.tick),
                            palette.shadow,
                        ));
                    });
            }
            track.spawn((
                // 生成滑块手柄实体。
                thumb_marker, // 挂载手柄业务 marker。
                Node {
                    // 手柄布局节点。
                    position_type: PositionType::Absolute, // 手柄相对轨道绝对定位。
                    left: Val::Px(thumb_left),             // 手柄初始横向偏移。
                    top: Val::Px(-5.0),                    // 保留原手柄向上探出的视觉效果。
                    width: Val::Px(visual.thumb_width),    // 设置手柄宽度。
                    height: Val::Px(visual.thumb_height),  // 设置手柄高度。
                    border_radius: BorderRadius::all(Val::Px(4.0)), // 手柄使用小圆角。
                    ..default()                            // 其它 Node 字段保持默认。
                },
                BackgroundColor(palette.thumb), // 使用统一滑块手柄背景色。
                FocusPolicy::Pass, // 手柄只负责视觉，鼠标命中继续传给父轨道，保证可以直接抓住手柄拖动。
                Pickable::IGNORE, // 现代拾取后端忽略 FocusPolicy，显式把手柄命中交给轨道。
            ));
        });
}
