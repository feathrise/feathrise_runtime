//! 专业数值属性框的自动进度填充与右拖灵敏度反馈同步。
//!
//! 【功能设计】填充只是当前值在正式拖动范围中的低对比度反馈，不提供绝对点击跳值；
//! 线性字段使用线性比例，指数拖动字段使用对数比例，使视觉行程与实际手感一致。

use bevy::prelude::*; // 查询公共数值模型并原地修改固定填充节点宽度。
use bevy::ui::{ComputedUiRenderTargetInfo, RelativeCursorPosition};

use crate::components::{
    NumericInputConfig,
    NumericInputDisabled,
    NumericInputProgressFill,
    NumericInputRoot,
    NumericInputScrubConfig,
    NumericInputScrubSensitivityText,
    NumericInputScrubValueText,
    NumericInputValue, // 填充只依赖公共根已有的正式模型，不引入业务组件。
};
use crate::helpers::normalize_numeric_value; // 外部赋值和首帧都共享正式边界语义。
use crate::resources::ActiveNumericInputScrub;
use crate::types::{NumericInputScrubMapping, NumericValue}; // 支持U32、I32和F32三种显示值。

/// 右拖在独立提示中显示当前真实倍率，业务值和单位保持可见，不改输入模型或提交消息。
pub(crate) fn sync_numeric_scrub_sensitivity_feedback(
    active: Res<ActiveNumericInputScrub>,
    roots: Query<
        (&NumericInputScrubConfig, &RelativeCursorPosition, &ComputedNode, &InheritedVisibility),
        (With<NumericInputRoot>, Without<NumericInputDisabled>),
    >,
    mut feedback: Query<
        (&NumericInputScrubSensitivityText, &mut Text, &mut Node),
        Without<NumericInputScrubValueText>,
    >,
    mut previous: Local<Option<(Entity, u32)>>,
) {
    let current = active
        .entity
        .filter(|_| {
            active.dragged
                && active.button == Some(crate::types::NumericInputScrubButton::Sensitivity)
        })
        .and_then(|root| {
            roots
                .get(root)
                .ok()
                .filter(|(_, cursor, node, visible)| {
                    cursor.normalized.is_some() && node.size().min_element() > 0.0 && visible.get()
                })
                .map(|(config, ..)| (root, config.sensitivity.to_bits()))
        });
    if *previous == current {
        return; // 静止和同倍率帧不格式化、复制文字或反复标脏布局。
    }
    for (part, mut text, mut node) in &mut feedback {
        let display = if let Some((_, sensitivity)) = current.filter(|(root, _)| part.root == *root)
        {
            let next = format!("灵敏度 {:.2}×", f32::from_bits(sensitivity)); // 0.01～100倍率与业务单位明确区分。
            if text.0 != next {
                text.0 = next;
            }
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    *previous = current;
}

/// 优先放在鼠标右下方，接近视口边缘时换侧；坐标均为已换算的逻辑像素。
fn sensitivity_feedback_position(cursor: Vec2, size: Vec2, viewport: Vec2) -> Vec2 {
    let gap = Vec2::new(12.0, 16.0);
    let margin = Vec2::splat(4.0);
    let mut position = cursor + gap;
    if position.x + size.x > viewport.x - margin.x {
        position.x = cursor.x - gap.x - size.x;
    }
    if position.y + size.y > viewport.y - margin.y {
        position.y = cursor.y - gap.y - size.y;
    }
    position.clamp(margin, (viewport - size - margin).max(margin))
}

/// 布局完成后只平移当前提示，使用真实UI缩放和目标视口；拖出原框后仍跟随鼠标。
pub(crate) fn position_numeric_scrub_sensitivity_feedback(
    active: Res<ActiveNumericInputScrub>,
    roots: Query<
        (&ComputedNode, &UiGlobalTransform, &RelativeCursorPosition, &InheritedVisibility),
        (With<NumericInputRoot>, Without<NumericInputScrubSensitivityText>, Without<NumericInputDisabled>),
    >,
    mut feedback: Query<
        (&NumericInputScrubSensitivityText, &mut Node, &ComputedNode, &ComputedUiRenderTargetInfo,
            &mut UiTransform, &mut UiGlobalTransform),
        Without<NumericInputRoot>,
    >,
) {
    let Some(root) = active.entity else { return; };
    for (part, mut node, computed, target, mut local, mut global) in &mut feedback {
        if part.root != root || node.display == Display::None { continue; }
        let Some((field, field_global, cursor, _)) = roots.get(root).ok()
            .filter(|(field, _, _, visible)| visible.get() && field.size().min_element() > 0.0)
            .filter(|(_, _, cursor, _)| cursor.normalized.is_some())
        else {
            node.display = Display::None; // 隐藏字段或光标离窗后，OverrideClip也不能留下幽灵提示。
            continue;
        };
        let inverse = computed.inverse_scale_factor();
        let viewport = target.physical_size().as_vec2();
        if computed.size().min_element() <= 0.0 || viewport.min_element() <= 0.0 {
            continue; // 精简无布局宿主不定位；正式布局在首个显示帧提供真实尺寸。
        }
        let cursor = field_global.affine().transform_point2(cursor.normalized.unwrap() * field.size());
        let position = sensitivity_feedback_position(cursor * inverse, computed.size() * inverse, viewport * inverse);
        let wanted = position / inverse + computed.size() * 0.5;
        let delta = wanted - global.translation;
        if delta.length_squared() > 0.0001 {
            let previous = match (local.translation.x, local.translation.y) {
                (Val::Px(x), Val::Px(y)) => Vec2::new(x, y),
                _ => Vec2::ZERO,
            };
            local.translation = Val2::px(previous.x + delta.x * inverse, previous.y + delta.y * inverse);
            // 同帧更新最终绘制位置，不能等下一次布局才让提示追上鼠标。
            let mut affine = global.affine();
            affine.translation += delta;
            *global = affine.into();
        }
    }
}

/// 把任意公共数值变体转换为进度计算所需的有限精度标量。
fn scalar(value: NumericValue) -> f64 {
    match value {
        NumericValue::U32(value) => f64::from(value), // u32可被f64精确表示。
        NumericValue::I32(value) => f64::from(value), // i32可被f64精确表示且保留负号。
        NumericValue::F32(value) => f64::from(value), // 进度计算使用更宽中间精度。
    }
}

/// 返回当前正式数值在拖动业务范围中的零到一比例。
fn progress_ratio(
    value: NumericValue,            // 根实体当前公共模型值。
    input: NumericInputConfig,      // 类型、小数位和周期边界规格。
    scrub: NumericInputScrubConfig, // 线性或指数映射与显示范围。
) -> f32 {
    let value = scalar(normalize_numeric_value(value, input.0)); // 先确保外部值服从正式规格。
    let first = f64::from(scrub.minimum); // 把拖动下界提升到稳定中间精度。
    let second = f64::from(scrub.maximum); // 把拖动上界提升到稳定中间精度。
    let minimum = first.min(second); // 反向损坏配置不会产生负范围。
    let maximum = first.max(second); // 与最小值形成有序区间。
    if !value.is_finite() || !minimum.is_finite() || !maximum.is_finite() || maximum <= minimum {
        return 0.0; // 非有限值和零长度范围安全显示空填充。
    }
    let ratio = match scrub.mapping {
        NumericInputScrubMapping::Linear { .. } => {
            (value - minimum) / (maximum - minimum) // 线性手感直接使用业务区间比例。
        }
        NumericInputScrubMapping::Exponential { .. } if minimum > 0.0 && value > 0.0 => {
            (value / minimum).ln() / (maximum / minimum).ln() // 正数跨数量级字段使用对数位置。
        }
        NumericInputScrubMapping::Exponential { .. } => {
            (value - minimum) / (maximum - minimum) // 非正指数配置安全退化为线性显示。
        }
    };
    if ratio.is_finite() {
        ratio.clamp(0.0, 1.0) as f32 // 合法结果限制在CSS百分比范围内。
    } else {
        0.0 // 对数退化或意外非数不污染Bevy布局。
    }
}

/// 自动同步所有专业属性框填充；实体数量很小且不创建、不重排任何UI节点。
pub fn sync_numeric_scrub_field_fills(
    roots: Query<
        (
            Ref<NumericInputValue>,
            Ref<NumericInputConfig>,
            Ref<NumericInputScrubConfig>,
        ), // Ref让静止帧跳过没有变化的固定字段。
        With<NumericInputRoot>, // 只读取完整公共数值根。
    >,
    mut fills: Query<(Ref<NumericInputProgressFill>, &mut Node)>, // 固定子节点只改一个宽度属性。
) {
    for (fill, mut node) in &mut fills {
        let Ok((value, input, scrub)) = roots.get(fill.root) else {
            continue; // UI换代后一帧内的悬空关联不产生任何副作用。
        };
        if !fill.is_added() && !value.is_changed() && !input.is_changed() && !scrub.is_changed() {
            continue; // 稳定帧不重复计算比例，也不触碰布局组件。
        }
        let width = Val::Percent(progress_ratio(value.0, *input, *scrub) * 100.0); // 正式比例转换为Bevy百分比。
        if node.width != width {
            node.width = width; // 仅在档位变化时触发布局变更，静止帧保持干净。
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 纯比例测试无需启动窗口或UI布局调度。
    use crate::types::{NumericInputSpec, NumericValue}; // 构造三种正式数值规格。

    #[test]
    fn sensitivity_feedback_follows_pointer_and_flips_inside_viewport_edges() {
        let size = Vec2::new(146.0, 30.0);
        let viewport = Vec2::new(900.0, 300.0);
        assert_eq!(sensitivity_feedback_position(Vec2::new(100.0, 80.0), size, viewport), Vec2::new(112.0, 96.0));
        let cursor = Vec2::new(898.0, 298.0);
        let edge = sensitivity_feedback_position(cursor, size, viewport);
        assert!(edge.x + size.x < cursor.x && edge.y + size.y < cursor.y);
        assert!(edge.cmpge(Vec2::splat(4.0)).all() && (edge + size).cmple(viewport - Vec2::splat(4.0)).all());
    }

    #[test]
    fn linear_progress_supports_signed_integer_values() {
        let input = NumericInputConfig(
            NumericInputSpec::i32()
                .with_min(NumericValue::I32(-10)) // 有符号范围起点。
                .with_max(NumericValue::I32(10)), // 有符号范围终点。
        );
        let scrub = NumericInputScrubConfig::linear(1.0, 1.0, -10.0, 10.0, 1.0); // 一比一线性显示范围。
        assert_eq!(progress_ratio(NumericValue::I32(0), input, scrub), 0.5); // 零点应位于对称范围中央。
    }

    #[test]
    fn exponential_progress_matches_relative_drag_range() {
        let input = NumericInputConfig(
            NumericInputSpec::f32(1)
                .with_min(NumericValue::F32(2.0)) // 半径最小值。
                .with_max(NumericValue::F32(512.0)), // 半径最大值。
        );
        let scrub = NumericInputScrubConfig::exponential(
            std::f32::consts::LN_2 / 48.0, // 四十八像素翻倍。
            0.5,                           // 半米吸附。
            2.0,                           // 对数范围起点。
            512.0,                         // 对数范围终点。
            1.0,                           // 标准一倍灵敏度。
        );
        assert_eq!(progress_ratio(NumericValue::F32(32.0), input, scrub), 0.5); // 2到512的几何中点为32。
    }
}
