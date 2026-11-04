// 游戏设置 — 主窗口呈现应用系统。

use bevy::{
    prelude::*,                           // ECS 与窗口组件常用类型。
    window::{PresentMode, PrimaryWindow}, // 主窗口垂直同步控制。
};

use crate::RenderSettings; // 当前统一游戏设置。

/// 将垂直同步应用到主窗口；稳定帧不重复写入交换链模式。
pub(crate) fn apply_vsync(
    settings: Res<RenderSettings>, // 已加载且可被 UI 即时修改的设置。
    mut windows: Query<&mut Window, With<PrimaryWindow>>, // 只影响实际输出主窗口。
) {
    let desired = if settings.graphics.vsync {
        PresentMode::AutoVsync // 开启时优先无撕裂呈现。
    } else {
        PresentMode::AutoNoVsync // 关闭时优先低延迟非同步呈现。
    };
    let Ok(mut window) = windows.single_mut() else {
        return;
    }; // 无窗口或多主窗口时安全跳过。
    if window.present_mode != desired {
        window.present_mode = desired; // 只在实际变化时重建交换链。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vsync_updates_primary_window_without_touching_stable_frames() {
        let mut app = App::new();
        app.init_resource::<RenderSettings>();
        app.add_systems(Update, apply_vsync);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .vsync = true;
        app.update();
        assert_eq!(
            app.world().get::<Window>(window).unwrap().present_mode,
            PresentMode::AutoVsync,
        );
        app.world_mut().clear_trackers();
        app.update();
        assert!(
            !app.world()
                .entity(window)
                .get_ref::<Window>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记窗口组件变更",
        );
    }
}
