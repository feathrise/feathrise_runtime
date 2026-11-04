//! 工程的窗口/任务池/设备参数；嵌入模式显式启用原作者视图所需设备能力。

use bevy::{
    app::{PluginGroupBuilder, TaskPoolThreadAssignmentPolicy},
    asset::AssetPlugin,
    prelude::*,
    render::{
        RenderPlugin,
        settings::{RenderCreation, WgpuSettings},
    },
    tasks::available_parallelism,
    window::{MonitorSelection, PresentMode, WindowMode, WindowPosition, WindowResolution},
};
use feathrise_settings::{StartupDisplayPreferences, StartupWindowPolicy};

fn pool_policy(threads: usize) -> TaskPoolThreadAssignmentPolicy {
    TaskPoolThreadAssignmentPolicy {
        min_threads: threads,
        max_threads: threads,
        percent: 1.0,
        on_thread_spawn: None,
        on_thread_destroy: None,
    }
}

fn task_pool_options() -> TaskPoolOptions {
    let threads = available_parallelism().max(1);
    let io = (threads.saturating_add(2) / 4).clamp(1, 4);
    let asynchronous = (threads.saturating_sub(io) / 2).max(1);
    let compute = threads
        .saturating_sub(io)
        .saturating_sub(asynchronous)
        .max(1);
    TaskPoolOptions {
        min_total_threads: threads,
        max_total_threads: threads,
        io: pool_policy(io),
        async_compute: pool_policy(asynchronous),
        compute: pool_policy(compute),
    }
}

fn render_settings(embedded: bool) -> WgpuSettings {
    let mut settings = WgpuSettings::default();
    if embedded {
        let author_features = bevy::render::render_resource::WgpuFeatures::POLYGON_MODE_LINE
            | bevy::render::render_resource::WgpuFeatures::IMMEDIATES;
        settings.features |= author_features;
        if let Some(disabled) = settings.disabled_features.as_mut() { disabled.remove(author_features); }
    }
    #[cfg(any(
        feature = "raytracing",
        feature = "rtx_rendering",
        feature = "fsr3_upscaling",
        feature = "fsr3_frame_generation"
    ))]
    {
        settings.backends = Some(bevy::render::settings::Backends::VULKAN);
    }
    #[cfg(all(debug_assertions, target_os = "windows"))]
    if std::env::var_os("WGPU_VALIDATION").is_none() {
        settings
            .instance_flags
            .remove(bevy::render::settings::InstanceFlags::VALIDATION);
    }
    settings
}

pub(super) fn default_plugins(
    policy: StartupWindowPolicy,
    display: StartupDisplayPreferences,
    assets: AssetPlugin,
    embedded: bool,
) -> PluginGroupBuilder {
    let mode = if !policy.force_windowed && display.fullscreen {
        WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
    } else {
        WindowMode::Windowed
    };
    let plugins = DefaultPlugins.build();
    // 编译好只形成候选；正式工程必须经过原管道的显式应用，不能由默认插件连接dx后立即换代码。
    #[cfg(feature = "function_hotpatch")]
    let plugins = plugins.disable::<bevy::app::hotpatch::HotPatchPlugin>();
    plugins
        .set(TaskPoolPlugin {
            task_pool_options: task_pool_options(),
        })
        .set(assets)
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "工程实例".into(),
                visible: !embedded,
                // 隐藏创建避免弹窗；装饰仍先保留工程原请求，由编辑租约临时移除。
                decorations: true,
                resolution: WindowResolution::new(1270, 720),
                position: WindowPosition::Centered(MonitorSelection::Primary),
                mode,
                present_mode: if display.vsync {
                    PresentMode::AutoVsync
                } else {
                    PresentMode::AutoNoVsync
                },
                ..default()
            }),
            // 独立预览的原生X/Alt+F4只请求返回编辑，窗口实体和World不能被默认系统销毁。
            close_when_requested: !embedded,
            ..default()
        })
        .set(RenderPlugin {
            render_creation: RenderCreation::Automatic(Box::new(render_settings(embedded))),
            ..default()
        })
}
