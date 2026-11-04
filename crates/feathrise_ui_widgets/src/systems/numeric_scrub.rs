//! 公共数字输入的相对拖动扩展：左键改变业务值，右键改变字段独立灵敏度。
//!
//! 【功能设计】左键只负责实际值拖动，右键只负责字段灵敏度，中键进入精确输入；
//! 一旦左键越过阈值，松开只提交一次结果。右拖也只在松开时通知持久化，避免逐帧写设置文件。

use super::*; // 复用数字输入根查询、格式化、提交消息与显示文本刷新入口。
use bevy::ui::RelativeCursorPosition; // 字段归一化坐标换算为物理像素拖动距离。

use crate::types::{NumericInputBoundaryMode, NumericInputSpec, NumericValue, NumericValueKind}; // 左拖映射复用三种公共数值和钳制或循环规格。

mod ordered_pointer;
pub(super) use ordered_pointer::OrderedPointer;

/// 本帧拖动仲裁结果；数字输入主循环据此跳过拖动帧或抑制松开后的误单击。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct NumericInputScrubFrame {
    pub(super) consume_frame: bool, // 指针仍被捕获时禁止普通数字输入继续消费同帧事件。
    pub(super) suppressed_click_root: Option<Entity>, // 左拖结束的根不能再进入精确输入。
    pub(super) precise_edit_root: Option<Entity>, // 中键命中的专业字段在本帧进入精确输入。
}

/// 把内部两种持续拖动职责转换为Bevy真实鼠标按键；中键另由一次性精确输入分支处理。
fn mouse_button(button: crate::types::NumericInputScrubButton) -> MouseButton {
    match button {
        crate::types::NumericInputScrubButton::Value => MouseButton::Left, // 左键调整实际值。
        crate::types::NumericInputScrubButton::Sensitivity => MouseButton::Right, // 右键调整灵敏度。
    }
}

/// 另一枚编辑鼠标键仍在操作或刚发生边沿时拒绝新捕获，避免数值事务与外部作者事务交叉。
fn has_conflicting_mouse_button(
    mouse: &ButtonInput<MouseButton>, // 本帧完整鼠标电平和按下/释放边沿。
    requested: MouseButton,           // 当前准备开始的左拖、右拖或中键精确编辑。
) -> bool {
    [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
        .into_iter()
        .filter(|button| *button != requested)
        .any(|button| {
            mouse.pressed(button) || mouse.just_pressed(button) || mouse.just_released(button)
        }) // 同帧刚结束的旧操作也先获得收口机会，下一静默帧才允许新事务。
}

/// 只接受Bevy UI命中系统选出的真正顶层字段；几何重叠但被遮挡的根保持`Interaction::None`。
fn pointer_targets_field(
    interaction: &Interaction, // 顶层命中由Bevy UI堆叠与FocusPolicy统一裁决。
    cursor: &RelativeCursorPosition, // 仍复核光标位于字段几何范围内。
) -> bool {
    matches!(*interaction, Interaction::Hovered | Interaction::Pressed) && cursor.cursor_over()
}

/// 返回字段当前相对光标的物理像素X；归一化坐标以节点中心为零。
fn cursor_x_pixels(cursor: &RelativeCursorPosition, computed: &ComputedNode) -> Option<f32> {
    let position = cursor.normalized?; // 布局首帧或光标离开窗口时没有可用坐标。
    let width = computed.size().x; // Bevy 0.19 ComputedNode直接提供物理像素宽度。
    (width.is_finite() && width > 0.0).then_some(position.x * width) // 坏布局不产生NaN拖动。
}

/// 钳制右拖产生的倍率；非法旧设置统一回退一倍标准手感。
fn sanitize_sensitivity(value: f32, config: &NumericInputScrubConfig) -> f32 {
    let minimum = config.minimum_sensitivity.max(f32::MIN_POSITIVE); // 灵敏度必须保持有限正数。
    let maximum = config.maximum_sensitivity.max(minimum); // 损坏上下界退化为固定最小倍率。
    if value.is_finite() {
        value.clamp(minimum, maximum) // 合法用户倍率只做受检范围钳制。
    } else {
        1.0_f32.clamp(minimum, maximum) // NaN或无穷值回退标准一倍。
    }
}

/// 右键总位移按八度缩放独立灵敏度，使向左和向右拥有对称倍率行程。
fn sensitivity_from_drag(
    start: f32,                       // 按下右键时冻结的字段倍率。
    delta_pixels: f32,                // 从按下点累计的物理像素位移。
    config: &NumericInputScrubConfig, // 提供翻倍像素距离和安全范围。
) -> f32 {
    let pixels_per_octave = config.sensitivity_pixels_per_octave.max(1.0); // 非法零距离至少按一像素翻倍。
    let raw = sanitize_sensitivity(start, config) * 2.0_f32.powf(delta_pixels / pixels_per_octave); // 向右翻倍、向左减半。
    if raw.is_infinite() && raw.is_sign_positive() {
        sanitize_sensitivity(f32::MAX, config) // 正向极端位移直接饱和受检上限，不能因溢出回退一倍。
    } else if raw.is_nan() {
        sanitize_sensitivity(start, config) // 非法光标位移保持按下时倍率。
    } else {
        sanitize_sensitivity(raw, config) // 有限结果和向左下溢统一限制在固定范围内。
    }
}

/// 按业务步长吸附左拖结果；是否限制端点由统一输入边界语义决定。
fn snap_drag_value(
    value: f64, // 映射产生的连续候选业务值；宽精度保证I32端点仍能逐档拖动。
    config: &NumericInputScrubConfig, // 拖动吸附步长和推荐业务范围。
    boundary: NumericInputBoundaryMode, // 周期角度必须保留越界量供后续循环。
) -> f64 {
    let first = f64::from(config.minimum); // 配置仍保持轻量f32，计算时提升为宽精度。
    let second = f64::from(config.maximum); // I32全范围端点不会因中间加减再次丢失低位。
    let minimum = first.min(second); // 即使调用方误写上下界也保持确定范围。
    let maximum = first.max(second); // 与最小值形成有序闭区间。
    let bounded = if value.is_finite() && boundary == NumericInputBoundaryMode::Wrap {
        value // 周期字段不能在进入正式规范化前丢失跨端点位移。
    } else if value.is_finite() {
        value.clamp(minimum, maximum) // 普通字段先限制到业务范围。
    } else if value.is_sign_negative() {
        minimum // 负无穷回退最小端点。
    } else {
        maximum // NaN或正无穷统一回退最大端点，随后仍经规范化。
    };
    let step = f64::from(config.drag_step); // 每个字段独立声明米、百分比或速度吸附粒度。
    let snapped = if step.is_finite() && step > 0.0 {
        (bounded / step).round() * step // 围绕零点吸附，保证正负整数和角度档位对称。
    } else {
        bounded // 非法步长安全退化为连续拖动，不执行除零。
    };
    if boundary == NumericInputBoundaryMode::Clamp {
        snapped.clamp(minimum, maximum) // 普通字段再次处理吸附产生的端点误差。
    } else {
        snapped // 周期字段把完整越界候选交给NumericInputSpec循环。
    }
}

/// 把宽精度拖动结果转换成正式数值变体；整数周期在收窄前完成，避免类型端点饱和吞掉越界方向。
fn candidate_from_drag(snapped: f64, input: NumericInputSpec) -> NumericValue {
    match input.kind {
        NumericValueKind::U32 => {
            if input.boundary_mode == NumericInputBoundaryMode::Wrap
                && let (Some(minimum), Some(maximum)) = (
                    input.min.and_then(NumericValue::as_u32),
                    input.max.and_then(NumericValue::as_u32),
                )
            {
                let first = f64::from(minimum); // 无符号周期的两个作者端点先分别提升精度。
                let second = f64::from(maximum); // 保留反向配置中的真实较大端点。
                let minimum = first.min(second); // 修正反向配置后定义半开周期。
                let maximum = first.max(second); // 保留有序周期终点。
                let span = maximum - minimum;
                if span.is_finite() && span > 0.0 && snapped.is_finite() {
                    return NumericValue::U32(
                        ((snapped.round() - minimum).rem_euclid(span) + minimum) as u32,
                    ); // 负向越过零点时也从周期尾部继续。
                }
            }
            NumericValue::U32(snapped.round().clamp(0.0, f64::from(u32::MAX)) as u32)
        }
        NumericValueKind::I32 => {
            if input.boundary_mode == NumericInputBoundaryMode::Wrap
                && let (Some(minimum), Some(maximum)) = (
                    input.min.and_then(NumericValue::as_i32),
                    input.max.and_then(NumericValue::as_i32),
                )
            {
                let first = f64::from(minimum); // 有符号周期允许负起点。
                let second = f64::from(maximum); // 上端点与起点一起形成半开区间。
                let minimum = first.min(second);
                let maximum = first.max(second);
                let span = maximum - minimum;
                if span.is_finite() && span > 0.0 && snapped.is_finite() {
                    return NumericValue::I32(
                        ((snapped.round() - minimum).rem_euclid(span) + minimum) as i32,
                    ); // 在收窄到i32前保留完整跨端点位移。
                }
            }
            NumericValue::I32(
                snapped
                    .round()
                    .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
            )
        }
        NumericValueKind::F32 => NumericValue::F32(snapped as f32), // 浮点正式模型仍使用项目统一f32。
    }
}

/// 检查字段及其UI祖先当前确实参与布局和可见性，避免隐藏面板保留旧cursor_over后截获玩法鼠标。
fn ui_path_is_interactable(
    mut entity: Entity, // 从专业字段根开始向上走到UI树根。
    hierarchy: &Query<(
        &Node,
        Option<&Visibility>,
        Option<&InheritedVisibility>,
        Option<&ChildOf>,
    )>,
) -> bool {
    for _ in 0..128 {
        let Ok((node, visibility, inherited, parent)) = hierarchy.get(entity) else {
            return false; // 层级换代或离开UI树时失败关闭命中。
        };
        if node.display == Display::None
            || visibility.is_some_and(|value| *value == Visibility::Hidden)
            || inherited.is_some_and(|value| !value.get())
        {
            return false; // 直接隐藏、父级隐藏和布局隐藏都不能启动三种专业手势。
        }
        let Some(parent) = parent else {
            return true; // 到达仍可见的UI根，整条路径均可交互。
        };
        entity = parent.parent(); // 父层只在按键边沿读取，不增加稳定帧遍历。
    }
    false // 异常循环或超深层级失败关闭，不能让损坏UI吞玩法输入。
}

/// 从按下时数值和总位移计算本帧左拖完成值；U32、I32和F32共享此入口。
fn value_from_drag(
    start: NumericValue,             // 按下左键时冻结的规范化数字输入值。
    delta_pixels: f32,               // 从按下点累计的物理像素位移。
    scrub: &NumericInputScrubConfig, // 字段映射、灵敏度、步长与业务范围。
    input: NumericInputSpec,         // 精确输入的小数位和最终规范化边界。
) -> NumericValue {
    let start_scalar = match start {
        NumericValue::U32(value) => f64::from(value), // 无符号离散量以宽精度进入统一连续计算。
        NumericValue::I32(value) => f64::from(value), // I32全范围仍能保留一单位差异。
        NumericValue::F32(value) => f64::from(value), // 浮点值提升后减少累计映射误差。
    };
    let sensitivity = f64::from(sanitize_sensitivity(scrub.sensitivity, scrub)); // 左拖始终使用当前受检字段倍率。
    let raw = match scrub.mapping {
        crate::types::NumericInputScrubMapping::Linear { units_per_pixel } => {
            start_scalar + f64::from(delta_pixels) * f64::from(units_per_pixel) * sensitivity // 百分比、整数和水位按相对业务单位线性变化。
        }
        crate::types::NumericInputScrubMapping::Exponential { exponent_per_pixel }
            if start_scalar > 0.0 =>
        {
            start_scalar
                * (f64::from(delta_pixels) * f64::from(exponent_per_pixel) * sensitivity).exp() // 正数跨度量按比例缩放。
        }
        crate::types::NumericInputScrubMapping::Exponential { .. } => start_scalar, // 非正数不能安全使用比例映射。
    };
    let snapped = snap_drag_value(raw, scrub, input.boundary_mode); // 吸附不得提前破坏周期越界信息。
    let candidate = candidate_from_drag(snapped, input); // 整数收窄和周期越界都在一个受检入口完成。
    normalize_numeric_value(candidate, input) // 最后统一应用小数位、钳制或循环边界。
}

/// 更新一次左键业务值拖动，并通过现有Changed消息提供实时业务预览。
fn update_value_drag(
    root: Entity,                          // 当前被捕获的数字输入业务根。
    delta_pixels: f32,                     // 从按下位置累计的水平物理像素位移。
    start_value: NumericValue,             // 按下时冻结值，避免逐帧累计误差。
    scrub: NumericInputScrubConfig,        // 本帧读取的映射和独立灵敏度。
    state: &mut NumericInputState<'_, '_>, // 直接更新数字模型、显示文本和Changed出口。
) {
    let Ok((input, mut value, mut edit_state, disabled)) = state.root_query.get_mut(root) else {
        return; // UI换代后旧捕获不再产生业务副作用。
    };
    if disabled {
        return; // 保存或切图期间禁用的字段保持当前值。
    }
    let next = value_from_drag(start_value, delta_pixels, &scrub, input.0); // 总位移一次得到确定结果。
    if value.0 == next {
        return; // 同一吸附档不重复传播Changed或重建业务预览。
    }
    value.0 = next; // 数字输入模型立即反映当前拖动结果。
    edit_state.editing = false; // 拖动与文本编辑互斥。
    edit_state.buffer.clear(); // 旧文本缓冲不得在拖动后重新出现。
    refresh_numeric_input_text(
        root,                                 // 只刷新当前业务根绑定的显示文本。
        &format_numeric_value(next, input.0), // 拖动继续使用同一小数位显示口径。
        &mut state.text_query,                // 不创建或替换UI实体。
    );
    state.changed_writer.write(NumericInputChanged {
        entity: root, // 业务层按稳定根身份区分四个字段。
        value: next,  // 当前帧完整规范化结果用于实时预览。
    });
}

/// 左拖结束后只发送一次Committed；拖动途中已经由Changed完成实时业务预览。
fn finish_value_drag(
    root: Entity,                          // 当前拖动数字输入根。
    start_value: NumericValue,             // 用于判断整次拖动是否产生净变化。
    state: &mut NumericInputState<'_, '_>, // 收口模型、文本和业务提交消息。
) {
    let Ok((input, value, mut edit_state, _)) = state.root_query.get_mut(root) else {
        return; // 根已销毁时不向业务发布悬空提交。
    };
    let final_value = value.0; // 松开时模型已经保存最后一次吸附值。
    edit_state.previous_value = final_value; // 下一次精确输入以本次拖动结果为恢复基线。
    edit_state.editing = false; // 松开后不自动进入文本模式。
    edit_state.buffer.clear(); // 拖动没有待提交字符缓冲。
    edit_state.replace_on_next_input = input.0.replace_on_first_key; // 下一次单击继续服从首键替换规格。
    edit_state.edited = false; // 文本编辑身份在拖动收口后归零。
    refresh_numeric_input_text(
        root,                                        // 只恢复当前字段的正式数值显示。
        &format_numeric_value(final_value, input.0), // 右侧单位仍由业务UI独立显示。
        &mut state.text_query,                       // 原地改写固定文本实体。
    );
    state.committed_writer.write(NumericInputCommitted {
        entity: root,                                 // 业务层用根身份结束对应拖动事务。
        value: final_value,                           // 松开时最终采用值。
        outcome: NumericInputCommitOutcome::Accepted, // 拖动始终产生可解析规范化数值。
        source: NumericInputCommitSource::Scrub,      // 业务层可把全部Changed合并为一次撤销。
        changed: final_value != start_value,          // 回到按下值时不生成净作者操作。
        edited: true,                                 // 超过阈值代表明确的拖动编辑意图。
    });
}

/// 处理可选拖动字段并返回普通数字输入本帧应采取的单击策略。
pub(super) fn process_numeric_input_scrub(
    interactions: &mut NumericInputInteractions<'_, '_>, // 字段命中、鼠标、配置与唯一捕获。
    state: &mut NumericInputState<'_, '_>,               // 数字模型、文本焦点和业务消息出口。
) -> NumericInputScrubFrame {
    if let Some(button) = interactions.active_scrub.canceled_button {
        interactions.ordered_pointer.discard();
        if !interactions.mouse.pressed(mouse_button(button))
            && !interactions.mouse.just_released(mouse_button(button))
        {
            interactions.active_scrub.canceled_button = None;
        }
        return NumericInputScrubFrame { consume_frame: true, ..default() };
    }
    let ordered = interactions.ordered_pointer.poll();
    if interactions.active_scrub.entity.is_none() {
        if interactions.mouse.just_pressed(MouseButton::Middle)
            && !has_conflicting_mouse_button(&interactions.mouse, MouseButton::Middle)
        {
            let precise_edit_root =
                interactions
                    .scrub_fields
                    .iter()
                    .find_map(|(field, interaction, cursor, _)| {
                        (pointer_targets_field(interaction, cursor)
                            && ui_path_is_interactable(field.root, &interactions.ui_hierarchy)
                            && interactions.scrub_configs.get(field.root).is_ok())
                        .then_some(field.root) // 中键只激活真正命中的专业输入根。
                    });
            if precise_edit_root.is_some() {
                return NumericInputScrubFrame {
                    consume_frame: false,        // 中键不开始持续拖动捕获，普通主循环负责切换焦点。
                    suppressed_click_root: None, // 本帧没有左键松开需要抑制。
                    precise_edit_root,           // 主循环会以公共方式提交旧字段并聚焦新字段。
                };
            }
        }
        let pressed_button = if let Some((_, button, _)) = ordered.start {
            if has_conflicting_mouse_button(&interactions.mouse, mouse_button(button)) {
                interactions.ordered_pointer.discard();
                return NumericInputScrubFrame::default();
            }
            Some(button)
        } else if ordered.handled {
            None // 官方消息已证明按下不属于字段，不能用帧末光标补捕获另一字段。
        } else if interactions.mouse.just_pressed(MouseButton::Left)
            && !has_conflicting_mouse_button(&interactions.mouse, MouseButton::Left)
        {
            Some(crate::types::NumericInputScrubButton::Value) // 左键优先捕获业务值。
        } else if interactions.mouse.just_pressed(MouseButton::Right)
            && !has_conflicting_mouse_button(&interactions.mouse, MouseButton::Right)
        {
            Some(crate::types::NumericInputScrubButton::Sensitivity) // 右键只捕获字段倍率。
        } else {
            None // 中键和无新按键帧不开始数字拖动。
        };
        let Some(button) = pressed_button else {
            return NumericInputScrubFrame::default(); // 没有新捕获时继续普通数字输入。
        };
        let pressed = ordered.start.map(|(root, _, x)| (root, x)).or_else(||
            interactions
                .scrub_fields
                .iter()
                .find_map(|(field, interaction, cursor, computed)| {
                    (pointer_targets_field(interaction, cursor)
                        && ui_path_is_interactable(field.root, &interactions.ui_hierarchy)
                        && interactions.scrub_configs.get(field.root).is_ok())
                    .then(|| cursor_x_pixels(cursor, computed).map(|x| (field.root, x)))
                    .flatten() // 只接受真正命中且布局宽度有效的可拖动根。
                }));
        let Some((root, start_cursor_x)) = pressed else {
            return NumericInputScrubFrame::default(); // 指针不在可拖动数字框时保留原按钮行为。
        };
        if !ui_path_is_interactable(root, &interactions.ui_hierarchy) {
            interactions.ordered_pointer.discard();
            return NumericInputScrubFrame::default();
        }
        if let Some(active_root) = state.active.entity {
            commit_numeric_input(active_root, false, false, state); // 新拖动先提交旧精确输入，防止两个模式并存。
        }
        let Ok((_, value, _, disabled)) = state.root_query.get_mut(root) else {
            interactions.ordered_pointer.discard();
            return NumericInputScrubFrame::default(); // 字段与业务根不同步时拒绝捕获。
        };
        if disabled {
            interactions.ordered_pointer.discard();
            return NumericInputScrubFrame::default(); // 禁用字段不开始左右键拖动。
        }
        let start_value = value.0; // 根查询借用结束前复制轻量规范化值。
        let Ok(mut config) = interactions.scrub_configs.get_mut(root) else {
            interactions.ordered_pointer.discard();
            return NumericInputScrubFrame::default(); // 关系换代时安全拒绝捕获。
        };
        let safe_sensitivity = sanitize_sensitivity(config.sensitivity, &config); // 先结束只读借用再写回配置。
        config.sensitivity = safe_sensitivity; // 捕获前修复非法持久化倍率。
        interactions.active_scrub.entity = Some(root); // 发布唯一拖动根供业务快捷键门读取。
        interactions.active_scrub.button = Some(button); // 冻结本次左右键职责。
        interactions.active_scrub.start_cursor_x = start_cursor_x; // 总位移始终相对按下物理位置。
        interactions.active_scrub.start_value = Some(start_value); // 左拖不逐帧累计误差。
        interactions.active_scrub.start_sensitivity = config.sensitivity; // 右拖倍率同样从按下快照计算。
        interactions.active_scrub.dragged = false; // 尚未越过四像素时保持短按无动作，不发布数值变化。
        if !ordered.owned { return NumericInputScrubFrame {
            consume_frame: true,         // 按下帧由拖动捕获拥有，键盘和普通单击等待松开仲裁。
            suppressed_click_root: None, // 是否抑制左键单击要等累计位移超过阈值。
            precise_edit_root: None,     // 左右拖动不会同时进入字符编辑。
        }; }
    }

    let root = interactions.active_scrub.entity.expect("已检查活动拖动根"); // 唯一捕获在本分支必然存在。
    let button = interactions
        .active_scrub
        .button
        .expect("活动拖动必须保存左右键职责"); // 捕获构造保证按键身份完整。
    if interactions.keyboard.just_pressed(KeyCode::Escape) || ordered.canceled
        || (ordered.owned && !ordered.released && !interactions.mouse.pressed(mouse_button(button)))
    {
        cancel_drag(root, button, interactions, state);
        return NumericInputScrubFrame { consume_frame: true, suppressed_click_root: Some(root), ..default() };
    }
    let position = if ordered.owned { ordered.position } else { interactions
        .scrub_fields
        .iter()
        .find(|(field, _, _, _)| field.root == root)
        .and_then(|(_, _, cursor, computed)| cursor_x_pixels(cursor, computed)) }; // 有序输入使用本次事件坐标，旧CPU夹具保留原相对坐标路径。
    let config = interactions.scrub_configs.get(root).ok().copied(); // 当前字段配置很小，复制后释放查询借用。
    if let (Some(current_x), Some(config)) = (position, config) {
        let delta_pixels = current_x - interactions.active_scrub.start_cursor_x; // 只使用总位移，帧率不影响结果。
        if delta_pixels.abs().max(ordered.excursion) >= config.drag_threshold_pixels.max(0.0) {
            interactions.active_scrub.dragged = true; // 越过阈值后回到原点也仍按一次拖动收口。
        }
        if interactions.active_scrub.dragged {
            match button {
                crate::types::NumericInputScrubButton::Value => {
                    if let Some(start_value) = interactions.active_scrub.start_value {
                        update_value_drag(root, delta_pixels, start_value, config, state); // 左拖实时更新实际业务值。
                    }
                }
                crate::types::NumericInputScrubButton::Sensitivity => {
                    let before = interactions.active_scrub.start_sensitivity; // 整次右拖只从按下倍率计算。
                    let after = sensitivity_from_drag(before, delta_pixels, &config); // 指数倍率允许精细与快速双向调整。
                    let mut changed = false;
                    if let Ok(mut live_config) = interactions.scrub_configs.get_mut(root) {
                        if live_config.sensitivity != after {
                            live_config.sensitivity = after; // 拖动中立即改变后续左拖手感，但暂不写磁盘。
                            changed = true;
                        }
                    }
                    if changed {
                        interactions.sensitivity_changed.write(
                            NumericInputScrubSensitivityChanged {
                                entity: root, // 业务层按字段显示本帧倍率。
                                value: after, // 只用于即时状态栏，不触发GameSettings变更。
                            },
                        );
                    }
                }
            }
        }
    }

    if if ordered.owned { !ordered.released } else { interactions.mouse.pressed(mouse_button(button)) } {
        return NumericInputScrubFrame {
            consume_frame: true,         // 对应按键仍按住时继续独占数字输入指针。
            suppressed_click_root: None, // 松开前不需要普通输入判断单击。
            precise_edit_root: None,     // 捕获期间中键不会切换到精确输入。
        };
    }

    let dragged = interactions.active_scrub.dragged; // 清空捕获前冻结最终阈值结论。
    let start_value = interactions.active_scrub.start_value; // 左拖提交需要按下值判断净变化。
    let start_sensitivity = interactions.active_scrub.start_sensitivity; // 右拖提交需要一次前后值。
    match button {
        crate::types::NumericInputScrubButton::Value if dragged => {
            if let Some(start_value) = start_value {
                finish_value_drag(root, start_value, state); // 松开只形成一条业务提交。
            }
        }
        crate::types::NumericInputScrubButton::Sensitivity if dragged => {
            if let Ok(config) = interactions.scrub_configs.get(root) {
                let after = sanitize_sensitivity(config.sensitivity, &config); // 最终倍率再次通过安全范围。
                interactions
                    .sensitivity_committed
                    .write(NumericInputScrubSensitivityCommitted {
                        entity: root,              // 业务层据根身份选择独立持久化字段。
                        before: start_sensitivity, // 右拖开始倍率用于诊断和测试。
                        after, // 即使拖回起点也通知状态收口；业务同值时不会写settings资源。
                    });
            }
        }
        _ => {} // 未超过阈值的左键和右键单击都保持无动作。
    }
    *interactions.active_scrub = ActiveNumericInputScrub::default(); // 松开或窗口失焦后释放唯一捕获。
    NumericInputScrubFrame {
        consume_frame: false, // 松开帧允许普通输入处理真正的左键单击。
        suppressed_click_root: (button == crate::types::NumericInputScrubButton::Value)
            .then_some(root), // 专业字段左键只负责实际值，短按也不能误入精确输入。
        precise_edit_root: None, // 左右键松开不会冒充中键精确输入。
    }
}

/// Esc与异常指针收口只发一次Canceled；原业务消费者负责恢复草稿、dirty与历史。
fn cancel_drag(root: Entity, button: crate::types::NumericInputScrubButton,
    interactions: &mut NumericInputInteractions, state: &mut NumericInputState) {
    match button {
        crate::types::NumericInputScrubButton::Value => {
            if let Some(before) = interactions.active_scrub.start_value
                && let Ok((config, mut value, mut edit, _)) = state.root_query.get_mut(root)
            {
                let changed = value.0 != before;
                value.0 = before;
                edit.previous_value = before;
                edit.editing = false;
                edit.buffer.clear();
                edit.edited = false;
                edit.replace_on_next_input = config.0.replace_on_first_key;
                refresh_numeric_input_text(root, &format_numeric_value(before, config.0), &mut state.text_query);
                state.committed_writer.write(NumericInputCommitted { entity: root, value: before,
                    outcome: NumericInputCommitOutcome::Canceled, source: NumericInputCommitSource::Scrub,
                    changed, edited: interactions.active_scrub.dragged });
            }
        }
        crate::types::NumericInputScrubButton::Sensitivity => {
            if let Ok(mut config) = interactions.scrub_configs.get_mut(root) {
                config.sensitivity = interactions.active_scrub.start_sensitivity;
            } // 右拖取消恢复原倍率，不发布持久化消息。
        }
    }
    *interactions.active_scrub = ActiveNumericInputScrub { canceled_button: Some(button), ..default() };
    interactions.ordered_pointer.discard();
}

#[cfg(test)]
mod tests; // 纯数值映射回归独立存放，主系统文件保持在模块行数上限内。
