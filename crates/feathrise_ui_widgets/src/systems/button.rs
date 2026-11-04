// 通用按钮视觉系统：只负责补齐三态颜色并响应Bevy交互状态。

use bevy::ecs::system::SystemParam; // 把按钮初始化查询封装为稳定系统参数。

use super::*; // 复用父模块导入的通用UI组件、颜色辅助函数与Bevy类型。

/// 尚未显式配置三态颜色的按钮查询数据。
type MissingButtonColorData = (Entity, &'static BackgroundColor);
/// 只匹配按钮且排除已经由业务侧配置ButtonColors的实体。
type MissingButtonColorFilter = (With<Button>, Without<ButtonColors>);

/// 自动补齐按钮颜色所需的单一查询。
#[derive(SystemParam)]
pub struct MissingButtonColors<'w, 's> {
    query: Query<'w, 's, MissingButtonColorData, MissingButtonColorFilter>,
}

/// Auto-add button colors for buttons that do not provide them explicitly.
pub fn ensure_button_colors_system(mut commands: Commands, missing: MissingButtonColors) {
    for (entity, bg) in &missing.query {
        commands.entity(entity).insert(ButtonColors::new(
            bg.0,
            lighten_color(bg.0, 0.12),
            lighten_color(bg.0, 0.25),
        ));
    }
}

/// Drive shared button hover/pressed visuals.
pub fn button_color_system(
    mut query: Query<(&Interaction, &ButtonColors, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, colors, mut bg) in &mut query {
        bg.0 = match interaction {
            Interaction::Pressed => colors.pressed,
            Interaction::Hovered => colors.hovered,
            Interaction::None => colors.normal,
        };
    }
}
