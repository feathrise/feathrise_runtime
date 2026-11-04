//! FSR 3 EarlyPostProcess 计算节点与原子 HDR 输出发布。

use crate::{
    masks,                         // 透明合成前后快照、两帧颜色历史与自动 Reactive / T&C 计算。
    prepare::fsr3_dispatch_jitter, // 把 Bevy 投影参数转换为真实屏幕像素位移。
    types::{
        ExtractedFsr3View, Fsr3DebugView, Fsr3PipelineContext, Fsr3RenderContext,
        Fsr3RuntimeFailure, Fsr3RuntimeTelemetry, Fsr3RuntimeTelemetryFeatureStatus,
    }, // 节点输入、逐视图历史、失败锁和遥测。
};
use bevy::{
    camera::MainPassResolutionOverride, // 当前低分辨率有效区域。
    core_pipeline::prepass::ViewPrepassTextures, // Depth32Float 与 Rg16Float 输入。
    prelude::*,                         // Commands、Res 和 UVec2。
    render::{
        camera::{ExtractedCamera, TemporalJitter}, // 当前相机预曝光与投影 Halton 偏移。
        diagnostic::RecordDiagnostics,             // FSR GPU 时间诊断标签。
        render_resource::{
            Buffer, Extent3d, TexelCopyTextureInfo, Texture, TextureAspect, TextureFormat,
            TextureUsages,
        }, // Bevy GPU 包装、紧凑颜色复制和纹理契约验证。
        renderer::{RenderContext, ViewQuery},      // 每视图 Core3d 系统入口。
        view::ViewTarget,                          // HDR 当前/备用纹理与原子双缓冲翻转。
    },
};
use std::{
    ops::Deref,             // 从 Bevy GPU 包装克隆同版本 wgpu 句柄。
    sync::atomic::Ordering, // 每个上下文只在首帧成功时发布低频遥测。
};
use wgpu_ffx::{FsrDispatchFlags, FsrDispatchInfo}; // 上游逐帧计算参数。

/// 录制一台当前视图的完整 FSR 3 计算链；成功后才发布备用 HDR 主纹理。
pub(crate) fn run_fsr3_upscaling(
    view: ViewQuery<(
        &ExtractedFsr3View,
        &ExtractedCamera,
        &Fsr3RenderContext,
        &MainPassResolutionOverride,
        &TemporalJitter,
        &ViewTarget,
        &ViewPrepassTextures,
    )>,
    pipeline: Res<Fsr3PipelineContext>, // 运行条件已保证共享管线存在。
    debug_view: Res<Fsr3DebugView>,     // 启动时冻结的非持久 AMD 诊断输出请求。
    telemetry: Res<Fsr3RuntimeTelemetry>, // 只在失败时写入低频状态。
    mut commands: Commands,             // dispatch 失败后移除 override 并建立稳定失败锁。
    mut ctx: RenderContext,             // 当前相机命令编码器。
) {
    let entity = view.entity(); // 失败组件必须写回同一渲染实体。
    let (
        extracted,
        extracted_camera,
        render_context,
        resolution_override,
        jitter,
        view_target,
        prepass,
    ) = view.into_inner();
    let render_size = resolution_override.0; // 与主场景 viewport 完全一致。
    let output_size = render_context.output_resolution; // 与备用 HDR 主纹理物理尺寸一致。

    let (Some(depth), Some(motion_vectors)) = (&prepass.depth, &prepass.motion_vectors) else {
        fail_view(
            entity,
            extracted,
            output_size,
            "FSR 3 缺少深度或运动矢量预通道".into(),
            &telemetry,
            &mut commands,
        ); // 不尝试绑定空输入，也不翻转 ViewTarget。
        return;
    };
    let color = view_target.main_texture(); // 当前 HDR 主颜色仍是后续节点的安全回退源。
    let output = view_target.main_texture_other(); // 备用主纹理只在成功后成为当前纹理。
    let depth_texture = &depth.texture.texture; // Bevy CachedTexture 的真实单采样深度。
    let motion_texture = &motion_vectors.texture.texture; // Bevy 当前运动矢量附件。
    if let Err(detail) = validate_texture_contract(
        color,
        &render_context.input_color,
        output,
        depth_texture,
        motion_texture,
        &render_context.dilated_depth,
        &render_context.dilated_motion_vectors,
        render_size,
        output_size,
    ) {
        fail_view(
            entity,
            extracted,
            output_size,
            detail,
            &telemetry,
            &mut commands,
        ); // 格式不匹配在录制前原子失败。
        return;
    }
    if let Err(detail) = masks::validate_mask_contract(
        &render_context.masks,
        &render_context.input_color,
        motion_texture,
        render_size,
        pipeline.masks.format,
    ) {
        fail_view(
            entity,
            extracted,
            output_size,
            detail,
            &telemetry,
            &mut commands,
        );
        return; // 自动遮罩 ABI 不完整时不向 FSR 提交零遮罩冒充成功。
    }

    let mut fsr_view = match render_context.view.lock() {
        Ok(value) => value,
        Err(_) => {
            fail_view(
                entity,
                extracted,
                output_size,
                "FSR 3 时间历史锁已损坏".into(),
                &telemetry,
                &mut commands,
            );
            return; // 锁异常不得 panic 或继续使用可能不一致的历史。
        }
    };
    ctx.command_encoder().copy_texture_to_texture(
        TexelCopyTextureInfo {
            texture: color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyTextureInfo {
            texture: &render_context.input_color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        Extent3d {
            width: render_size.x,
            height: render_size.y,
            depth_or_array_layers: 1,
        },
    ); // 纯GPU复制有效左上区域；无回读/等待，并让AMD归一化采样严格覆盖低分辨率画面。
    let render_device = ctx.render_device().clone(); // O(1) 设备句柄克隆，避免与命令编码器可变借用冲突。
    if let Err(detail) = masks::record_fsr3_masks(
        &pipeline.masks,
        &render_context.masks,
        &render_context.input_color,
        motion_texture,
        render_size,
        extracted.settings.reset,
        &render_device,
        ctx.command_encoder(),
    ) {
        fail_view(
            entity,
            extracted,
            output_size,
            detail,
            &telemetry,
            &mut commands,
        );
        return; // 只留下无害的紧凑颜色复制，不翻转 HDR 输出或推进 FSR 历史。
    }
    let sharpening = if extracted.settings.sharpening.is_finite() {
        extracted.settings.sharpening.clamp(0.0, 1.0) // 外部设置再次防御钳制。
    } else {
        0.0 // 非法反射写入安全关闭 RCAS，不能把 NaN 注入常量缓冲。
    };
    let pre_exposure = if extracted_camera.exposure.is_finite() && extracted_camera.exposure > 0.0 {
        extracted_camera.exposure // 匹配 Bevy 写入 HDR 主颜色的物理曝光倍率。
    } else {
        1.0 // 非法外部 Exposure 不得把 NaN/Infinity 注入时间历史。
    };
    let dispatch = FsrDispatchInfo {
        color: clone_raw_texture(&render_context.input_color), // 不能直接绑定Bevy的全尺寸viewport载体。
        depth: clone_raw_texture(depth_texture),
        motion_vectors: clone_raw_texture(motion_texture),
        exposure: None, // AUTO_EXPOSURE 标志要求不提供外部曝光纹理。
        reactive_mask: Some(clone_raw_texture(&render_context.masks.reactive_mask)), // 应用侧 GPU 自动遮罩只通过红通道传给固定上游。
        transparency_and_composition: Some(clone_raw_texture(
            &render_context.masks.transparency_and_composition_mask,
        )), // T&C 使用同帧透明贡献和运动重投影的两帧变化。
        dilated_depth: clone_raw_texture(&render_context.dilated_depth),
        dilated_motion_vectors: clone_raw_texture(&render_context.dilated_motion_vectors),
        reconstructed_previous_depth: clone_raw_buffer(
            &render_context.reconstructed_previous_depth,
        ),
        output: clone_raw_texture(output),
        jitter_offset: fsr3_dispatch_jitter(jitter), // 必须等于透视除法后的像素位移，避免静止和运动细节均因抖动错位而模糊。
        motion_vector_scale: [-(render_size.x as f32), -(render_size.y as f32)], // 对齐 Bevy/NVIDIA 路径的低分辨率 NDC 方向。
        render_size: render_size.to_array(),
        upscale_size: output_size.to_array(),
        enable_sharpening: sharpening > 0.0,
        sharpness: sharpening,
        frame_time_delta: extracted.frame_time_ms,
        pre_exposure,
        reset_history: extracted.settings.reset,
        camera_near: extracted.camera_near,
        camera_far: extracted.camera_far,
        camera_fov_y: extracted.camera_fov_y,
        view_space_to_meters_factor: 1.0, // 项目世界单位固定按一米解释。
        flags: if debug_view.0 {
            FsrDispatchFlags::DRAW_DEBUG_VIEW // 显式诊断进程显示 AMD 运动、深度和历史可信度网格。
        } else {
            FsrDispatchFlags::empty() // 正常进程保持完整 FSR 输出，不增加 debug pass。
        },
    };

    let diagnostics = ctx.diagnostic_recorder(); // 与其他 Bevy 后处理共用 GPU 时间记录器。
    let diagnostics = diagnostics.as_deref();
    let time_span = diagnostics.time_span(ctx.command_encoder(), "fsr3_upscaling");
    if let Err(error) = pipeline
        .upscaler
        .dispatch(&mut fsr_view, ctx.command_encoder(), &dispatch)
    {
        let detail = error.to_string(); // 上游参数校验保证错误发生在部分命令录制前。
        fail_view(
            entity,
            extracted,
            output_size,
            detail,
            &telemetry,
            &mut commands,
        );
        return; // 当前 HDR 主纹理保持未翻转，下一帧恢复原生尺寸。
    }
    let _committed_write = view_target.post_process_write(); // 完整 dispatch 成功后才原子发布备用纹理。
    time_span.end(ctx.command_encoder()); // GPU 诊断覆盖全部 FSR 计算 pass。
    if !render_context
        .dispatch_confirmed
        .swap(true, Ordering::Relaxed)
    {
        telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
            quality: Some(render_context.quality),
            output_resolution: output_size,
            internal_resolution: render_context.internal_resolution,
            failure: None,
        }); // 只有真实首帧命令录制并发布HDR输出后，主世界才宣称该档位实际生效。
        info!(
            "FSR 3 首帧超分已成功录制：{}×{} -> {}×{}",
            render_context.internal_resolution.x,
            render_context.internal_resolution.y,
            output_size.x,
            output_size.y
        ); // 每次模式或尺寸换代最多一条，便于实机矩阵核对真实dispatch。
    }
}

/// 在录制前验证 Bevy 0.19 的全尺寸附件与固定 FSR 着色器的纹理 ABI。
#[allow(clippy::too_many_arguments)] // 每项分别对应一个不可互换的外部绑定或辅助输出。
fn validate_texture_contract(
    source_color: &Texture,
    compact_color: &Texture,
    output: &Texture,
    depth: &Texture,
    motion_vectors: &Texture,
    dilated_depth: &Texture,
    dilated_motion_vectors: &Texture,
    render_size: UVec2,
    output_size: UVec2,
) -> Result<(), String> {
    if source_color.format() != TextureFormat::Rgba16Float
        || compact_color.format() != TextureFormat::Rgba16Float
        || output.format() != TextureFormat::Rgba16Float
    {
        return Err("FSR 3 需要 Rgba16Float HDR 输入和输出".into()); // LDR 或自定义纹理相机不进入候选。
    }
    if depth.format() != TextureFormat::Depth32Float {
        return Err(format!(
            "FSR 3 需要 Depth32Float，实际为 {:?}",
            depth.format()
        )); // 深度 ABI 不猜测转换。
    }
    if motion_vectors.format() != TextureFormat::Rg16Float {
        return Err(format!(
            "FSR 3 需要 Rg16Float 运动矢量，实际为 {:?}",
            motion_vectors.format()
        )); // 不用错误的纹理格式污染时间历史。
    }
    if dilated_depth.format() != TextureFormat::R32Float
        || dilated_motion_vectors.format() != TextureFormat::Rg16Float
    {
        return Err("FSR 3 膨胀深度或运动矢量辅助输出格式不匹配".into());
    }
    let extent = |texture: &Texture| UVec2::new(texture.width(), texture.height());
    let source_extent = extent(source_color);
    if source_extent.x < render_size.x || source_extent.y < render_size.y {
        return Err(format!(
            "FSR 3 主颜色有效区域 {render_size} 超出实际纹理 {source_extent}"
        )); // 复制范围必须在录制命令前完整验证。
    }
    for (label, actual) in [
        ("紧凑颜色", extent(compact_color)),
        ("膨胀深度", extent(dilated_depth)),
        ("膨胀运动矢量", extent(dilated_motion_vectors)),
    ] {
        if actual != render_size {
            return Err(format!(
                "FSR 3 {label}纹理尺寸应为 {render_size}，实际为 {actual}"
            )); // AMD内部归一化UV按MaxRenderSize解释这些资源。
        }
    }
    for (label, actual) in [
        ("深度", extent(depth)),
        ("运动矢量", extent(motion_vectors)),
    ] {
        if actual.x < render_size.x || actual.y < render_size.y {
            return Err(format!(
                "FSR 3 {label}有效区域 {render_size} 超出实际纹理 {actual}"
            )); // Bevy全尺寸预通道可直接按AMD整数像素坐标读取左上有效区。
        }
    }
    let actual_output = extent(output);
    if actual_output != output_size {
        return Err(format!(
            "FSR 3 输出纹理尺寸应为 {output_size}，实际为 {actual_output}"
        ));
    }
    if !source_color.usage().contains(TextureUsages::COPY_SRC)
        || !compact_color.usage().contains(
            TextureUsages::COPY_DST | TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING,
        )
    {
        return Err(
            "FSR 3 紧凑颜色复制/遮罩历史缺少 COPY_SRC/COPY_DST/TEXTURE_BINDING 用途".into(),
        );
    }
    Ok(())
}

/// 把单视图失败转为稳定锁和共享遥测，且不翻转 HDR 双缓冲。
fn fail_view(
    entity: Entity,
    extracted: &ExtractedFsr3View,
    output_resolution: UVec2,
    detail: String,
    telemetry: &Fsr3RuntimeTelemetry,
    commands: &mut Commands,
) {
    commands
        .entity(entity)
        .remove::<(Fsr3RenderContext, MainPassResolutionOverride)>()
        .insert(Fsr3RuntimeFailure {
            quality: extracted.settings.quality,
            output_resolution,
        }); // 相同模式和尺寸的稳定帧不再尝试。
    telemetry.record_status(Fsr3RuntimeTelemetryFeatureStatus {
        quality: Some(extracted.settings.quality),
        output_resolution,
        internal_resolution: UVec2::ZERO,
        failure: Some(detail.clone()),
    }); // 主世界下一帧释放画质租约并显示明确阻断。
    warn!("FSR 3 dispatch 失败并安全回退：{detail}"); // 每个失败键只输出一次。
}

/// O(1) 克隆 Bevy 包装中的共享 wgpu 纹理句柄。
fn clone_raw_texture(texture: &Texture) -> wgpu::Texture {
    texture.deref().clone() // 句柄克隆不复制像素也不触发 GPU 同步。
}

/// O(1) 克隆 Bevy 包装中的共享 wgpu 缓冲句柄。
fn clone_raw_buffer(buffer: &Buffer) -> wgpu::Buffer {
    buffer.deref().clone() // 句柄克隆不复制历史数据也不触发 CPU 回读。
}
