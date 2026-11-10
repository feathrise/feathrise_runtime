// 游戏设置 — 窗口目标 3D 相机 CAS 自适应锐化应用系统。

use bevy::{
    anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening, // Bevy 0.19 原生 CAS 后处理组件。
    camera::RenderTarget, // 区分窗口输出与角色预览、小场景等纹理输出。
    prelude::*,           // ECS、Camera3d 与系统参数。
};

use crate::{
    DlssRuntimeStatus, RenderSettings, SharpeningQuality,
    components::{AuthoredWindowSharpening, DlssPostProcessTarget, Fsr3PostProcessTarget},
}; // 玩家普通与 DLSS 专用锐化状态、运行路径及每台相机作者备份。

/// 将 CAS 应用到窗口 3D 相机；关闭或离开窗口目标时恢复首次接管前的作者状态。
#[allow(clippy::type_complexity)] // 单个 Query 快照同时承载目标、当前 CAS 与作者备份，保证精确恢复。
pub(crate) fn apply_sharpening(
    mut commands: Commands,      // 仅在 CAS 或所有权状态真实变化时提交组件命令。
    settings: Res<RenderSettings>, // 当前统一画质设置。
    dlss_status: Res<DlssRuntimeStatus>, // 判断专用 DLSS 锐化是否已取得目标相机控制权。
    cameras: Query<
        (
            Entity,                              // 当前 3D 相机实体。
            &RenderTarget,                       // 用于隔离窗口与纹理目标。
            Option<&ContrastAdaptiveSharpening>, // 当前可能存在的作者或全局 CAS 参数。
            Option<&AuthoredWindowSharpening>,   // 首次接管前保存的作者状态。
            Has<DlssPostProcessTarget>,          // 只对真实 SR/RR 目标应用 DLSS 百分比锐化。
            Has<Fsr3PostProcessTarget>,          // FSR内部RCAS接管时必须暂停普通Bevy CAS。
        ),
        With<Camera3d>, // CAS 设置只作用于 3D 相机。
    >,
) {
    let ordinary_desired = sharpening_for_quality(settings.graphics.sharpening_quality); // 普通画质档一帧只构造一次。
    let dlss_desired = sharpening_for_dlss_percent(settings.graphics.dlss_sharpening_percent); // 百分比连续映射且 0% 不构造 CAS。
    for (entity, target, current, authored, dlss_target, fsr3_target) in &cameras {
        let renders_to_window = matches!(target, RenderTarget::Window(_)); // 主菜单与玩法窗口相机共享设置。
        if !renders_to_window {
            if let Some(authored) = authored {
                restore_authored_sharpening(&mut commands, entity, current, *authored); // 相机改为纹理输出后立即归还作者控制权。
                commands.entity(entity).remove::<AuthoredWindowSharpening>(); // 归还后清理窗口专用备份，纹理相机不保留全局私有状态。
            }
            continue; // 从未接管的纹理相机保持专用配置且不附加备份。
        }

        let desired = if fsr3_target {
            None // FSR dispatch内部完成可选RCAS，外部CAS会重复锐化并污染时间输出。
        } else if dlss_target && dlss_status.is_dlss_effective() {
            dlss_desired.as_ref() // DLSS 生效时暂时覆盖普通锐化但不改持久值。
        } else {
            ordinary_desired.as_ref() // DLSS 关闭、失败或非目标相机使用玩家普通 CAS。
        };
        if let Some(desired) = desired {
            if authored.is_none() {
                commands
                    .entity(entity)
                    .insert(capture_authored_sharpening(current)); // 每台窗口相机仅在首次启用 CAS 时记录作者基线。
            }
            if current.is_none_or(|current| !sharpening_matches(current, desired)) {
                commands.entity(entity).insert(desired.clone()); // 新相机继承或档位变化时更新 CAS。
            }
        } else if let Some(authored) = authored {
            restore_authored_sharpening(&mut commands, entity, current, *authored); // 关闭全局 CAS 后精确恢复作者组件。
            commands.entity(entity).remove::<AuthoredWindowSharpening>(); // 恢复完成即归还所有权，关闭期间允许作者系统继续管理 CAS。
        }
    }
}

/// 按 DLSS 后处理百分比构造连续强度 CAS；该值不是 NGX 内部锐化参数。
fn sharpening_for_dlss_percent(percent: u8) -> Option<ContrastAdaptiveSharpening> {
    let percent = percent.min(100); // 防御外部代码绕过设置规范化写入非法大值。
    (percent > 0).then_some(ContrastAdaptiveSharpening {
        enabled: true, // 非零百分比才进入 Bevy CAS 后处理节点。
        sharpening_strength: f32::from(percent) / 100.0, // 0..=100 连续映射到 0.0..=1.0。
        denoise: false, // 与普通 CAS 保持同一无胶片颗粒管线变体。
    })
}

/// 按玩家档位构造固定强度、关闭降噪的 CAS 参数。
fn sharpening_for_quality(quality: SharpeningQuality) -> Option<ContrastAdaptiveSharpening> {
    quality
        .sharpening_strength()
        .map(|sharpening_strength| ContrastAdaptiveSharpening {
            enabled: true,       // 启用档必须进入 Bevy CAS 提取与后处理节点。
            sharpening_strength, // 低档 0.25，高档 0.60。
            denoise: false,      // Bevy 官方建议通常保持关闭，避免为无胶片颗粒画面增加额外变体。
        })
}

/// 捕获作者是否声明 CAS 以及全部可配置字段。
fn capture_authored_sharpening(
    current: Option<&ContrastAdaptiveSharpening>, // 首次接管前的相机 CAS 组件。
) -> AuthoredWindowSharpening {
    current.map_or(
        AuthoredWindowSharpening {
            present: false,           // 作者没有声明 CAS。
            enabled: false,           // 缺失组件时该字段只作为安全占位。
            sharpening_strength: 0.0, // 缺失组件时不保留无效强度。
            denoise: false,           // 缺失组件时不保留无效管线变体。
        },
        |current| AuthoredWindowSharpening {
            present: true,                                    // 作者原本声明了 CAS。
            enabled: current.enabled,                         // 保存原启用状态。
            sharpening_strength: current.sharpening_strength, // 保存原始强度。
            denoise: current.denoise,                         // 保存原降噪选择。
        },
    )
}

/// 恢复作者 CAS 组件；作者原本没有组件时只移除本功能引入的部分。
fn restore_authored_sharpening(
    commands: &mut Commands,                      // 相机组件写入入口。
    entity: Entity,                               // 当前 3D 相机实体。
    current: Option<&ContrastAdaptiveSharpening>, // 当前 CAS 状态。
    authored: AuthoredWindowSharpening,           // 首次接管前保存的作者状态。
) {
    if authored.present {
        let desired = ContrastAdaptiveSharpening {
            enabled: authored.enabled,                         // 恢复作者原启用状态。
            sharpening_strength: authored.sharpening_strength, // 恢复作者原强度。
            denoise: authored.denoise,                         // 恢复作者原降噪选择。
        };
        if current.is_none_or(|current| !sharpening_matches(current, &desired)) {
            commands.entity(entity).insert(desired); // 仅状态不一致时写回作者参数。
        }
    } else if current.is_some() {
        commands
            .entity(entity)
            .remove::<ContrastAdaptiveSharpening>(); // 移除仅由全局画质引入的 CAS。
    }
}

/// 比较全部会影响 CAS 提取、uniform 或管线变体的字段。
fn sharpening_matches(
    current: &ContrastAdaptiveSharpening, // 相机当前参数。
    desired: &ContrastAdaptiveSharpening, // 当前档位或作者基线参数。
) -> bool {
    current.enabled == desired.enabled
        && current.sharpening_strength == desired.sharpening_strength
        && current.denoise == desired.denoise
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::WindowRef; // 构造主窗口渲染目标。

    /// 建立只运行正式 CAS 应用系统的轻量 ECS 测试应用。
    fn test_app() -> App {
        let mut app = App::new(); // CAS 组件测试不需要初始化真实渲染器。
        app.init_resource::<RenderSettings>(); // 使用正式默认关闭基线。
        app.init_resource::<DlssRuntimeStatus>(); // 普通测试默认保持光栅路径。
        app.add_systems(Update, apply_sharpening); // 注册待测系统。
        app
    }

    #[test]
    fn low_high_and_off_restore_absent_author_component() {
        let mut app = test_app(); // 使用正式设置资源和运行时系统。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立窗口 3D 相机身份。
                RenderTarget::Window(WindowRef::Primary), // 允许全局 CAS 接管。
            ))
            .id(); // 保存实体用于逐档断言。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::Low; // 选择轻度锐化。
        app.update(); // 应用低档 CAS。
        let low = app
            .world()
            .get::<ContrastAdaptiveSharpening>(camera)
            .unwrap(); // 读取低档参数。
        assert!(low.enabled); // 启用档必须进入后处理。
        assert_eq!(low.sharpening_strength, 0.25); // 低档固定为 0.25。
        assert!(!low.denoise); // 当前画面不启用额外噪点保护。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::High; // 切到标准高清晰度。
        app.update(); // 更新既有 CAS 组件。
        assert_eq!(
            app.world()
                .get::<ContrastAdaptiveSharpening>(camera)
                .unwrap()
                .sharpening_strength,
            0.60,
        ); // 高档固定为 Bevy 默认 0.60。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::Off; // 关闭全局锐化。
        app.update(); // 恢复首次接管前的无组件状态。
        assert!(
            app.world()
                .get::<ContrastAdaptiveSharpening>(camera)
                .is_none()
        ); // 本功能引入的 CAS 必须完全移除。
        assert!(
            app.world()
                .get::<AuthoredWindowSharpening>(camera)
                .is_none()
        ); // 关闭后必须归还所有权而不是常驻私有备份。
    }

    #[test]
    fn off_restores_authored_parameters_exactly() {
        let mut app = test_app(); // 使用正式所有权恢复路径。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立窗口 3D 相机身份。
                RenderTarget::Window(WindowRef::Primary), // 允许全局 CAS 临时覆盖。
                ContrastAdaptiveSharpening {
                    enabled: false,           // 模拟作者保留但停用的 CAS。
                    sharpening_strength: 0.4, // 模拟作者专用强度。
                    denoise: true,            // 模拟作者专用管线变体。
                },
            ))
            .id(); // 保存实体用于恢复断言。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::High; // 临时应用全局高档。
        app.update(); // 捕获并覆盖作者 CAS。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::Off; // 归还作者控制权。
        app.update(); // 执行精确恢复。

        let restored = app
            .world()
            .get::<ContrastAdaptiveSharpening>(camera)
            .unwrap(); // 作者组件必须仍存在。
        assert!(!restored.enabled); // 恢复作者停用状态。
        assert_eq!(restored.sharpening_strength, 0.4); // 恢复作者强度。
        assert!(restored.denoise); // 恢复作者降噪选择。
        assert!(
            app.world()
                .get::<AuthoredWindowSharpening>(camera)
                .is_none()
        ); // 作者状态恢复后全局所有权必须清理。
    }

    #[test]
    fn new_window_cameras_inherit_while_texture_cameras_remain_authored() {
        let mut app = test_app(); // 使用正式窗口目标过滤。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::High; // 在生成相机前选择高档。
        let window_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 新主菜单或玩法相机。
                RenderTarget::Window(WindowRef::Primary), // 输出到窗口。
            ))
            .id(); // 保存窗口相机实体。
        let texture_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                     // 角色预览或小场景相机。
                RenderTarget::None { size: UVec2::ONE }, // 模拟专用纹理目标。
                ContrastAdaptiveSharpening {
                    enabled: true,            // 作者专用 CAS 保持开启。
                    sharpening_strength: 0.9, // 作者专用强度不得被覆盖。
                    denoise: true,            // 作者专用变体不得被覆盖。
                },
            ))
            .id(); // 保存纹理相机实体。
        app.update(); // 让后生成相机继承当前全局档。

        assert_eq!(
            app.world()
                .get::<ContrastAdaptiveSharpening>(window_camera)
                .unwrap()
                .sharpening_strength,
            0.60,
        ); // 窗口相机继承高档。
        let texture_cas = app
            .world()
            .get::<ContrastAdaptiveSharpening>(texture_camera)
            .unwrap(); // 读取作者专用配置。
        assert_eq!(texture_cas.sharpening_strength, 0.9); // 纹理相机保持作者强度。
        assert!(texture_cas.denoise); // 纹理相机保持作者变体。
        assert!(
            app.world()
                .get::<AuthoredWindowSharpening>(texture_camera)
                .is_none()
        ); // 隔离相机不得附加全局所有权组件。
    }

    #[test]
    fn camera_that_changes_to_a_texture_target_restores_author_state() {
        let mut app = test_app(); // 使用正式目标切换恢复路径。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::High; // 先开启全局高档。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立原本输出到窗口的相机。
                RenderTarget::Window(WindowRef::Primary), // 首帧允许全局 CAS 接管。
            ))
            .id(); // 保存实体用于切换目标。
        app.update(); // 安装全局 CAS 并记录作者原本无组件。
        assert!(
            app.world()
                .get::<ContrastAdaptiveSharpening>(camera)
                .is_some()
        ); // 窗口阶段必须启用 CAS。

        app.world_mut()
            .entity_mut(camera)
            .insert(RenderTarget::None { size: UVec2::ONE }); // 模拟相机复用为角色预览纹理目标。
        app.update(); // 离开窗口目标后归还作者状态。
        assert!(
            app.world()
                .get::<ContrastAdaptiveSharpening>(camera)
                .is_none()
        ); // 作者原本无 CAS，因此纹理阶段不得残留全局组件。
        assert!(
            app.world()
                .get::<AuthoredWindowSharpening>(camera)
                .is_none()
        ); // 纹理阶段也不得残留窗口专用作者备份。
    }

    #[test]
    fn stable_sharpening_frame_does_not_rewrite_components() {
        let mut app = test_app(); // 使用正式状态比较路径。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .sharpening_quality = SharpeningQuality::Low; // 固定低档。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立窗口 3D 相机。
                RenderTarget::Window(WindowRef::Primary), // 允许全局 CAS 接管。
            ))
            .id(); // 保存实体用于变更检测。
        app.update(); // 首帧合法安装 CAS 与作者备份。
        app.world_mut().clear_trackers(); // 清除首帧变更标记。
        app.update(); // 相同设置的稳定帧不得重复写入。

        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<ContrastAdaptiveSharpening>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记 CAS 参数变更",
        );
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<AuthoredWindowSharpening>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复保存 CAS 作者状态",
        );
    }

    #[test]
    fn dlss_percent_replaces_ordinary_cas_only_on_real_target() {
        let mut app = test_app(); // 使用正式作者所有权与结构化 DLSS 状态。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立正式三维窗口相机。
                RenderTarget::Window(WindowRef::Primary), // 允许 CAS 接管。
                DlssPostProcessTarget,                    // 模拟 SR/RR 已真实挂接。
            ))
            .id();
        {
            let mut settings = app.world_mut().resource_mut::<RenderSettings>(); // 同时保留普通高档与专用 37%。
            settings.graphics.sharpening_quality = SharpeningQuality::High; // 关闭 DLSS 后应恢复的最新玩家选择。
            settings.graphics.dlss_sharpening_percent = 37; // 连续专用后处理强度。
        }
        {
            let mut status = app.world_mut().resource_mut::<DlssRuntimeStatus>(); // 模拟光栅 + SR 真实有效。
            status.path = crate::DlssRuntimePath::RasterSuperResolution; // 路径声明使用 DLSS。
            status.target_camera_count = 1; // 至少一台目标相机真实存在。
        }
        app.update(); // 应用专用 37% CAS。
        assert_eq!(
            app.world()
                .get::<ContrastAdaptiveSharpening>(camera)
                .unwrap()
                .sharpening_strength,
            0.37
        ); // 百分比必须连续映射。
        app.world_mut().resource_mut::<DlssRuntimeStatus>().path = crate::DlssRuntimePath::Raster; // 模拟运行失败同帧回退。
        app.update(); // 普通玩家高档应恢复且持久值未被改写。
        assert_eq!(
            app.world()
                .get::<ContrastAdaptiveSharpening>(camera)
                .unwrap()
                .sharpening_strength,
            0.60
        ); // 恢复最新普通 CAS 选择。
    }
}
