// 游戏设置 — 窗口目标 3D 相机画面亮度 / 曝光补偿应用系统。

use bevy::{
    camera::RenderTarget,       // 区分窗口输出与角色预览、小地图等纹理输出。
    prelude::*,                 // ECS、Camera3d 与系统参数。
    render::view::ColorGrading, // 全局曝光补偿在色调映射前以 EV 档应用。
};

use crate::{
    DEFAULT_EXPOSURE_COMPENSATION, RenderSettings, components::AuthoredWindowExposure,
    sanitize_exposure_compensation,
}; // 玩家显示设置、范围规范化与相机作者状态备份。

const EXPOSURE_EPSILON: f32 = 0.000_1; // 避免浮点尾差导致稳定帧反复标记 ColorGrading 变更。

/// 将玩家曝光补偿应用到窗口 3D 相机，并在归零或离开窗口目标时恢复作者基线。
pub(crate) fn apply_exposure_compensation(
    mut commands: Commands,      // 只在取得或归还曝光所有权时增删备份组件。
    settings: Res<RenderSettings>, // 显示页即时写入的统一设置资源。
    mut cameras: Query<
        (
            Entity,                          // 当前 3D 相机实体。
            &RenderTarget,                   // 纹理目标相机必须保持作者配置。
            &mut ColorGrading,               // 仅修改 global.exposure，保留其余颜色分级字段。
            Option<&AuthoredWindowExposure>, // 首次接管前记录的作者 EV 基线。
        ),
        With<Camera3d>, // 2D UI 相机与纯 UI 输出不参与曝光设置。
    >,
) {
    let compensation = sanitize_exposure_compensation(settings.exposure_compensation); // 每帧只规范化一次用户值。
    let enabled = (compensation - DEFAULT_EXPOSURE_COMPENSATION).abs() > EXPOSURE_EPSILON; // 零档完全归还作者控制权。

    for (entity, target, mut color_grading, authored) in &mut cameras {
        let renders_to_window = matches!(target, RenderTarget::Window(_)); // 主菜单与玩法窗口相机共享设置。
        if !enabled || !renders_to_window {
            if let Some(authored) = authored {
                if (color_grading.global.exposure - authored.0).abs() > EXPOSURE_EPSILON {
                    color_grading.global.exposure = authored.0; // 只在不一致时恢复作者曝光字段。
                }
                commands.entity(entity).remove::<AuthoredWindowExposure>(); // 恢复后立即归还所有权。
            }
            continue; // 从未接管的中性档或纹理相机不附加任何私有状态。
        }

        let authored_exposure = authored.map_or(color_grading.global.exposure, |value| value.0); // 新相机首次捕获当前作者基线。
        if authored.is_none() {
            commands
                .entity(entity)
                .insert(AuthoredWindowExposure(authored_exposure)); // 每台符合条件的窗口相机只备份一次。
        }
        let desired_exposure = authored_exposure + compensation; // 玩家值是相对作者配置的 EV 偏移而非绝对覆盖。
        if (color_grading.global.exposure - desired_exposure).abs() > EXPOSURE_EPSILON {
            color_grading.global.exposure = desired_exposure; // 档位或新相机变化时才更新 GPU 颜色分级输入。
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 复用正式应用系统与所有权组件。
    use bevy::window::WindowRef; // 构造主窗口输出目标。

    /// 建立只运行正式曝光应用系统的轻量 ECS 测试应用。
    fn test_app() -> App {
        let mut app = App::new(); // 组件状态测试不需要初始化真实渲染器。
        app.init_resource::<RenderSettings>(); // 使用正式中性默认设置。
        app.add_systems(Update, apply_exposure_compensation); // 注册待测运行时系统。
        app
    }

    /// 生成带指定作者曝光的完整窗口 3D 相机。
    fn spawn_window_camera(app: &mut App, authored_exposure: f32) -> Entity {
        let mut color_grading = ColorGrading::default(); // 从 Bevy 正式默认颜色分级开始。
        color_grading.global.exposure = authored_exposure; // 注入场景作者自定义基线。
        app.world_mut()
            .spawn((
                Camera3d::default(),                      // 建立窗口 3D 相机身份。
                RenderTarget::Window(WindowRef::Primary), // 允许全局显示设置接管。
                color_grading,                            // 显式提供待验证颜色分级组件。
            ))
            .id() // 返回相机实体供测试读取。
    }

    #[test] // 中性默认值必须完全不接管或改写作者相机。
    fn neutral_compensation_preserves_authored_exposure_without_ownership() {
        let mut app = test_app(); // 使用默认零档。
        let camera = spawn_window_camera(&mut app, 0.35); // 模拟作者已有正曝光分级。
        app.update(); // 执行稳定默认帧。

        assert_eq!(
            app.world()
                .get::<ColorGrading>(camera)
                .unwrap()
                .global
                .exposure,
            0.35,
        ); // 默认显示设置不得改变作者曝光。
        assert!(app.world().get::<AuthoredWindowExposure>(camera).is_none()); // 零档不得占有相机控制权。
    }

    #[test] // 非零补偿必须相对作者基线应用，并在归零后精确恢复。
    fn compensation_is_relative_and_neutral_restores_authored_exposure() {
        let mut app = test_app(); // 使用正式应用系统。
        let camera = spawn_window_camera(&mut app, -0.2); // 作者相机本身略暗。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .exposure_compensation = 0.7; // 玩家选择增加 0.7 EV。
        app.update(); // 捕获作者基线并应用补偿。
        assert!(
            (app.world()
                .get::<ColorGrading>(camera)
                .unwrap()
                .global
                .exposure
                - 0.5)
                .abs()
                < EXPOSURE_EPSILON
        ); // -0.2 + 0.7 应得到 0.5 EV。
        assert_eq!(
            app.world().get::<AuthoredWindowExposure>(camera),
            Some(&AuthoredWindowExposure(-0.2)),
        ); // 首次接管必须保存作者真实基线。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .exposure_compensation = DEFAULT_EXPOSURE_COMPENSATION; // 玩家恢复中性零档。
        app.update(); // 精确恢复并归还所有权。
        assert_eq!(
            app.world()
                .get::<ColorGrading>(camera)
                .unwrap()
                .global
                .exposure,
            -0.2,
        ); // 作者曝光必须完整恢复。
        assert!(app.world().get::<AuthoredWindowExposure>(camera).is_none()); // 恢复后不得残留私有备份。
    }

    #[test] // 后生成窗口相机应继承，纹理目标相机必须保持专用曝光。
    fn new_window_camera_inherits_while_texture_camera_is_isolated() {
        let mut app = test_app(); // 使用窗口目标过滤路径。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .exposure_compensation = -0.6; // 在创建相机前选择变暗档。
        let window_camera = spawn_window_camera(&mut app, 0.1); // 模拟后续切图生成的玩法相机。
        let mut texture_grading = ColorGrading::default(); // 创建角色预览专用颜色分级。
        texture_grading.global.exposure = 0.8; // 作者为预览相机配置独立曝光。
        let texture_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                     // 建立纹理 3D 相机身份。
                RenderTarget::None { size: UVec2::ONE }, // 模拟角色预览、小地图或无颜色目标。
                texture_grading,                         // 注入作者专用曝光。
            ))
            .id(); // 保存纹理相机实体。
        app.update(); // 让新相机继承当前设置。

        assert!(
            (app.world()
                .get::<ColorGrading>(window_camera)
                .unwrap()
                .global
                .exposure
                + 0.5)
                .abs()
                < EXPOSURE_EPSILON
        ); // 窗口相机应得到 0.1 - 0.6 = -0.5 EV。
        assert_eq!(
            app.world()
                .get::<ColorGrading>(texture_camera)
                .unwrap()
                .global
                .exposure,
            0.8,
        ); // 纹理相机保持作者曝光。
        assert!(
            app.world()
                .get::<AuthoredWindowExposure>(texture_camera)
                .is_none()
        ); // 隔离目标不得附加窗口所有权组件。
    }

    #[test] // 已接管相机改作纹理目标时必须立即恢复作者曝光。
    fn camera_changing_to_texture_target_restores_authored_exposure() {
        let mut app = test_app(); // 使用正式目标切换恢复路径。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .exposure_compensation = 0.9; // 开启明显变亮补偿。
        let camera = spawn_window_camera(&mut app, 0.25); // 首帧作为窗口相机。
        app.update(); // 应用曝光并取得所有权。
        app.world_mut()
            .entity_mut(camera)
            .insert(RenderTarget::None { size: UVec2::ONE }); // 模拟复用为角色预览纹理目标。
        app.update(); // 离开窗口范围后恢复作者状态。

        assert_eq!(
            app.world()
                .get::<ColorGrading>(camera)
                .unwrap()
                .global
                .exposure,
            0.25,
        ); // 纹理阶段恢复作者原曝光。
        assert!(app.world().get::<AuthoredWindowExposure>(camera).is_none()); // 窗口专用备份必须清理。
    }

    #[test] // 设置与相机身份稳定时不得重复触发颜色分级上传。
    fn stable_frame_does_not_rewrite_color_grading_or_ownership() {
        let mut app = test_app(); // 使用正式状态比较路径。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .exposure_compensation = 0.4; // 固定非零补偿以保持接管状态。
        let camera = spawn_window_camera(&mut app, 0.0); // 建立作者中性窗口相机。
        app.update(); // 首帧应用并记录作者状态。
        app.world_mut().clear_trackers(); // 清除首次合法变更标记。
        app.update(); // 稳定帧只比较，不应写组件。

        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<ColorGrading>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记 ColorGrading 变更",
        );
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<AuthoredWindowExposure>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复保存曝光作者状态",
        );
    }
}
