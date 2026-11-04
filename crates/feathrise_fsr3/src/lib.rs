//! AMD FSR 3 时间域超分的 Bevy 0.19 RenderApp 隔离层。
//!
//! 【功能设计】本 crate 只负责相机输入、固定内部尺寸、逐视图历史和原子 HDR 输出；设置、
//! 厂商互斥与普通画质恢复由 `feathrise_settings` 的运行时租约统一管理。

mod debug; // AMD 调试视图下提供非持久 HUD 截图显隐控制。
mod extract; // 主世界只提取合格 FSR 相机及同帧相机参数。
mod history; // 相机瞬移、切镜与投影变化的一次性历史重置。
mod masks; // main opaque 快照与 GPU 自动 Reactive / T&C 遮罩。
mod node; // EarlyPostProcess 中录制 FSR 计算并原子发布备用主纹理。
mod prepare; // 只在模式或输出尺寸变化时创建逐视图 GPU 资源。
mod types; // 组件、资源、质量档和遥测契约集中定义。

pub use types::{
    Fsr3BackendSupported, Fsr3Quality, Fsr3RuntimeTelemetry, Fsr3RuntimeTelemetrySnapshot,
    Fsr3Upscaling,
}; // 设置与 UI 只消费稳定公开契约，不接触底层 wgpu-ffx 对象。

use bevy::{
    camera::visibility::VisibilitySystems, // HUD 诊断显隐须赶在本帧可见性传播前完成。
    core_pipeline::{
        core_3d::{main_opaque_pass_3d, main_transparent_pass_3d}, // 自动遮罩必须精确截取透明合成前后的 HDR。
        schedule::{Core3d, Core3dSystems}, // FSR 必须在主场景后、色调映射前执行。
    },
    pbr::main_transmissive_pass_3d, // 透射材质也属于透明合成侧，不能进入不透明底图。
    prelude::*,                     // App、Plugin 与 PostUpdate 常用类型。
    render::{
        ExtractSchedule,
        Render,
        RenderApp,
        RenderSystems,               // RenderApp 提取与视图准备调度。
        renderer::RenderAdapterInfo, // 只在主世界发布真实 Vulkan 后端资格。
        view::prepare_view_targets,  // FSR 内部尺寸必须早于主纹理创建确定。
    },
};
use types::{
    Fsr3DebugView, Fsr3PipelineContext, Fsr3PipelineInitialization,
    Fsr3RuntimeTelemetryFeatureStatus,
}; // RenderApp 共享管线、诊断开关、初始化门和低频状态。

/// 显式存在时启用 AMD FSR 3×3 调试输出；只面向本地定向排错。
const FSR3_DEBUG_VIEW_ENV: &str = "BEVY_FSR3_DEBUG_VIEW";

/// 安装 FSR 3 时间域超分候选；关闭或被互斥阻断时不创建任何 FSR 驱动管线。
#[derive(Default)]
pub struct Fsr3Plugin; // 无配置字段；玩家请求全部来自统一 RenderSettings。

impl Plugin for Fsr3Plugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Fsr3Upscaling>().add_systems(
            PostUpdate,
            (
                history::reset_fsr3_history_on_camera_discontinuity,
                debug::toggle_fsr3_debug_hud.before(VisibilitySystems::VisibilityPropagate),
            ),
        ); // 主世界只维护有限相机姿态，不分配 GPU 资源。
    }

    fn finish(&self, app: &mut App) {
        let telemetry = Fsr3RuntimeTelemetry::default(); // 主/渲染世界共享低频创建和失败快照。
        app.insert_resource(telemetry.clone()); // 设置页无需跨世界通道即可读取最终结果。
        let debug_view = Fsr3DebugView(std::env::var_os(FSR3_DEBUG_VIEW_ENV).is_some()); // 启动时读取一次，避免帧路径访问环境变量。
        app.insert_resource(debug_view); // 主世界只用它限定 F10 诊断 HUD 快捷键的生效范围。
        app.init_resource::<debug::Fsr3DebugHudVisibility>(); // 非持久截图状态每次进程从“显示”开始。

        let backend = app.get_sub_app_mut(RenderApp).map(|render_app| {
            render_app.insert_resource(telemetry.clone()); // 节点只在状态真实变化时更新共享快照。
            render_app.insert_resource(debug_view); // 默认 false；显式环境变量只改变本次进程的最终呈现。
            render_app.world().resource::<RenderAdapterInfo>().backend // 不按显卡品牌猜能力。
        });
        let Some(backend) = backend else {
            telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
                failure: Some("Bevy RenderApp 未创建，FSR 3 候选保持关闭".into()),
                ..Default::default()
            });
            return; // 纯逻辑或无渲染测试不得因可选候选插件 panic。
        };
        if backend != wgpu::Backend::Vulkan {
            telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
                failure: Some(format!("当前渲染后端为 {backend:?}，候选仅启用 Vulkan")),
                ..Default::default()
            }); // 非 Vulkan 构建保留原生画面并向 UI 给出明确原因。
            return;
        }

        app.insert_resource(Fsr3BackendSupported); // 设置系统可挂接请求，但不能提前宣称共享管线就绪。
        app.sub_app_mut(RenderApp)
            .init_resource::<Fsr3PipelineInitialization>()
            .add_systems(ExtractSchedule, extract::extract_fsr3_views)
            .add_systems(
                Render,
                (
                    prepare::initialize_fsr3_pipeline,
                    ApplyDeferred,
                    prepare::prepare_fsr3_views,
                )
                    .chain() // 仅首个真实请求创建管线，并让同帧逐视图准备看见新资源。
                    .in_set(RenderSystems::PrepareViews)
                    .before(prepare_view_targets),
            )
            .add_systems(
                Core3d,
                (
                    masks::copy_fsr3_opaque_color
                        .run_if(resource_exists::<Fsr3PipelineContext>)
                        .after(main_opaque_pass_3d)
                        .before(main_transmissive_pass_3d)
                        .before(main_transparent_pass_3d)
                        .in_set(Core3dSystems::MainPass),
                    node::run_fsr3_upscaling
                        .run_if(resource_exists::<Fsr3PipelineContext>)
                        .in_set(Core3dSystems::EarlyPostProcess),
                ),
            ); // FSR 在原生分辨率 UI 与色调映射前完成 HDR 重建。
        if debug_view.0 {
            warn!(
                "已通过 {FSR3_DEBUG_VIEW_ENV} 启用 AMD FSR 诊断视图；按 F10 可隐藏/恢复 Bevy HUD，该状态不会写入 settings.json"
            ); // 每次启动只记录一次，避免把诊断画面误认为正常输出。
        }
        info!("FSR 3 Vulkan 后端资格已确认；共享管线将在首次有效请求时延迟创建");
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 验证可选插件在无 RenderApp 的纯逻辑进程中只发布阻断。

    #[test]
    fn missing_render_app_degrades_without_panicking() {
        let mut app = App::new();
        app.add_plugins(Fsr3Plugin);
        app.finish();

        let snapshot = app.world().resource::<Fsr3RuntimeTelemetry>().snapshot();
        assert!(
            snapshot
                .status
                .failure
                .as_deref()
                .is_some_and(|detail| detail.contains("RenderApp 未创建"))
        );
        assert!(!app.world().contains_resource::<Fsr3BackendSupported>());
        assert!(!snapshot.pipeline_ready);
    }
}
