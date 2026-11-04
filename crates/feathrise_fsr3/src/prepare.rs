//! FSR 3 逐视图上下文、固定内部尺寸与 GPU 辅助资源准备。

use crate::{
    masks, // 共享自动遮罩管线与逐视图两帧颜色历史。
    types::{
        ExtractedFsr3View, Fsr3PipelineContext, Fsr3PipelineInitialization, Fsr3RenderContext,
        Fsr3RuntimeFailure, Fsr3RuntimeTelemetry, Fsr3RuntimeTelemetryFeatureStatus,
    }, // RenderApp 输入、上下文键和低频遥测。
};
use bevy::{
    camera::{Camera3d, CameraMainTextureUsages, MainPassResolutionOverride}, // 主颜色/深度用途与低分辨率主通道。
    core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass}, // 查询必须拥有两种 FSR 输入。
    diagnostic::FrameCount,                                      // 稳定 Halton 相位索引。
    prelude::*,                                                  // ECS Query、Commands 和 UVec2。
    render::{
        camera::{MipBias, TemporalJitter}, // 本帧投影抖动与材质纹理偏移。
        render_resource::{
            BufferDescriptor, BufferUsages, Extent3d, TextureDescriptor, TextureDimension,
            TextureFormat, TextureUsages,
        },
        renderer::{RenderAdapter, RenderDevice, RenderQueue}, // 实际格式能力、共享设备、队列和资源创建入口。
        view::ExtractedView,                                  // 最终窗口物理 viewport。
    },
};
use wgpu_ffx::{FsrContext, FsrContextFlags, FsrContextInfo}; // 固定上游 FSR 3 计算实现。

/// 首次存在真实 FSR 视图时才在 RenderApp 内创建共享管线。
pub(crate) fn initialize_fsr3_pipeline(
    mut commands: Commands, // 成功后通过同一 RenderApp 命令队列发布共享上下文。
    requesting_views: Query<(), With<ExtractedFsr3View>>, // 关闭或互斥时保持空查询和零驱动副作用。
    mut initialization: ResMut<Fsr3PipelineInitialization>, // 初始化成功或失败均只尝试一次。
    render_adapter: Res<RenderAdapter>, // R8 存储写入能力不能按显卡品牌推断。
    render_device: Res<RenderDevice>, // 在正常渲染调度中取得已经稳定运行的逻辑设备。
    telemetry: Res<Fsr3RuntimeTelemetry>, // 主世界下一帧读取就绪或失败结果。
) {
    if initialization.attempted || requesting_views.is_empty() {
        return; // 稳定帧和未请求状态完全绕过管线构造。
    }
    initialization.attempted = true; // 在进入上游前锁门，panic 也不会逐帧重试。

    let mask_format = match masks::choose_mask_format(&render_adapter, &render_device) {
        Ok(format) => format,
        Err(detail) => {
            telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
                failure: Some(detail.clone()),
                ..default()
            });
            error!("FSR 3 自动遮罩能力检查失败并保持原生渲染：{detail}");
            return; // 无合法遮罩存储格式时不创建半套候选管线。
        }
    };

    let flags = FsrContextFlags::HIGH_DYNAMIC_RANGE
        | FsrContextFlags::DEPTH_INVERTED
        | FsrContextFlags::DEPTH_INFINITE
        | FsrContextFlags::AUTO_EXPOSURE; // 对齐 Bevy HDR 与无限远反向 Z 投影。
    let device = render_device.wgpu_device().clone(); // O(1) 共享句柄，不复制 GPU 资源。
    let created = catch_unwind(AssertUnwindSafe(|| Fsr3PipelineContext {
        upscaler: FsrContext::new(FsrContextInfo { device, flags }),
        masks: masks::create_mask_pipeline(&render_device, mask_format),
    })); // 上游与遮罩构造器没有 Result；Rust panic 转为候选回退。
    match created {
        Ok(context) => {
            commands.insert_resource(context);
            telemetry.record_pipeline_ready();
            info!("FSR 3 共享计算管线与 GPU 自动遮罩已创建，遮罩格式：{mask_format:?}");
        }
        Err(payload) => {
            let detail = panic_detail(payload, "FSR 3 计算管线创建发生未知错误");
            telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
                failure: Some(detail.clone()),
                ..default()
            });
            error!("FSR 3 共享计算管线创建失败并保持原生渲染：{detail}");
        }
    }
}
use std::{
    ops::Deref, // 把 Bevy RenderQueue 安全借用为同版本 wgpu Queue。
    panic::{AssertUnwindSafe, catch_unwind}, // 上游 create_view 无 Result，失败转结构化回退。
};

/// 一台 RenderApp FSR 视图在准备阶段的完整可变附件集合。
type Fsr3PrepareViewData<'a> = (
    Entity,
    &'a ExtractedView,
    &'a ExtractedFsr3View,
    &'a mut Camera3d,
    &'a mut CameraMainTextureUsages,
    &'a mut TemporalJitter,
    &'a mut MipBias,
    Option<&'a Fsr3RenderContext>,
    Option<&'a Fsr3RuntimeFailure>,
);

/// 只有两项必需预通道均已提取的视图才能建立 FSR 上下文。
type Fsr3PrepareViewFilter = (With<DepthPrepass>, With<MotionVectorPrepass>);

/// 使用 Halton 序列驱动 Bevy 投影；屏幕空间符号在 dispatch 边界转换。
fn apply_fsr3_temporal_jitter(temporal_jitter: &mut TemporalJitter, jitter: [f32; 2]) {
    temporal_jitter.offset = Vec2::from_array(jitter); // Bevy 内部负责 `(2x, -2y) / renderSize` 投影换算。
}

/// 返回实际屏幕像素位移：Bevy 修改矩阵 Z 列，透视除法的 w=-z 使位移反号。
pub(crate) fn fsr3_dispatch_jitter(temporal_jitter: &TemporalJitter) -> [f32; 2] {
    (-temporal_jitter.offset).to_array() // AMD 从采样位置减去此值；同号提交会留下两倍抖动，使静止细节也无法对齐。
}

/// 为活动 FSR 视图准备固定内部尺寸；稳定模式和尺寸不重建任何 GPU 资源。
#[allow(clippy::too_many_arguments)] // 设备、队列、管线、遥测和视图查询分别承担独立职责。
pub(crate) fn prepare_fsr3_views(
    mut query: Query<Fsr3PrepareViewData<'_>, Fsr3PrepareViewFilter>, // 必需附件缺失时不错误创建半套上下文。
    pipeline: Option<Res<Fsr3PipelineContext>>, // 初始化失败时保持原生路径而不触发缺资源panic。
    render_device: Res<RenderDevice>,           // 创建逐视图纹理和缓冲。
    render_queue: Res<RenderQueue>,             // 初始化上游查找表和历史资源。
    telemetry: Res<Fsr3RuntimeTelemetry>,       // 向主世界报告实际内部尺寸或失败。
    frame_count: Res<FrameCount>,               // 同一视图使用确定性 Halton 相位。
    mut commands: Commands,                     // 只在键变化、恢复或失败时增删上下文。
) {
    let Some(pipeline) = pipeline else {
        return; // 首次请求的初始化失败已经通过遥测发布结构化阻断。
    };
    for (
        entity,
        extracted_view,
        extracted_fsr,
        mut camera_3d,
        mut main_texture_usages,
        mut temporal_jitter,
        mut mip_bias,
        context,
        failure,
    ) in &mut query
    {
        main_texture_usages.0 |=
            TextureUsages::STORAGE_BINDING | TextureUsages::COPY_DST | TextureUsages::COPY_SRC; // FSR 写备用 HDR，并把主通道有效区复制成紧凑颜色输入。
        let mut depth_usages = TextureUsages::from(camera_3d.depth_texture_usages); // 保留作者已有深度用途。
        depth_usages |= TextureUsages::TEXTURE_BINDING; // FSR 计算着色器必须采样 Depth32Float。
        camera_3d.depth_texture_usages = depth_usages.into(); // 只增加用途，不移除现有标志。

        let output_resolution = extracted_view.viewport.zw(); // 最终窗口 HDR 目标的真实物理尺寸。
        if output_resolution.x < 2 || output_resolution.y < 2 {
            commands
                .entity(entity)
                .remove::<(Fsr3RenderContext, MainPassResolutionOverride)>(); // 最小化时不保留无效 override。
            continue;
        }
        let quality = extracted_fsr.settings.quality; // 一帧只读取一次质量键。
        let internal_resolution = quality.render_size(output_resolution); // 固定而非动态内部尺寸。
        let phase_count = wgpu_ffx::get_jitter_phase_count(
            internal_resolution.x as i32,
            output_resolution.x as i32,
        ); // 相位数量随线性放大倍率平方增长。
        let jitter_index = (frame_count.0 % phase_count as u32) as i32; // 长时运行也不把u32帧号截成负数。
        let jitter = wgpu_ffx::get_jitter_offset(jitter_index, phase_count); // AMD 单位像素空间的 Halton 偏移。
        apply_fsr3_temporal_jitter(&mut temporal_jitter, jitter); // 保持同一相位，dispatch 再转换为实际屏幕像素位移。
        mip_bias.0 = quality.mip_bias(); // 低分辨率主通道选择更高细节 mip。

        if context.is_some_and(|current| {
            current.quality == quality
                && current.output_resolution == output_resolution
                && current.internal_resolution == internal_resolution
        }) {
            continue; // 稳定帧只更新抖动与常量，不重建历史、纹理或缓冲。
        }
        if failure.is_some_and(|failed| {
            failed.quality == quality && failed.output_resolution == output_resolution
        }) {
            continue; // 同一失败键等待玩家切档或窗口尺寸变化，不持续重试。
        }

        let created = catch_unwind(AssertUnwindSafe(|| {
            create_render_context(
                &pipeline,
                &render_device,
                render_queue.deref(),
                quality,
                internal_resolution,
                output_resolution,
            )
        })); // 资源分配或上游校验 panic 转为非致命候选阻断。
        match created {
            Ok(context) => {
                commands
                    .entity(entity)
                    .insert((context, MainPassResolutionOverride(internal_resolution))); // 同一命令批原子安装历史和低分辨率 viewport。
                commands.entity(entity).remove::<Fsr3RuntimeFailure>(); // 尺寸/模式变化后的成功恢复清旧锁。
            }
            Err(payload) => {
                let detail = panic_detail(payload, "FSR 3 GPU 资源创建发生未知错误"); // 不泄露任意类型。
                commands
                    .entity(entity)
                    .remove::<(Fsr3RenderContext, MainPassResolutionOverride)>()
                    .insert(Fsr3RuntimeFailure {
                        quality,
                        output_resolution,
                    }); // 保持原生尺寸路径并锁住相同失败键。
                telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
                    quality: Some(quality),
                    output_resolution,
                    internal_resolution: UVec2::ZERO,
                    failure: Some(detail.clone()),
                });
                warn!("FSR 3 逐视图资源创建失败并安全回退：{detail}"); // 每个失败键最多记录一次。
            }
        }
    }
}

/// 创建一台视图的全部时间历史和外部辅助输出。
fn create_render_context(
    pipeline: &Fsr3PipelineContext, // 共享分辨率无关管线。
    device: &RenderDevice,          // Bevy 与 FSR 共用逻辑设备。
    queue: &wgpu::Queue,            // 上游初始化内部 LUT 所需队列。
    quality: crate::Fsr3Quality,    // 稳定上下文比较键。
    internal_resolution: UVec2,     // 低分辨率颜色、深度和运动矢量有效区域。
    output_resolution: UVec2,       // 最终 HDR 输出尺寸。
) -> Fsr3RenderContext {
    let view = pipeline.upscaler.create_view(
        queue,
        internal_resolution.to_array(),
        output_resolution.to_array(),
    ); // 只在真实键变化时分配上游完整时间历史。
    let input_color = device.create_texture(&TextureDescriptor {
        label: Some("fsr3_compact_input_color"),
        size: Extent3d {
            width: internal_resolution.x,
            height: internal_resolution.y,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba16Float,
        usage: TextureUsages::COPY_DST | TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    }); // Bevy只缩小全尺寸主纹理的viewport；FSR归一化采样要求资源物理尺寸就是MaxRenderSize。
    let texture = |label: &'static str, format: TextureFormat| {
        device.create_texture(&TextureDescriptor {
            label: Some(label),
            size: Extent3d {
                width: internal_resolution.x,
                height: internal_resolution.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }) // 外部输出仅供 FSR 当前 dispatch 与后续候选效果读取。
    };
    let dilated_depth = texture("fsr3_dilated_depth", TextureFormat::R32Float); // 上游固定格式。
    let dilated_motion_vectors = texture("fsr3_dilated_motion_vectors", TextureFormat::Rg16Float); // 上游固定格式。
    let reconstructed_bytes = u64::from(internal_resolution.x)
        .saturating_mul(u64::from(internal_resolution.y))
        .saturating_mul(4); // 每个输入像素一个 32 位重建深度值。
    let reconstructed_previous_depth = device.create_buffer(&BufferDescriptor {
        label: Some("fsr3_reconstructed_previous_depth"),
        size: reconstructed_bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }); // 由 FSR 清空和写入，不产生 CPU 回读。
    let masks = masks::create_mask_resources(device, internal_resolution, pipeline.masks.format); // 只在模式/尺寸换代时分配两帧历史和两张遮罩。

    Fsr3RenderContext {
        view: std::sync::Mutex::new(view), // 相机节点独占推进时间历史。
        dispatch_confirmed: std::sync::atomic::AtomicBool::new(false), // 首帧HDR输出后才允许UI宣称实际生效。
        quality,
        output_resolution,
        internal_resolution,
        input_color,
        masks,
        dilated_depth,
        dilated_motion_vectors,
        reconstructed_previous_depth,
    }
}

/// 把上游无 Result 构造器的 panic payload 收敛为稳定诊断文本。
fn panic_detail(payload: Box<dyn std::any::Any + Send>, fallback: &'static str) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned(); // 常见静态 panic 消息。
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone(); // 动态格式化 panic 消息。
    }
    fallback.into() // 不可显示 payload 使用调用点对应的稳定回退文本。
}

#[cfg(test)]
mod tests {
    use super::*; // 使用真实投影验证屏幕位移，不创建 RenderApp 或 GPU 设备。
    use bevy::camera::CameraProjection; // 调用 Bevy 正式透视投影矩阵。

    #[test]
    fn fsr_dispatch_jitter_matches_projected_pixel_displacement() {
        for render_size in [Vec2::new(1280.0, 720.0), Vec2::new(640.0, 360.0)] {
            let projection = PerspectiveProjection::default().get_clip_from_view(); // Bevy 反向无限远右手透视。
            for phase in 0..18 {
                let mut temporal_jitter = TemporalJitter::default(); // 每相位独立应用正式抖动路径。
                apply_fsr3_temporal_jitter(
                    &mut temporal_jitter,                   // 写入与正式视图相同的投影组件。
                    wgpu_ffx::get_jitter_offset(phase, 18), // 覆盖完整 Quality 相位周期。
                ); // 覆盖非零 X/Y 和正负偏移。
                let mut jittered_projection = projection; // 从无抖动矩阵开始，避免累加相位。
                temporal_jitter.jitter_projection(&mut jittered_projection, render_size); // 与 Bevy prepare_view_uniforms 使用相同转换。
                for point in [Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.3, -0.2, -10.0)] {
                    let before = projection.project_point3(point); // 透视除法后的无抖动 NDC。
                    let after = jittered_projection.project_point3(point); // 透视除法后的实际 NDC。
                    let pixel_displacement =
                        (after - before).truncate() * Vec2::new(0.5, -0.5) * render_size; // 转为左上原点的输入像素位移。
                    let dispatch = Vec2::from_array(fsr3_dispatch_jitter(&temporal_jitter)); // AMD 用此偏移从采样位置消除抖动。
                    assert!(
                        (pixel_displacement - dispatch).abs().max_element() < 0.0001,
                        "phase={phase}, size={render_size:?}, actual={pixel_displacement:?}, dispatch={dispatch:?}"
                    ); // 旧实现会产生两倍偏移误差。
                }
            }
        }
    }
}
