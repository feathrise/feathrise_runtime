//! 轻量 FPS 文本系统。
//!
//! F3 资源监控已经迁入独立功能域；这里仅保留右上角 FPS 文本，避免基础显示和
//! 大型诊断面板继续共享依赖与职责。

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use feathrise_settings::RenderSettings;

use super::components::FpsText;
use super::resources::{FpsDisplayFont, FpsRefreshTimer};

/// 创建右上角常驻的轻量 FPS 文本。
pub(super) fn spawn_fps_text(mut commands: Commands, font: Res<FpsDisplayFont>) {
    commands.spawn((
        FpsText,              // 标记实体供刷新与显隐系统查询。
        Text::new("FPS: --"), // 首份诊断样本产生前显示占位值。
        TextFont {
            font: font.0.clone().into(), // 与F3共用消费应用提供的字体，不主动加载原游戏资产。
            font_size: 14.0.into(), // 保持原有轻量字号与界面占用。
            ..default()             // 其余字体属性采用 Bevy 默认值。
        },
        TextColor(Color::srgba(0.9, 0.9, 0.3, 0.7)), // 使用原有半透明黄色诊断色。
        TextShadow {
            offset: Vec2::new(1.0, 1.0), // 一像素阴影保证亮场景中的可读性。
            color: Color::srgba(0.0, 0.0, 0.0, 0.8), // 深色阴影提升文字边缘对比度。
        },
        Node {
            position_type: PositionType::Absolute, // 脱离普通 UI 布局固定在视口角落。
            top: Val::Px(5.0),                     // 保留距顶部五像素的原布局。
            right: Val::Px(10.0),                  // 保留距右侧十像素的原布局。
            ..default()                            // 不额外占用布局空间。
        },
        FocusPolicy::Pass, // 诊断文字不截获鼠标或键盘交互。
        GlobalZIndex(100), // 位于普通 HUD 上层但低于 F3 资源监控面板。
    ));
}

/// 按固定低频刷新 FPS，并同步设置中的显隐开关。
pub(super) fn update_fps_text(
    time: Res<Time<Real>>, // 暂停菜单和游戏时间缩放不影响真实呈现计时。
    mut timer: ResMut<FpsRefreshTimer>, // 独立计时器避免每帧格式化文本。
    diagnostics: Res<DiagnosticsStore>, // 从 Bevy 帧诊断读取平滑 FPS。
    mut fps_text_q: Query<(Entity, &mut Text, &mut Visibility), With<FpsText>>, // 同步文字与可见性。
    settings: Option<Res<RenderSettings>>, // 设置插件缺失时保持默认显示，便于独立使用。
    mut display_cache: Local<Option<(Entity, Option<i64>)>>, // 区分有效整数与等待采样，避免残留旧数字。
    #[cfg(feature = "fsr3_frame_generation")] // 默认构建不依赖候选运行状态。
    fsr_status: Option<
        Res<feathrise_fsr3_frame_generation::Fsr3FrameGenerationRuntimeStatus>,
    >, // 读取本帧已经发布的累计Present。
    #[cfg(feature = "dlss_frame_generation")] // 仅DLSS构建读取Streamline主世界快照。
    dlss_status: Option<Res<feathrise_frame_generation::FrameGenerationRuntimeStatus>>, // 启动未选DLSS时资源不存在，继续使用有效后端或基础FPS。
    #[cfg(feature = "dlss_frame_generation")] // DLSS关闭但代理仍在时，尺寸变化也必须重建统计区间。
    primary_window: Query<&Window, With<bevy::window::PrimaryWindow>>, // 唯一主窗口的物理尺寸补足原生epoch身份。
    #[cfg(any(feature = "dlss_frame_generation", feature = "fsr3_frame_generation"))]
    // 每个显示系统保存一个固定大小采样器。
    mut present_sample: Local<super::resources::PresentationFpsSample>, // 不分配历史数组、不向GPU发起查询。
) {
    let show = settings.as_ref().is_none_or(|settings| settings.show_fps); // 缺少设置资源时兼容原有默认行为。
    let target_visibility = if show {
        Visibility::Visible // 设置允许时显示右上角 FPS。
    } else {
        Visibility::Hidden // 设置关闭时隐藏但保留实体和计时器状态。
    };
    for (_, _, mut visibility) in fps_text_q.iter_mut() {
        if *visibility != target_visibility {
            *visibility = target_visibility;
        }
    }

    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed()); // 无可信帧生成代理时保持原有基础帧率。
    #[cfg(any(feature = "dlss_frame_generation", feature = "fsr3_frame_generation"))]
    // 仅存在真实代理计数时替换来源，不能按设置枚举猜测倍率。
    let fps = {
        // 隐藏FPS期间也进行常量次采样，重新显示不混入旧计数。
        use super::types::{PresentationBackend, PresentationCounter, PresentationIdentity}; // 两个SDK共用采样器，只保留计数来源差异。
        let source: Option<PresentationCounter> = None; // 默认无代理；按编译能力读取已有快照。
        #[cfg(feature = "fsr3_frame_generation")] // 保留FSR原有有效代理判定，不改变候选行为。
        let source = fsr_status
            .as_deref()
            .filter(|status| status.device_attached && status.swapchain_ready) // 临时不生成时仍统计代理实际普通Present。
            .map(|status| PresentationCounter {
                // 将已有FSR快照转为共享计数输入。
                identity: PresentationIdentity {
                    // FSR原有计数回零与尺寸变化继续触发基线重置。
                    backend: PresentationBackend::Fsr, // 明确SDK来源，禁止与DLSS计数相减。
                    epoch: 0, // FSR尚无独立代次字段，保持原有重建检测口径。
                    extent: (status.display_width, status.display_height), // 使用真实输出尺寸。
                }, // FSR身份结束。
                count: status.total_present_count, // 使用SDK事实，不使用frames_presented倍率。
            })
            .or(source); // 资源不存在或代理失效时不制造呈现速率。
        #[cfg(feature = "dlss_frame_generation")]
        // 启动选择DLSS时才存在有效资源，两后端不会同时拥有交换链。
        let source = dlss_status
            .as_deref()
            .filter(|status| status.presentation_ready) // SDK状态查询成功且原生Present可采信才使用计数。
            .zip(primary_window.single().ok()) // 主窗口缺失或身份不唯一时，不沿用旧呈现来源。
            .filter(|(_, window)| window.physical_width() > 0 && window.physical_height() > 0) // 零尺寸窗口没有有效可采样输出。
            .map(|(status, window)| PresentationCounter {
                // DLSS可报告1帧或2帧，均由真实总数计算。
                identity: PresentationIdentity {
                    // 同尺寸重建也由原生层发布的epoch隔离。
                    backend: PresentationBackend::Dlss, // 后端切换撤销旧SDK的半秒区间。
                    epoch: status.presentation_epoch,   // 交换链或有效性换代不能跨区间取差值。
                    extent: (window.physical_width(), window.physical_height()), // 关闭FG后的普通交换链缩放也不能跨尺寸取差值。
                }, // DLSS身份结束。
                count: status.total_present_count, // 不将基础FPS乘2，也不以最近一次Present代替持续采样。
            })
            .or(source); // 无可信DLSS快照时继续已有有效FSR来源或原诊断。
        let measured = super::helpers::sample_present_fps(
            &mut present_sample,
            time.elapsed_secs_f64(),
            source,
        ); // 每帧检测计数重置，半秒计算一次速率。
        if source.is_some() { measured } else { fps } // 有代理但尚未收齐样本时显示--，不混入基础FPS。
    }; // FPS来源选择结束。
    timer.0.tick(time.delta()); // 无论显隐都推进真实时间计时器，暂停菜单仍可更新。
    if !timer.0.just_finished() || !show {
        return; // 尚未到刷新周期或当前隐藏时跳过诊断查询和字符串分配。
    }

    for (entity, mut text, _) in fps_text_q.iter_mut() {
        let rounded_fps = fps
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value.round() as i64); // 拒绝异常值，保留合法的0FPS。
        let key = (entity, rounded_fps); // 等待采样也参与缓存身份。
        if display_cache.as_ref() != Some(&key) {
            // 整数或有效性变化时才更新文本。
            **text = rounded_fps.map_or_else(|| "FPS: --".into(), |value| format!("FPS: {value}")); // 保留原布局，只改变真实统计来源。
            *display_cache = Some(key); // 稳定帧不重复分配字符串。
        } // 当前FPS文字同步结束。
    }
}
