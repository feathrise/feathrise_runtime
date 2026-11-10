//! 公共数字拖动映射的纯函数回归，避免启动窗口或布局系统。

use crate::UiInteraction;
use super::*; // 子模块可直接验证生产模块的私有换算入口。

#[path = "ordered_tests.rs"]
mod ordered;

fn linear_config(sensitivity: f32) -> NumericInputScrubConfig {
    NumericInputScrubConfig {
        mapping: crate::types::NumericInputScrubMapping::Linear {
            units_per_pixel: 0.1, // 每像素一分米用于水位手感测试。
        },
        drag_step: 0.001,                    // 水位继续保留毫米显示和作者精度。
        minimum: -10_000.0,                  // 与场景编辑器水位下界一致。
        maximum: 10_000.0,                   // 与场景编辑器水位上界一致。
        sensitivity,                         // 测试显式传入字段独立倍率。
        minimum_sensitivity: 0.01,           // 最细仍允许百倍精调。
        maximum_sensitivity: 100.0,          // 最快仍保持固定倍率上限。
        drag_threshold_pixels: 4.0,          // 生产单击/拖动阈值。
        sensitivity_pixels_per_octave: 48.0, // 四十八像素翻倍。
    }
}

#[test]
fn another_mouse_button_blocks_new_numeric_capture_until_a_quiet_frame() {
    let mut mouse = ButtonInput::<MouseButton>::default(); // 模拟世界作者左键尚未收口。
    mouse.press(MouseButton::Left);
    assert!(has_conflicting_mouse_button(&mouse, MouseButton::Right)); // 不能并发开始右拖灵敏度。
    assert!(!has_conflicting_mouse_button(&mouse, MouseButton::Left)); // 当前请求键本身不构成冲突。

    mouse.clear(); // 推进到下一帧但保持左键按住电平。
    mouse.release(MouseButton::Left); // 本帧释放沿仍属于旧作者事务。
    assert!(has_conflicting_mouse_button(&mouse, MouseButton::Right)); // release帧仍不能抢占指针。

    mouse.clear(); // 再推进一个没有按键边沿的静默帧。
    assert!(!has_conflicting_mouse_button(&mouse, MouseButton::Right)); // 此时才允许开始右拖。
}

#[test]
fn overlapping_geometry_only_accepts_the_ui_hit_test_winner() {
    let cursor = RelativeCursorPosition {
        cursor_over: true,            // Bevy布局命中确认几何上位于两个重叠字段内部。
        normalized: Some(Vec2::ZERO), // 像素位置对本次顶层仲裁并不重要。
        ..default()
    };
    assert!(!pointer_targets_field(&UiInteraction::None, &cursor)); // 被顶层面板遮住的底层根不能截获输入。
    assert!(pointer_targets_field(&UiInteraction::Hovered, &cursor)); // Bevy选中的顶层根才允许候选。
    assert!(pointer_targets_field(&UiInteraction::Pressed, &cursor)); // 左键按下后的顶层根继续属于同一字段。
}

#[test]
fn linear_scrub_uses_total_pixels_and_field_sensitivity() {
    let config = linear_config(0.5); // 半倍灵敏度应把十像素位移缩为半米。
    let value = value_from_drag(
        NumericValue::F32(2.0),   // 从两米静止水位开始。
        10.0,                     // 向右拖十个物理像素。
        &config,                  // 使用当前字段独立设置。
        NumericInputSpec::f32(3), // 保留三位小数。
    );
    assert_eq!(value, NumericValue::F32(2.5)); // 2 + 10 × 0.1 × 0.5 = 2.5。
}

#[test]
fn right_drag_is_symmetric_and_clamped() {
    let config = linear_config(1.0); // 从标准一倍手感开始。
    assert_eq!(sensitivity_from_drag(1.0, 48.0, &config), 2.0); // 向右一个八度翻倍。
    assert_eq!(sensitivity_from_drag(1.0, -48.0, &config), 0.5); // 向左一个八度减半。
    assert_eq!(sensitivity_from_drag(1.0, 10_000.0, &config), 100.0); // 极端位移受固定上限保护。
}

#[test]
fn exponential_scrub_preserves_positive_range_and_snap() {
    let mut config = linear_config(1.0); // 复用安全范围和灵敏度边界。
    config.mapping = crate::types::NumericInputScrubMapping::Exponential {
        exponent_per_pixel: (500.0_f32).ln() / 120.0, // 一百二十像素覆盖2至1000的完整比例跨度。
    };
    config.drag_step = 0.5; // 半径保持半米吸附。
    config.minimum = 2.0; // 笔刷半径正式下界。
    config.maximum = 1_000.0; // 笔刷半径正式上界。
    let value = value_from_drag(
        NumericValue::F32(2.0),   // 从最小半径开始。
        120.0,                    // 向右拖完整设计行程。
        &config,                  // 使用指数映射。
        NumericInputSpec::f32(1), // 半米值使用一位小数显示。
    );
    assert_eq!(value, NumericValue::F32(1_000.0)); // 完整行程精确到达最大半径。
}

#[test]
fn signed_integer_scrub_uses_linear_steps() {
    let mut config = linear_config(1.0); // 复用受检灵敏度与阈值。
    config.mapping = crate::types::NumericInputScrubMapping::Linear {
        units_per_pixel: 1.0, // 每像素改变一个优先级单位。
    };
    config.drag_step = 1.0; // 有符号整数吸附到完整档位。
    config.minimum = -100.0; // 允许负优先级。
    config.maximum = 100.0; // 允许正优先级。
    let value = value_from_drag(
        NumericValue::I32(-3), // 从负优先级开始。
        5.0,                   // 向右拖五像素跨过零点。
        &config,               // 使用正式线性配置。
        NumericInputSpec::i32()
            .with_min(NumericValue::I32(-100)) // 精确输入和拖动共享下界。
            .with_max(NumericValue::I32(100)), // 精确输入和拖动共享上界。
    );
    assert_eq!(value, NumericValue::I32(2)); // -3 + 5得到正式有符号结果2。
}

#[test]
fn signed_integer_scrub_keeps_single_steps_near_i32_max() {
    let mut config = linear_config(1.0); // 使用标准倍率和明确一单位线性手感。
    config.mapping = crate::types::NumericInputScrubMapping::Linear {
        units_per_pixel: 1.0, // 每像素正好改变一个优先级。
    };
    config.drag_step = 1.0; // 拖动结果仍为正式整数档。
    config.minimum = i32::MIN as f32; // 公共配置保持轻量f32端点。
    config.maximum = i32::MAX as f32; // 即使端点本身舍入，中间计算也必须保留I32低位。
    let value = value_from_drag(
        NumericValue::I32(i32::MAX - 1), // f32无法直接区分该值和I32最大值。
        -1.0,                            // 向左一像素应精确减少一档。
        &config,
        NumericInputSpec::i32()
            .with_min(NumericValue::I32(i32::MIN))
            .with_max(NumericValue::I32(i32::MAX)),
    );
    assert_eq!(value, NumericValue::I32(i32::MAX - 2)); // 宽精度中间值不得卡死在端点。
}

#[test]
fn wrapped_float_scrub_crosses_angle_seam() {
    let mut config = linear_config(1.0); // 复用标准灵敏度范围。
    config.mapping = crate::types::NumericInputScrubMapping::Linear {
        units_per_pixel: 1.0, // 每像素旋转一度。
    };
    config.drag_step = 1.0; // 角度吸附到整度。
    config.minimum = 0.0; // 周期起点。
    config.maximum = 360.0; // 与零度等价的半开最大端点。
    let value = value_from_drag(
        NumericValue::F32(350.0), // 从接近周期末端开始。
        20.0,                     // 向右拖过接缝二十度。
        &config,                  // 拖动层必须保留370度候选。
        NumericInputSpec::f32(0)
            .with_min(NumericValue::F32(0.0)) // 角度周期起点。
            .with_max(NumericValue::F32(360.0)) // 角度周期终点。
            .with_wrap(true), // 正式规范化负责跨接缝循环。
    );
    assert_eq!(value, NumericValue::F32(10.0)); // 370度必须显示为10度。
}

#[derive(Resource, Default)]
struct FeedbackMessages {
    business_changed: usize,
    business_committed: Vec<NumericInputCommitted>,
    sensitivity_changed: Vec<f32>,
    sensitivity_committed: Vec<NumericInputScrubSensitivityCommitted>,
}

fn capture_feedback_messages(
    mut changed: MessageReader<NumericInputChanged>,
    mut committed: MessageReader<NumericInputCommitted>,
    mut sensitivity_changed: MessageReader<NumericInputScrubSensitivityChanged>,
    mut sensitivity_committed: MessageReader<NumericInputScrubSensitivityCommitted>,
    mut captured: ResMut<FeedbackMessages>,
) {
    captured.business_changed += changed.read().count();
    captured
        .business_committed
        .extend(committed.read().copied());
    captured
        .sensitivity_changed
        .extend(sensitivity_changed.read().map(|event| event.value));
    captured
        .sensitivity_committed
        .extend(sensitivity_committed.read().copied());
}

/// 使用正式builder和输入插件，只把已测量节点尺寸及指针位置作为CPU夹具输入。
fn feedback_fixture(
    value: NumericValue,
    input: NumericInputSpec,
    unit: &'static str,
) -> (App, Entity) {
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<FeedbackMessages>()
        .add_plugins(crate::UiWidgetsPlugin)
        .add_systems(
            Update,
            capture_feedback_messages.after(crate::UiWidgetsSystemSet::NumericInput),
        )
        .add_systems(Startup, move |mut commands: Commands| {
            commands
                .spawn((Node::default(), InheritedVisibility::VISIBLE))
                .with_children(|parent| {
                    crate::spawn_numeric_scrub_field_with_palette(
                        parent,
                        Name::new("反馈测试字段"),
                        Handle::default(),
                        crate::NumericScrubFieldOptions::new(
                            "X",
                            unit,
                            value,
                            input,
                            linear_config(1.0),
                        ),
                        crate::NumericScrubFieldPalette {
                            input: default(),
                            fill: Color::BLACK,
                            label: Color::WHITE,
                            value: Color::WHITE,
                            unit: Color::WHITE,
                            shadow: default(),
                        },
                    );
                });
        });
    app.update();
    let root = app
        .world_mut()
        .query_filtered::<Entity, With<NumericInputRoot>>()
        .single(app.world())
        .unwrap();
    app.world_mut().entity_mut(root).insert((
        InheritedVisibility::VISIBLE,
        ComputedNode {
            size: Vec2::new(120.0, 26.0),
            ..default()
        },
    ));
    point_feedback(&mut app, root, 0.0);
    (app, root)
}

fn tick_feedback(app: &mut App) {
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .clear();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
}

fn point_feedback(app: &mut App, root: Entity, x: f32) {
    app.world_mut().entity_mut(root).insert((
        UiInteraction::Hovered,
        RelativeCursorPosition {
            cursor_over: true,
            normalized: Some(Vec2::new(x / 120.0, 0.0)),
        },
    ));
}

fn feedback_entities(app: &mut App, root: Entity) -> (Entity, Entity, Vec<Entity>) {
    let world = app.world_mut();
    let value = world
        .query::<(Entity, &NumericInputDisplayText)>()
        .iter(world)
        .find_map(|(entity, part)| (part.root == root).then_some(entity))
        .unwrap();
    let feedback = world
        .query::<(Entity, &crate::NumericInputScrubSensitivityText)>()
        .iter(world)
        .find_map(|(entity, part)| (part.root == root).then_some(entity))
        .unwrap();
    let parts = world
        .query::<(Entity, &crate::NumericInputScrubValueText)>()
        .iter(world)
        .filter_map(|(entity, part)| (part.root == root).then_some(entity))
        .collect();
    (value, feedback, parts)
}

#[test]
fn right_scrub_feedback_updates_same_frame_and_keeps_precision_units_without_business_edits() {
    for (value, input, unit, display) in [
        (
            NumericValue::F32(-27.17),
            NumericInputSpec::f32(3),
            "m",
            "-27.170",
        ),
        (NumericValue::U32(4), NumericInputSpec::u32(), "", "4"),
        (NumericValue::I32(-4), NumericInputSpec::i32(), "层", "-4"),
    ] {
        let (mut app, root) = feedback_fixture(value, input, unit);
        let input = app.world().get::<NumericInputConfig>(root).unwrap().0; // 正式builder会关闭步进按钮，在初始化后冻结不应被反馈修改的完整规格。
        let (value_text, feedback, parts) = feedback_entities(&mut app, root);
        assert_eq!(parts.len(), if unit.is_empty() { 1 } else { 2 });
        assert_eq!(
            app.world().get::<Node>(feedback).unwrap().display,
            Display::None
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        tick_feedback(&mut app);
        point_feedback(&mut app, root, 48.0);
        tick_feedback(&mut app); // 仍按住右键，倍率与原输入框显隐必须已经完成。
        assert_eq!(app.world().get::<Text>(feedback).unwrap().0, "灵敏度 2.00×");
        assert_eq!(
            app.world().get::<Node>(feedback).unwrap().display,
            Display::Flex
        );
        assert!(
            parts
                .iter()
                .all(|entity| app.world().get::<Node>(*entity).unwrap().display == Display::Flex)
        );
        assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, value);
        assert_eq!(app.world().get::<Text>(value_text).unwrap().0, display);
        assert_eq!(
            app.world().get::<NumericInputConfig>(root).unwrap().0,
            input
        );
        let captured = app.world().resource::<FeedbackMessages>();
        assert_eq!(captured.business_changed, 0);
        assert!(captured.business_committed.is_empty());
        assert_eq!(captured.sensitivity_changed, vec![2.0]);
        assert!(captured.sensitivity_committed.is_empty()); // 按住期间仍不能持久化。

        let text_tick = app
            .world()
            .entity(feedback)
            .get_ref::<Text>()
            .unwrap()
            .last_changed();
        let node_tick = app
            .world()
            .entity(feedback)
            .get_ref::<Node>()
            .unwrap()
            .last_changed();
        tick_feedback(&mut app);
        assert_eq!(
            app.world()
                .entity(feedback)
                .get_ref::<Text>()
                .unwrap()
                .last_changed(),
            text_tick
        );
        assert_eq!(
            app.world()
                .entity(feedback)
                .get_ref::<Node>()
                .unwrap()
                .last_changed(),
            node_tick
        );
        point_feedback(&mut app, root, -48.0);
        tick_feedback(&mut app);
        assert_eq!(app.world().get::<Text>(feedback).unwrap().0, "灵敏度 0.50×");

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Right);
        tick_feedback(&mut app);
        assert_eq!(
            app.world().get::<Node>(feedback).unwrap().display,
            Display::None
        );
        assert!(
            parts
                .iter()
                .all(|entity| app.world().get::<Node>(*entity).unwrap().display == Display::Flex)
        );
        assert_eq!(app.world().get::<Text>(value_text).unwrap().0, display);
        assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, value);
        let captured = app.world().resource::<FeedbackMessages>();
        assert_eq!(captured.business_changed, 0);
        assert!(captured.business_committed.is_empty());
        assert_eq!(captured.sensitivity_committed.len(), 1);
        assert_eq!(captured.sensitivity_committed[0].before, 1.0);
        assert_eq!(captured.sensitivity_committed[0].after, 0.5);
        for child in app.world().get::<Children>(root).unwrap().iter() {
            if let Some(text) = app.world().get::<Text>(child)
                && (text.0 == "X" || (!unit.is_empty() && text.0 == unit))
            {
                assert_eq!(
                    app.world().get::<Node>(child).unwrap().display,
                    Display::Flex
                ); // 原轴标签与单位仍保留原节点。
            }
        }
    }
}

#[test]
fn right_feedback_resumes_after_pointer_returns_at_the_same_saturated_multiplier() {
    let (mut app, root) = feedback_fixture(NumericValue::F32(1.0), NumericInputSpec::f32(2), "m");
    let (_, feedback, _) = feedback_entities(&mut app, root);
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
    tick_feedback(&mut app);
    assert_eq!(app.world().get::<Node>(feedback).unwrap().display, Display::None); // 右键短按仍不形成拖动。
    point_feedback(&mut app, root, 480.0);
    tick_feedback(&mut app);
    assert_eq!(app.world().get::<Text>(feedback).unwrap().0, "灵敏度 100.00×");
    app.world_mut().get_mut::<RelativeCursorPosition>(root).unwrap().normalized = None;
    tick_feedback(&mut app);
    assert_eq!(app.world().get::<Node>(feedback).unwrap().display, Display::None);
    point_feedback(&mut app, root, 500.0);
    tick_feedback(&mut app);
    assert_eq!(app.world().get::<Node>(feedback).unwrap().display, Display::Flex);
    assert_eq!(app.world().get::<Text>(feedback).unwrap().0, "灵敏度 100.00×");
    assert_eq!(app.world().get::<NumericInputValue>(root).unwrap().0, NumericValue::F32(1.0));
}

#[test]
fn feedback_keeps_left_scrub_single_commit_and_middle_cancel_protocol() {
    let input = NumericInputSpec::f32(3).with_replace_on_first_key(true);
    let (mut app, root) = feedback_fixture(NumericValue::F32(1.0), input, "m");
    let (value_text, feedback, parts) = feedback_entities(&mut app, root);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick_feedback(&mut app);
    point_feedback(&mut app, root, 10.0);
    tick_feedback(&mut app);
    assert_eq!(app.world().get::<Text>(value_text).unwrap().0, "2.000");
    assert_eq!(
        app.world().get::<Node>(feedback).unwrap().display,
        Display::None
    );
    assert!(
        parts
            .iter()
            .all(|entity| app.world().get::<Node>(*entity).unwrap().display == Display::Flex)
    );
    assert!(
        app.world()
            .resource::<FeedbackMessages>()
            .business_committed
            .is_empty()
    );
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick_feedback(&mut app);
    assert_eq!(
        app.world()
            .resource::<FeedbackMessages>()
            .business_committed
            .len(),
        1
    );
    assert_eq!(
        app.world()
            .resource::<FeedbackMessages>()
            .business_committed[0]
            .source,
        NumericInputCommitSource::Scrub
    );

    point_feedback(&mut app, root, 0.0);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Middle);
    tick_feedback(&mut app);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Middle);
    tick_feedback(&mut app);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Digit9);
    tick_feedback(&mut app);
    assert_eq!(app.world().get::<Text>(value_text).unwrap().0, "9");
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Escape);
    tick_feedback(&mut app);
    assert_eq!(app.world().get::<Text>(value_text).unwrap().0, "2.000");
    assert_eq!(
        app.world().get::<NumericInputValue>(root).unwrap().0,
        NumericValue::F32(2.0)
    );
    assert_eq!(
        app.world().get::<Node>(feedback).unwrap().display,
        Display::None
    );
    let captured = app.world().resource::<FeedbackMessages>();
    assert_eq!(captured.business_committed.len(), 2);
    assert_eq!(
        captured.business_committed[1].outcome,
        NumericInputCommitOutcome::Canceled
    );
    assert!(captured.sensitivity_committed.is_empty());
}
