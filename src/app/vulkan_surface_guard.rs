//! Bevy 0.19 / wgpu 29 的 Windows 交换链呈现保护。
//!
//! Bevy 会为 Wayland 窗口设置 `needs_initial_present`，保证没有相机时也提交首个缓冲；
//! 该标记在 Windows 同样被初始化。NVIDIA Vulkan 驱动可能因此取得尚无生产写入的
//! surface texture，随后触发 `UNDEFINED` 布局呈现与 acquire semaphore 复用错误。
//! Windows 不需要 Wayland 的首次 buffer commit，因此在 `prepare_windows` 取得交换链
//! 图像前清除此标记；真正的窗口相机就绪后仍完整使用 Bevy 原生渲染和呈现路径。
//!
//! Bevy 0.19 的最终放大节点还有第二条首帧路径：它会先把窗口附件标记为已使用，再在
//! blit 管线尚未就绪时直接返回。保护系统紧跟 Core2d/Core3d 的原生放大节点，仅在对应
//! blit 管线未就绪时向同一命令缓冲补初始化 pass，避免单独提交和稳定帧额外 GPU 工作。
//!
//! GamePP 1.2.1615.625 的隐式 Vulkan 层还会在 NVIDIA 驱动调用内传入无效地址；入口
//! 按其 layer manifest 声明的开关，仅为当前游戏进程设置 `DISABLE_GAMEPP_LAYER=1`。

use bevy::{
    camera::{CameraOutputMode, ClearColor, ClearColorConfig},
    core_pipeline::{
        schedule::{Core2d, Core3d},
        upscaling::{ViewUpscalingPipeline, upscaling}, // 查询本视图真正使用的管线，包括UNORM窗口编码变体。
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        camera::ExtractedCamera,
        render_resource::{
            LoadOp,
            Operations,
            PipelineCache,
            RenderPassColorAttachment,
            RenderPassDescriptor,
            StoreOp, // 不再重建普通blit键，避免与实际编码管线身份错配。
        },
        renderer::{RenderContext, ViewQuery},
        view::ViewTarget,
        view::window::{ExtractedWindows, create_surfaces, prepare_windows},
    },
};

/// 在任何 Bevy 任务池或 Vulkan 实例创建前，按 GamePP 清单约定禁用其进程内隐式层。
pub(crate) fn disable_incompatible_gamepp_vulkan_layer() {
    // SAFETY: main在线程池和插件构造前调用一次；此后项目不会再读写该环境变量。
    unsafe { std::env::set_var("DISABLE_GAMEPP_LAYER", "1") };
}

/// 在 Windows RenderApp 中禁止仅 Wayland 需要的无内容首次呈现。
pub(crate) struct VulkanSurfaceGuardPlugin;

impl Plugin for VulkanSurfaceGuardPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return; // 无头测试或未安装RenderPlugin时无需交换链保护。
        };
        render_app.add_systems(
            Render,
            suppress_unrendered_initial_present
                .in_set(RenderSystems::PrepareViews) // 与Bevy取得窗口交换链图像的系统处于同一准备阶段。
                .after(create_surfaces) // 先允许Bevy创建和配置surface，再只调整首次呈现资格。
                .before(prepare_windows), // 必须在get_current_texture前取消无相机窗口的强制取得。
        );
        render_app.add_systems(
            Core2d,
            initialize_surface_when_upscaling_is_pending.after(upscaling), // 二维加载/菜单相机也会经过同一最终放大节点。
        );
        render_app.add_systems(
            Core3d,
            initialize_surface_when_upscaling_is_pending.after(upscaling), // 三维玩法相机使用相同的精确管线就绪门。
        );
    }
}

/// 清除新 Windows 窗口的 Wayland 首次提交标记，避免无相机帧提前取得并呈现交换链图像。
fn suppress_unrendered_initial_present(mut windows: ResMut<ExtractedWindows>) {
    for window in windows.values_mut() {
        if window.needs_initial_present {
            window.needs_initial_present = false; // 后续仅在真实窗口相机产生输出时取得和呈现surface。
        }
    }
}

/// 仅在当前视图的最终 blit 尚不可用时补一个同命令缓冲初始化 pass。
fn initialize_surface_when_upscaling_is_pending(
    view: ViewQuery<(
        &ViewTarget,
        Option<&ExtractedCamera>,
        &ViewUpscalingPipeline,
    )>, // 精确读取当前视图的编译身份。
    pipeline_cache: Res<PipelineCache>,
    clear_color_global: Res<ClearColor>,
    mut context: RenderContext,
) {
    let (target, camera, upscaling_pipeline) = view.into_inner(); // 编码策略与实际管线共同来自原生准备系统。
    if !target.needs_present() {
        return; // 纹理目标和未被原生放大节点触碰的窗口附件都不属于本保护范围。
    }
    if upscaling_pipeline.is_ready(&pipeline_cache) {
        // 不排队、不等待，也不会误检查不同shader定义的普通blit。
        return; // 正常路径已经由原生upscaling写入窗口，稳定帧不新增pass或submit。
    }

    let Some(output_view) = target.out_texture() else {
        return; // needs_present与窗口输出通常保证存在，此处仍防御设备恢复期资源变化。
    };
    let clear_color = match camera.map(|camera| camera.output_mode) {
        Some(CameraOutputMode::Write { clear_color, .. }) => match clear_color {
            ClearColorConfig::Default => clear_color_global.0,
            ClearColorConfig::Custom(color) => color,
            ClearColorConfig::None => Color::BLACK, // 未定义的新交换链内容不能执行Load，首帧以黑色安全初始化。
        },
        Some(CameraOutputMode::Skip) => return,
        None => clear_color_global.0,
    };

    let load = if camera.is_some_and(|camera| camera.sorted_camera_index_for_target > 0) {
        LoadOp::Load // 后续相机必须保留前序相机已经写入或初始化的同一窗口附件。
    } else {
        let values = if upscaling_pipeline.encodes_srgb() {
            // UNORM清除不会执行硬件编码，必须使用同一显示颜色数值。
            let encoded = clear_color.to_srgba(); // 与最终片元转换对应的sRGB传递函数。
            LinearRgba::new(encoded.red, encoded.green, encoded.blue, encoded.alpha) // 仅承载待写数值，禁止再次转换。
        } else {
            clear_color.to_linear()
        }; // 普通sRGB视图仍使用线性清除值。
        LoadOp::Clear(values.into()) // 首台相机确定性写入并生成Vulkan布局转换。
    };
    let _initialization_pass = context
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("windows_surface_pending_upscaling_guard"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: output_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load,
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }); // pass随作用域结束并进入本视图原有命令缓冲，不额外调用queue.submit。
}
