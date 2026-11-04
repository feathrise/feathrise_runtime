//! FSR 诊断视图的主世界辅助控制。

use crate::types::Fsr3DebugView; // 只有显式 AMD 调试进程才接管诊断快捷键。
use bevy::prelude::*; // 输入、UI 根节点与可见性组件。

/// 当前进程的诊断 HUD 显隐状态；不进入玩家设置或存档。
#[derive(Resource, Clone, Copy, Debug, Default)]
pub(crate) struct Fsr3DebugHudVisibility {
    hidden: bool, // F10 只在 AMD 调试视图启用时翻转该状态。
}

/// 记录被诊断功能临时覆盖前的 UI 根可见性，恢复时不误显示原本关闭的菜单。
#[derive(Component, Clone, Debug)]
pub(crate) struct HiddenForFsr3Debug(Visibility); // 系统由 crate 根注册，签名涉及的组件须保持 crate 内可见。

/// 在 AMD 3×3 调试视图中用 F10 临时隐藏或恢复全部屏幕空间 Bevy UI。
///
/// 【功能设计】只处理没有 `ChildOf` 的 `Node` 根；后代依靠 Bevy 可见性传播自动隐藏，
/// 世界空间模型、草叶、名牌和运动矢量输入保持不变。调试期间新生成的 UI 根也会在同帧
/// 被接管，退出时按每个根保存的原状态恢复，避免把原本关闭的菜单错误显示出来。
pub(crate) fn toggle_fsr3_debug_hud(
    keyboard: Res<ButtonInput<KeyCode>>, // F10 未占用项目现有 F1-F5/F9/F12 开发入口。
    debug_view: Res<Fsr3DebugView>,      // 正常进程完全忽略该快捷键。
    mut state: ResMut<Fsr3DebugHudVisibility>,
    mut roots: Query<
        (Entity, &mut Visibility, Option<&HiddenForFsr3Debug>),
        (With<Node>, Without<ChildOf>),
    >,
    mut commands: Commands,
) {
    if !debug_view.0 {
        return; // 发行画面和普通 FSR 输出不增加任何 HUD 行为变化。
    }

    if keyboard.just_pressed(KeyCode::F10) {
        state.hidden = !state.hidden;
        info!(
            "FSR 诊断截图 HUD 已{}；再次按 F10 恢复",
            if state.hidden { "隐藏" } else { "恢复" }
        ); // 每次人工切换只记录一次，便于确认截图状态。
    }

    for (entity, mut visibility, saved) in &mut roots {
        match (state.hidden, saved) {
            (true, None) => {
                let previous = visibility.clone(); // 必须在覆盖前保留该根自己的真实状态。
                *visibility = Visibility::Hidden;
                commands.entity(entity).insert(HiddenForFsr3Debug(previous));
            }
            (true, Some(_)) => {
                *visibility = Visibility::Hidden; // 抵消其他 HUD 系统在诊断期间重新显示根节点。
            }
            (false, Some(saved)) => {
                *visibility = saved.0.clone();
                commands.entity(entity).remove::<HiddenForFsr3Debug>();
            }
            (false, None) => {} // 未被诊断功能接管的根保持作者当前状态。
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debug_hud_test_app(debug_view: bool) -> App {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(Fsr3DebugView(debug_view))
            .init_resource::<Fsr3DebugHudVisibility>()
            .add_systems(Update, toggle_fsr3_debug_hud);
        app
    }

    #[test]
    fn f10_hides_and_restores_only_ui_roots_in_debug_view() {
        let mut app = debug_hud_test_app(true);
        let visible_root = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited))
            .id();
        let hidden_root = app
            .world_mut()
            .spawn((Node::default(), Visibility::Hidden))
            .id();
        let parent = app.world_mut().spawn_empty().id();
        let child = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited, ChildOf(parent)))
            .id(); // 非根 UI 节点由父级传播，不应被单独记录或覆盖。

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F10);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(visible_root),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().get::<Visibility>(hidden_root),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().get::<Visibility>(child),
            Some(&Visibility::Inherited)
        );

        {
            let mut keyboard = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keyboard.reset(KeyCode::F10); // 纯测试 App 没有 InputPlugin，须显式模拟松键和帧末边沿清理。
            keyboard.press(KeyCode::F10);
        }
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(visible_root),
            Some(&Visibility::Inherited)
        );
        assert_eq!(
            app.world().get::<Visibility>(hidden_root),
            Some(&Visibility::Hidden)
        ); // 原本隐藏的根必须恢复为隐藏，而不是统一改成 Inherited。
    }

    #[test]
    fn normal_process_ignores_f10() {
        let mut app = debug_hud_test_app(false);
        let root = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited))
            .id();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F10);

        app.update();

        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Inherited)
        );
    }
}
