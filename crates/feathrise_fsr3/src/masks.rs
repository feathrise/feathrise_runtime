//! FSR 3 反应遮罩与透明合成遮罩的 GPU 生成、历史和主通道阶段快照。

use crate::types::Fsr3RenderContext; // 不透明阶段复制只接管已经成功准备的 FSR 视图。
use bevy::{
    camera::MainPassResolutionOverride, // 复制范围必须与本帧低分辨率主通道完全一致。
    prelude::*,                         // UVec2 与 Res。
    render::{
        render_resource::{BindGroup, BindGroupLayout, ComputePipeline, Texture, TextureId}, // 持久管线、逐视图纹理与按运动附件缓存的绑定组。
        renderer::{RenderAdapter, RenderContext, RenderDevice, ViewQuery}, // 设备能力、资源创建和 Core3d 编码入口。
        view::ViewTarget, // 在透明物绘制前读取当前 HDR 主颜色。
    },
};
use std::{
    borrow::Cow, // WGSL 格式占位符替换后由着色器模块取得拥有型源码。
    sync::{
        Mutex,                          // 运动矢量附件变化时低频换代绑定组，不在稳定帧分配。
        atomic::{AtomicBool, Ordering}, // 新上下文与切镜只播种一次遮罩历史。
    },
};

/// 固定八乘八工作组，与 AMD 自动反应遮罩通道的二维派发粒度一致。
const MASK_WORKGROUP_SIZE: u32 = 8;
/// WGSL 中唯一允许被初始化代码替换的存储格式标记。
const MASK_FORMAT_TOKEN: &str = "__MASK_FORMAT__";
/// 应用侧自动遮罩着色器；不经 AssetServer，首次真实请求时才交给驱动编译。
const MASK_SHADER_TEMPLATE: &str = include_str!("shaders/generate_masks.wgsl");

/// 分辨率无关的自动遮罩计算管线。
pub(crate) struct Fsr3MaskPipeline {
    pub(crate) pipeline: ComputePipeline, // 每帧只录制一次固定二维 dispatch。
    pub(crate) layout: BindGroupLayout,   // 逐视图资源与运动附件共享同一绑定 ABI。
    pub(crate) format: wgpu::TextureFormat, // 优先 R8，必要时用 RGBA8 红通道保持跨厂商存储写入。
}

/// 一台视图持有的当前不透明快照、两帧历史与 FSR 外部遮罩。
pub(crate) struct Fsr3MaskResources {
    pub(crate) opaque_color: Texture, // main opaque 后、transmission/transparent 前的紧凑 HDR。
    pub(crate) previous_opaque_color: Texture, // 运动重投影读取的上一帧不透明底图。
    pub(crate) previous_composed_color: Texture, // 运动重投影读取的上一帧透明合成结果。
    pub(crate) reactive_mask: Texture, // FSR 用红通道降低不可信透明历史累积。
    pub(crate) transparency_and_composition_mask: Texture, // FSR 用红通道撤销锁并加强颜色钳制。
    cached_bind_group: Mutex<Option<CachedMaskBindGroup>>, // 稳定运动附件下不重复创建纹理视图或绑定组。
    history_seeded: AtomicBool, // 首帧和显式 reset 在计算前用当前颜色播种历史。
}

/// 运动矢量预通道附件可能在窗口/视图换代时改变，按 Bevy 纹理身份缓存绑定组。
struct CachedMaskBindGroup {
    motion_texture_id: TextureId, // 只比较 O(1) 资源身份，不读取任何像素。
    bind_group: BindGroup,        // 克隆仅复制共享 GPU 句柄。
}

/// 选择可写存储遮罩格式；R8 不可写时回退到保证更广的 RGBA8 红通道。
pub(crate) fn choose_mask_format(
    adapter: &RenderAdapter,
    device: &RenderDevice,
) -> Result<wgpu::TextureFormat, String> {
    for format in [
        wgpu::TextureFormat::R8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        if format == wgpu::TextureFormat::R8Unorm
            && !device
                .features()
                .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
        {
            continue; // 未向逻辑设备申请扩展格式能力时必须使用核心保证的 RGBA8。
        }
        let features = adapter.get_texture_format_features(format); // 查询实际适配器而非按品牌猜测。
        let usages = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        if features.allowed_usages.contains(usages)
            && features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY)
        {
            return Ok(format); // 两种格式交给 FSR 时都只采样归一化红通道。
        }
    }
    Err("当前适配器不支持可采样且可写的 FSR 自动遮罩纹理格式".into())
}

/// 在首个真实 FSR 请求中编译固定自动遮罩 WGSL 与显式绑定布局。
pub(crate) fn create_mask_pipeline(
    device: &RenderDevice,
    format: wgpu::TextureFormat,
) -> Fsr3MaskPipeline {
    let storage_format = match format {
        wgpu::TextureFormat::R8Unorm => "r8unorm",
        wgpu::TextureFormat::Rgba8Unorm => "rgba8unorm",
        _ => unreachable!("遮罩格式选择器只会返回两种已审计格式"),
    };
    let shader_source = MASK_SHADER_TEMPLATE.replace(MASK_FORMAT_TOKEN, storage_format); // 不维护两份易漂移的 WGSL。
    let shader = device.create_and_validate_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fsr3_generate_masks_shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_source)),
    }); // 静态项目着色器仍走 wgpu 完整 WGSL 校验。
    let sampled_texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let storage_texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(
        "fsr3_generate_masks_layout",
        &[
            sampled_texture(0), // 当前不透明 HDR。
            sampled_texture(1), // 当前透明合成后 HDR。
            sampled_texture(2), // 上一帧不透明 HDR。
            sampled_texture(3), // 上一帧透明合成后 HDR。
            sampled_texture(4), // Bevy Rg16Float 运动矢量。
            storage_texture(5), // 反应遮罩。
            storage_texture(6), // T&C 遮罩。
        ],
    );
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fsr3_generate_masks_pipeline_layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("fsr3_generate_masks_pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("generate_fsr3_masks"),
        compilation_options: Default::default(),
        cache: None,
    });
    Fsr3MaskPipeline {
        pipeline,
        layout,
        format,
    }
}

/// 按内部尺寸创建两帧颜色历史与两张外部遮罩；稳定帧全部复用。
pub(crate) fn create_mask_resources(
    device: &RenderDevice,
    internal_resolution: UVec2,
    mask_format: wgpu::TextureFormat,
) -> Fsr3MaskResources {
    let texture = |label, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: internal_resolution.x,
                height: internal_resolution.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color_usage = wgpu::TextureUsages::COPY_DST
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::TEXTURE_BINDING; // 当前快照既接收主通道复制，也写入下一帧历史。
    let history_usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING; // 历史只由 GPU 复制写入并由计算读取。
    let mask_usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING; // 先计算写入，再由 FSR 采样。
    Fsr3MaskResources {
        opaque_color: texture(
            "fsr3_opaque_color",
            wgpu::TextureFormat::Rgba16Float,
            color_usage,
        ),
        previous_opaque_color: texture(
            "fsr3_previous_opaque_color",
            wgpu::TextureFormat::Rgba16Float,
            history_usage,
        ),
        previous_composed_color: texture(
            "fsr3_previous_composed_color",
            wgpu::TextureFormat::Rgba16Float,
            history_usage,
        ),
        reactive_mask: texture("fsr3_reactive_mask", mask_format, mask_usage),
        transparency_and_composition_mask: texture(
            "fsr3_transparency_and_composition_mask",
            mask_format,
            mask_usage,
        ),
        cached_bind_group: Mutex::new(None),
        history_seeded: AtomicBool::new(false),
    }
}

/// 在不透明主通道完成后保存紧凑 HDR，后续 transmission/transparent 的差值就是合成贡献。
pub(crate) fn copy_fsr3_opaque_color(
    view: ViewQuery<(&Fsr3RenderContext, &MainPassResolutionOverride, &ViewTarget)>,
    mut ctx: RenderContext,
) {
    let (render_context, resolution_override, view_target) = view.into_inner();
    let source = view_target.main_texture();
    let destination = &render_context.masks.opaque_color;
    let render_size = resolution_override.0;
    if source.format() != wgpu::TextureFormat::Rgba16Float
        || destination.format() != wgpu::TextureFormat::Rgba16Float
        || !source.usage().contains(wgpu::TextureUsages::COPY_SRC)
        || !destination.usage().contains(wgpu::TextureUsages::COPY_DST)
        || source.width() < render_size.x
        || source.height() < render_size.y
        || destination.width() != render_size.x
        || destination.height() != render_size.y
    {
        return; // EarlyPostProcess 的完整 ABI 校验会结构化回退，当前阶段绝不录制非法复制。
    }
    copy_texture(ctx.command_encoder(), source, destination, render_size); // 纯 GPU 有界复制，不扫描实体、不回读像素。
}

/// 在任何复制或计算前验证自动遮罩的纹理 ABI，失败交给 FSR 统一结构化回退。
pub(crate) fn validate_mask_contract(
    resources: &Fsr3MaskResources,
    composed_color: &Texture,
    motion_vectors: &Texture,
    internal_resolution: UVec2,
    expected_mask_format: wgpu::TextureFormat,
) -> Result<(), String> {
    let extent = |texture: &Texture| UVec2::new(texture.width(), texture.height());
    for (label, texture) in [
        ("不透明快照", &resources.opaque_color),
        ("上一帧不透明快照", &resources.previous_opaque_color),
        ("上一帧合成快照", &resources.previous_composed_color),
    ] {
        if texture.format() != wgpu::TextureFormat::Rgba16Float
            || extent(texture) != internal_resolution
        {
            return Err(format!(
                "FSR 3 {label}需要内部尺寸 Rgba16Float，实际为 {:?} {}",
                texture.format(),
                extent(texture)
            ));
        }
    }
    for (label, texture) in [
        ("反应遮罩", &resources.reactive_mask),
        ("透明合成遮罩", &resources.transparency_and_composition_mask),
    ] {
        if texture.format() != expected_mask_format || extent(texture) != internal_resolution {
            return Err(format!(
                "FSR 3 {label}需要内部尺寸 {expected_mask_format:?}，实际为 {:?} {}",
                texture.format(),
                extent(texture)
            ));
        }
        if !texture
            .usage()
            .contains(wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING)
        {
            return Err(format!("FSR 3 {label}缺少存储写入或采样用途"));
        }
    }
    if !resources.opaque_color.usage().contains(
        wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
    ) || !composed_color.usage().contains(
        wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
    ) {
        return Err("FSR 3 自动遮罩当前颜色缺少复制或采样用途".into());
    }
    for (label, texture) in [
        ("上一帧不透明快照", &resources.previous_opaque_color),
        ("上一帧合成快照", &resources.previous_composed_color),
    ] {
        if !texture
            .usage()
            .contains(wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING)
        {
            return Err(format!("FSR 3 {label}缺少历史复制或采样用途"));
        }
    }
    if motion_vectors.format() != wgpu::TextureFormat::Rg16Float
        || motion_vectors.width() < internal_resolution.x
        || motion_vectors.height() < internal_resolution.y
        || !motion_vectors
            .usage()
            .contains(wgpu::TextureUsages::TEXTURE_BINDING)
    {
        return Err("FSR 3 自动遮罩需要覆盖内部尺寸且可采样的 Rg16Float 运动矢量".into());
    }
    Ok(())
}

/// 生成本帧两张遮罩并在同一编码器末尾推进两帧颜色历史。
#[allow(clippy::too_many_arguments)] // 每个输入对应独立着色器绑定，合并会隐藏资源 ABI。
pub(crate) fn record_fsr3_masks(
    pipeline: &Fsr3MaskPipeline,
    resources: &Fsr3MaskResources,
    composed_color: &Texture,
    motion_vectors: &Texture,
    internal_resolution: UVec2,
    reset_history: bool,
    device: &RenderDevice,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), String> {
    let first_frame = !resources.history_seeded.swap(true, Ordering::Relaxed); // 新尺寸上下文没有可重投影历史。
    if first_frame || reset_history {
        copy_color_history(resources, composed_color, internal_resolution, encoder); // 切镜帧先用当前图播种，禁止整屏假变化。
    }
    let bind_group = mask_bind_group(pipeline, resources, composed_color, motion_vectors, device)?; // 运动附件未换代时只克隆 O(1) 共享句柄。
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fsr3_generate_reactive_and_composition_masks"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(
            internal_resolution.x.div_ceil(MASK_WORKGROUP_SIZE),
            internal_resolution.y.div_ceil(MASK_WORKGROUP_SIZE),
            1,
        ); // 着色器边界检查覆盖非八对齐尺寸。
    }
    copy_color_history(resources, composed_color, internal_resolution, encoder); // Compute 读取完成后再原子推进下一帧来源。
    Ok(())
}

/// 创建或复用包含当前运动矢量附件的完整遮罩绑定组。
fn mask_bind_group(
    pipeline: &Fsr3MaskPipeline,
    resources: &Fsr3MaskResources,
    composed_color: &Texture,
    motion_vectors: &Texture,
    device: &RenderDevice,
) -> Result<BindGroup, String> {
    let mut cached = resources
        .cached_bind_group
        .lock()
        .map_err(|_| "FSR 3 自动遮罩绑定缓存锁已损坏".to_owned())?;
    if let Some(current) = cached.as_ref()
        && current.motion_texture_id == motion_vectors.id()
    {
        return Ok(current.bind_group.clone()); // 稳定帧不创建纹理视图或绑定组。
    }
    let descriptor = wgpu::TextureViewDescriptor::default();
    let current_opaque = resources.opaque_color.create_view(&descriptor);
    let current_composed = composed_color.create_view(&descriptor);
    let previous_opaque = resources.previous_opaque_color.create_view(&descriptor);
    let previous_composed = resources.previous_composed_color.create_view(&descriptor);
    let motion = motion_vectors.create_view(&descriptor);
    let reactive = resources.reactive_mask.create_view(&descriptor);
    let composition = resources
        .transparency_and_composition_mask
        .create_view(&descriptor);
    let bind_group = device.create_bind_group(
        "fsr3_generate_masks_bind_group",
        &pipeline.layout,
        &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&current_opaque),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&current_composed),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&previous_opaque),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&previous_composed),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&motion),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&reactive),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&composition),
            },
        ],
    );
    *cached = Some(CachedMaskBindGroup {
        motion_texture_id: motion_vectors.id(),
        bind_group: bind_group.clone(),
    }); // 附件只在视图目标换代时替换缓存。
    Ok(bind_group)
}

/// 把当前不透明/合成颜色复制到上一帧资源；调用顺序由同一命令编码器保证。
fn copy_color_history(
    resources: &Fsr3MaskResources,
    composed_color: &Texture,
    internal_resolution: UVec2,
    encoder: &mut wgpu::CommandEncoder,
) {
    copy_texture(
        encoder,
        &resources.opaque_color,
        &resources.previous_opaque_color,
        internal_resolution,
    );
    copy_texture(
        encoder,
        composed_color,
        &resources.previous_composed_color,
        internal_resolution,
    );
}

/// 录制同尺寸单层纹理的完整 GPU 复制。
fn copy_texture(
    encoder: &mut wgpu::CommandEncoder,
    source: &Texture,
    destination: &Texture,
    size: UVec2,
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: destination,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*; // 纯工作组规则不创建 GPU 设备。
    use std::time::Duration; // 显式 GPU 烟雾必须有有界提交等待。

    /// 只有显式验收命令才创建独立 Vulkan 设备，普通单元测试与 CI 不占用显卡。
    const GPU_SMOKE_ENV: &str = "FSR3_MASK_GPU_SMOKE";

    #[test]
    fn workgroup_rounding_covers_non_aligned_internal_sizes() {
        for (dimension, expected) in [(1_u32, 1_u32), (8, 1), (9, 2), (1270, 159)] {
            assert_eq!(dimension.div_ceil(MASK_WORKGROUP_SIZE), expected);
        }
    }

    #[test]
    fn shader_template_has_exactly_the_two_storage_format_slots() {
        assert_eq!(MASK_SHADER_TEMPLATE.matches(MASK_FORMAT_TOKEN).count(), 2); // 防止替换静默遗漏任一输出。
        for storage_format in ["r8unorm", "rgba8unorm"] {
            let source = MASK_SHADER_TEMPLATE.replace(MASK_FORMAT_TOKEN, storage_format);
            assert!(!source.contains(MASK_FORMAT_TOKEN));
            assert!(source.contains(&format!("texture_storage_2d<{storage_format}, write>")));
        }
    }

    #[test]
    fn generated_masks_passes_real_vulkan_validation_when_explicitly_requested() {
        if std::env::var_os(GPU_SMOKE_ENV).is_none() {
            return; // 默认测试保持纯 CPU；正式验收通过环境变量明确取得 GPU 所有权。
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN, // FSR 候选只支持 Vulkan，不允许 DX12 静默替代。
            flags: wgpu::InstanceFlags::VALIDATION, // 独立烟雾始终打开 wgpu 校验。
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None, // 纯 Compute 不创建窗口或 Surface。
        });
        let adapter =
            bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            }))
            .expect("FSR mask GPU smoke requires a Vulkan hardware adapter");
        let info = adapter.get_info();
        assert_eq!(info.backend, wgpu::Backend::Vulkan); // 防止环境配置意外换后端。
        let required_features =
            adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES; // R8存储写入需要与正式Bevy设备相同的格式能力位。
        let (device, queue) =
            bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("fsr3_mask_gpu_smoke_device"),
                required_features,
                required_limits: wgpu::Limits::downlevel_defaults(),
                ..Default::default()
            }))
            .expect("FSR mask GPU smoke should create a Vulkan device");
        let mask_formats = [
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ]
        .into_iter()
        .filter(|format| {
            if *format == wgpu::TextureFormat::R8Unorm
                && !device
                    .features()
                    .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            {
                return false;
            }
            let features = adapter.get_texture_format_features(*format);
            features.allowed_usages.contains(
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            ) && features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY)
        })
        .collect::<Vec<_>>();
        assert!(
            !mask_formats.is_empty(),
            "Vulkan adapter should expose R8 or RGBA8 mask storage writes"
        );
        println!(
            "FSR mask GPU smoke: adapter='{}', driver='{}', formats={mask_formats:?}",
            info.name, info.driver
        ); // 验收日志保存实际设备与格式回退结果。

        let render_device = RenderDevice::from(device.clone());
        let size = UVec2::new(17, 9); // 同时覆盖X/Y非八对齐工作组边界。
        let texture = |label, format, usage| {
            render_device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.x,
                    height: size.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        for mask_format in mask_formats {
            let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation); // 每种正式格式都捕获管线、绑定、复制和 dispatch 校验错误。
            let pipeline = create_mask_pipeline(&render_device, mask_format); // 正式WGSL由真实Vulkan驱动编译，包括RGBA兼容回退变体。
            let resources = create_mask_resources(&render_device, size, mask_format);
            let composed = texture(
                "fsr3_mask_gpu_smoke_composed",
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let motion = texture(
                "fsr3_mask_gpu_smoke_motion",
                wgpu::TextureFormat::Rg16Float,
                wgpu::TextureUsages::TEXTURE_BINDING,
            );
            validate_mask_contract(&resources, &composed, &motion, size, mask_format)
                .expect("GPU smoke fixture should satisfy the production texture ABI");
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fsr3_mask_gpu_smoke_encoder"),
            });
            record_fsr3_masks(
                &pipeline,
                &resources,
                &composed,
                &motion,
                size,
                true,
                &render_device,
                &mut encoder,
            )
            .expect("mask history and bind group cache should record successfully");
            let submission = queue.submit([encoder.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(Duration::from_secs(30)),
                })
                .expect("FSR mask GPU smoke submission should finish before timeout");
            let validation_error = bevy::tasks::block_on(validation_scope.pop());
            assert!(
                validation_error.is_none(),
                "FSR mask {mask_format:?} Vulkan validation error: {validation_error:?}"
            );
        }
    }
}
