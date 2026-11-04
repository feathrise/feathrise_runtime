//! 显式门控的真实GPU完整Mip生成与异步映射回读对等测试。

use std::borrow::Cow; // WGPU着色器模块直接借用正式WGSL，不维护测试副本。
use std::sync::Arc; // GPU烟雾夹具按正式活动布局共享不可变解析块。
use std::sync::mpsc; // 映射回调通过一次性通道把完成结果交回测试线程。
use std::time::Duration; // 设备轮询和回调接收都有明确超时，避免坏驱动永久挂住测试。

use bevy::prelude::{IVec2, Rect}; // 构造跨两个世界块的非均匀活动布局。

use crate::{VegetationResolvedLayer, VegetationResolvedTile}; // GPU输入必须来自正式活动布局到图集的CPU参考路径。

use super::*; // 读取私有紧凑Mip链作为逐级字节权威。
use crate::blade_backend::density::gpu_mip::plan::GpuMipReadbackPlan;
use crate::blade_backend::density::gpu_mip::{
    GPU_MIP_SHADER, GpuMipDispatch, build_gpu_mip_dispatches,
}; // 测试与后续渲染世界接线复用同一调度ABI和WGSL。 // 独立设备与RenderApp复制复用同一对齐规划。

/// 只有显式设置该变量时才创建独立WGPU设备。
const GPU_SMOKE_ENV: &str = "VEGETATION_LAYOUT_GPU_MIP_SMOKE"; // 普通单元测试和CI无GPU环境保持快速稳定。

/// 显式选择的后端及其期望枚举。
#[derive(Clone, Copy, Debug)]
struct RequestedBackend {
    enabled: wgpu::Backends, // Instance只加载本次要求的一个后端。
    expected: wgpu::Backend, // 适配器创建后再次核对，防止环境变量未生效。
}

/// 真实GPU测试需要的最小设备上下文。
struct GpuContext {
    device: wgpu::Device,    // 创建纹理、管线、命令和MAP_READ缓冲。
    queue: wgpu::Queue,      // 上传基础级并提交完整Compute/复制命令。
    info: wgpu::AdapterInfo, // 输出实际显卡和后端，作为双后端验收证据。
}

#[test]
fn gpu_generated_mips_match_cpu_reference_on_requested_backend() {
    if std::env::var_os(GPU_SMOKE_ENV).is_none() {
        return; // 默认测试不隐式占用显卡；验收命令会明确开启门控并指定后端。
    }
    let requested = requested_backend(); // 门控开启后必须明确选择Vulkan或DX12。
    let atlas = gpu_fixture_atlas(); // 128×64非均匀输入覆盖多块拼接和七次递归平均。
    let base = atlas
        .levels
        .first()
        .expect("GPU fixture atlas must contain a base mip"); // 正式构造器保证至少有安全1×1基础级。
    let dispatches =
        build_gpu_mip_dispatches(base.width, base.height).expect("fixture extent must be valid");
    assert_eq!(dispatches.len() + 1, atlas.levels.len()); // GPU计划必须覆盖CPU参考链的每一层。

    let context = create_gpu_context(requested); // 独立设备避免测试依赖窗口或RenderApp生命周期。
    println!(
        "vegetation layout GPU mip smoke: adapter='{}', backend={:?}, driver='{}'",
        context.info.name, context.info.backend, context.info.driver
    ); // 日志明确记录双后端实际落到的适配器。
    let validation_scope = context
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation); // 所有资源与命令错误都归入本次测试。

    let texture = create_density_texture(&context.device, &atlas); // 只分配完整链，尚未写入父Mip。
    upload_base_level(&context.queue, &texture, base); // 模拟运行时脏区完成后的基础级上传边界。
    let pipeline = create_mip_pipeline(&context.device); // 正式WGSL由真实驱动编译。
    let bind_group_layout = pipeline.get_bind_group_layout(0); // 自动布局仍由正式WGSL绑定声明唯一决定。
    let bind_groups = create_dispatch_bind_groups(
        &context.device,
        &context.queue,
        &bind_group_layout,
        &texture,
        &dispatches,
    ); // 每一项只暴露相邻且不重叠的源/目标Mip视图。
    let readback_plan = GpuMipReadbackPlan::new(base.width, base.height, atlas.levels.len() as u32)
        .expect("fixture complete mip chain should produce a readback plan"); // 与RenderApp共享全部级对齐规划。
    let readback = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vegetation_layout_mip_readback"), // 图形调试器中标识P5.2b1验收缓冲。
        size: readback_plan.buffer_size(),             // 已包含每级行对齐与级间偏移。
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, // GPU复制后仅由CPU读取。
        mapped_at_creation: false, // 映射必须通过异步回调完成。
    });
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vegetation_layout_mip_encoder"), // 一次提交保持逐级写入和全部回读的严格顺序。
        });
    for (dispatch, bind_group) in dispatches.iter().zip(&bind_groups) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("vegetation_layout_mip_level_pass"), // 与RenderApp一致，每级提供逐Mip布局屏障边界。
            timestamp_writes: None,                          // 对等测试不引入时间戳feature要求。
        });
        pass.set_pipeline(&pipeline); // 所有级共享同一个正式计算管线。
        let (workgroups_x, workgroups_y) = dispatch.workgroups(); // 覆盖当前目标级的有效范围。
        pass.set_bind_group(0, bind_group, &[]); // 当前绑定只读父级、只写相邻子级。
        pass.dispatch_workgroups(workgroups_x, workgroups_y, 1); // 二维纹理Z轴固定一。
    } // 全部级仍在同一编码器和提交内，pass边界允许wgpu插入相邻级布局转换。
    encode_all_mip_copies(&mut encoder, &texture, &readback, &readback_plan); // 基础级和GPU级全部进入同一回读快照。
    let submission = context.queue.submit([encoder.finish()]); // 单次提交建立明确的Compute到Copy顺序。

    let (sender, receiver) = mpsc::sync_channel(1); // 回调只发送一次映射结果。
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result); // 测试若已超时退出，关闭通道也不在WGPU回调线程二次panic。
        }); // 运行时同样采用异步映射；测试线程只为确定性验收等待完成。
    context
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),     // 只等待本次完整Mip提交。
            timeout: Some(Duration::from_secs(30)), // 坏驱动或设备丢失不能无限阻塞。
        })
        .expect("GPU mip submission should complete before timeout");
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("GPU mip map callback should be delivered")
        .expect("GPU mip readback buffer should map successfully");

    {
        let mapped = readback.slice(..).get_mapped_range(); // 回调成功后才借用CPU可见字节。
        let actual = readback_plan
            .decode(&mapped)
            .expect("mapped mip bytes should match the shared readback plan"); // 使用运行时同一去填充解码。
        assert_eq!(
            actual,
            atlas.rgba_mip_bytes().unwrap(),
            "GPU mip bytes diverged on {:?}",
            context.info.backend
        ); // 基础级和全部递归子级按连续RGBA字节无容差比较。
    } // 先释放映射视图，再解除缓冲映射。
    readback.unmap(); // 显式完成回读生命周期。
    let validation_error = bevy::tasks::block_on(validation_scope.pop()); // WGPU 29由守卫按线程配对并收集验证错误。
    assert!(
        validation_error.is_none(),
        "GPU mip validation error on {:?}: {validation_error:?}",
        context.info.backend
    ); // 字节一致之外还要求WGPU验证层完全通过。
}

/// 解析显式双后端验收变量。
fn requested_backend() -> RequestedBackend {
    let backend = std::env::var("WGPU_BACKEND")
        .expect("GPU mip smoke requires WGPU_BACKEND=vulkan or WGPU_BACKEND=dx12")
        .to_ascii_lowercase(); // 接受PowerShell设置时的大小写差异。
    match backend.as_str() {
        "vulkan" => RequestedBackend {
            enabled: wgpu::Backends::VULKAN, // Instance不加载DX12作为静默回退。
            expected: wgpu::Backend::Vulkan, // 创建后必须实际报告Vulkan。
        },
        "dx12" => RequestedBackend {
            enabled: wgpu::Backends::DX12, // Instance不加载Vulkan作为静默回退。
            expected: wgpu::Backend::Dx12, // 创建后必须实际报告DX12。
        },
        other => {
            panic!("unsupported WGPU_BACKEND='{other}' for GPU mip smoke; use 'vulkan' or 'dx12'")
        } // P5.2b1只验收Windows正式双后端。
    }
}

/// 创建指定后端的独立无窗口Compute设备。
fn create_gpu_context(requested: RequestedBackend) -> GpuContext {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: requested.enabled, // 禁止后端自动替换掩盖单后端故障。
        flags: wgpu::InstanceFlags::VALIDATION, // 测试始终开启WGPU验证层。
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(), // 使用库默认预算策略。
        backend_options: wgpu::BackendOptions::default(), // 不修改驱动编译器和GL选项。
        display: None,               // 独立Compute烟雾不创建Surface，无需平台显示句柄。
    });
    let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance, // 与正式草地渲染选择一致地优先独显。
        force_fallback_adapter: false, // 软件适配器不能代替正式双后端验收。
        compatible_surface: None,      // Compute与回读不需要窗口表面。
    }))
    .expect("requested GPU backend should expose a hardware adapter");
    let info = adapter.get_info(); // 在请求Device前保存可读验收信息。
    assert_eq!(
        info.backend, requested.expected,
        "WGPU adapter did not honor the explicitly requested backend"
    ); // 防止环境或实例配置意外回退。
    let format_features = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm);
    assert!(
        format_features
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING),
        "{:?} adapter does not support rgba8unorm storage writes",
        info.backend
    ); // 正式图集格式必须能作为Compute输出。
    let (device, queue) = bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("vegetation_layout_mip_smoke_device"), // 驱动调试日志中标识独立测试设备。
        required_features: wgpu::Features::empty(),        // 算法只使用WebGPU核心Compute能力。
        required_limits: wgpu::Limits::downlevel_defaults(), // 8×8工作组和小图集不要求桌面扩展限制。
        ..Default::default()
    }))
    .expect("requested GPU backend should create the mip smoke device");
    GpuContext {
        device, // 保持到异步回调和错误域都完成。
        queue,  // 保持到提交完成。
        info,   // 供日志与断言报告实际后端。
    }
}

/// 构造包含全部八位端点和块边界的128×64参考图集。
fn gpu_fixture_atlas() -> ResolvedDensityAtlas {
    let layer = VegetationLayerId(0x52B1); // 稳定测试层ID只用于诊断。
    let left = VegetationDirtyTile(IVec2::new(-1, 0)); // 负坐标验证图集拼接不依赖世界正象限。
    let right = VegetationDirtyTile(IVec2::ZERO); // 相邻块让高层Mip跨越块边界。
    let mut layout = ActiveVegetationLayout {
        revision: 1,     // 测试固定一次成功活动版本。
        texel_size: 1.0, // 每个基础纹素覆盖一米。
        ..Default::default()
    };
    let left_density = (0..VEGETATION_TILE_TEXEL_COUNT)
        .map(|index| ((index * 37 + index / VEGETATION_TILE_SIDE * 11) % 256) as f32 / 255.0)
        .collect::<Vec<_>>(); // 非对称周期同时暴露X/Y寻址和整数恢复误差。
    let right_density = (0..VEGETATION_TILE_TEXEL_COUNT)
        .map(|index| (255 - ((index * 73 + 19) % 256)) as f32 / 255.0)
        .collect::<Vec<_>>(); // 第二块使用不同周期，边界平均不能偶然保持常量。
    layout.tiles.insert(
        left,
        Arc::new(resolved_gpu_tile(left.0, layer, left_density)),
    ); // 左半基础级来自正式解析块结构。
    layout.tiles.insert(
        right,
        Arc::new(resolved_gpu_tile(right.0, layer, right_density)),
    ); // 右半基础级来自正式解析块结构。
    build_layer_density_atlas(&layout, layer).expect("GPU fixture atlas should build") // CPU链是本测试唯一字节权威。
}

/// 构造一块已经通过核心竞争解析的测试数据。
fn resolved_gpu_tile(
    coordinate: IVec2,
    layer: VegetationLayerId,
    density: Vec<f32>,
) -> VegetationResolvedTile {
    VegetationResolvedTile {
        coordinate: VegetationDirtyTile(coordinate), // 块坐标决定128×64图集中的水平位置。
        world_bounds: Rect::from_corners(
            coordinate.as_vec2() * VEGETATION_TILE_SIDE as f32, // 每纹素一米的块左下角。
            (coordinate + IVec2::ONE).as_vec2() * VEGETATION_TILE_SIDE as f32, // 块右上外边界。
        ),
        layers: vec![VegetationResolvedLayer { layer, density }], // 本测试只需要一个解析层。
    }
}

/// 创建可采样、可逐Mip写入、可上传和可回读的正式格式纹理。
fn create_density_texture(device: &wgpu::Device, atlas: &ResolvedDensityAtlas) -> wgpu::Texture {
    let base = atlas
        .levels
        .first()
        .expect("density atlas always contains a base level");
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vegetation_layout_mip_texture"), // 图形调试器中标识P5.2b1图集。
        size: wgpu::Extent3d {
            width: base.width,        // 基础级真实宽。
            height: base.height,      // 基础级真实高。
            depth_or_array_layers: 1, // 单个无限层是一张普通二维纹理。
        },
        mip_level_count: atlas.levels.len() as u32, // 分配CPU参考链相同的完整级数。
        sample_count: 1,                            // Compute存储纹理不能使用多重采样。
        dimension: wgpu::TextureDimension::D2,      // 叶片密度契约固定二维。
        format: wgpu::TextureFormat::Rgba8Unorm,    // 与正式Bevy图片和叶片采样ABI一致。
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC, // 覆盖上传、逐级生成、消费采样和异步回读全生命周期。
        view_formats: &[],                          // 不需要sRGB或其他格式重解释。
    })
}

/// 只上传CPU参考基础级，后续Mip必须全部由GPU生成。
fn upload_base_level(queue: &wgpu::Queue, texture: &wgpu::Texture, base: &DensityMipLevel) {
    let pixels = rgba_bytes(base); // 正式基础级RGB同值且Alpha固定255。
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,                          // 写入刚创建的完整链纹理。
            mip_level: 0,                     // 只有基础级允许CPU上传。
            origin: wgpu::Origin3d::ZERO,     // 覆盖基础级全范围。
            aspect: wgpu::TextureAspect::All, // RGBA颜色纹理使用全部颜色方面。
        },
        &pixels, // `write_texture`不要求256字节行填充。
        wgpu::TexelCopyBufferLayout {
            offset: 0,                           // 输入切片从首字节开始。
            bytes_per_row: Some(base.width * 4), // 紧凑RGBA行跨度。
            rows_per_image: Some(base.height),   // 单层二维图像的完整行数。
        },
        wgpu::Extent3d {
            width: base.width,        // 上传全部基础级列。
            height: base.height,      // 上传全部基础级行。
            depth_or_array_layers: 1, // 单个二维层。
        },
    );
}

/// 用正式WGSL创建最小Compute管线。
fn create_mip_pipeline(device: &wgpu::Device) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vegetation_layout_mip_shader"), // 驱动日志和图形调试器中标识正式WGSL。
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(GPU_MIP_SHADER)), // 禁止测试复制或改写算法。
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("vegetation_layout_mip_pipeline"), // 标识完整Mip生成管线。
        layout: None,    // 由WGSL三个绑定推导唯一布局，测试随后从管线取回。
        module: &shader, // 使用刚由真实驱动验证的正式模块。
        entry_point: Some("downsample_density"), // 显式锁定稳定入口名。
        compilation_options: wgpu::PipelineCompilationOptions::default(), // 不使用运行时覆盖常量。
        cache: None,     // 独立验收设备不共享管线缓存。
    })
}

/// 为每一级创建非重叠源/目标视图与尺寸uniform。
fn create_dispatch_bind_groups(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    dispatches: &[GpuMipDispatch],
) -> Vec<wgpu::BindGroup> {
    dispatches
        .iter()
        .map(|dispatch| {
            let source_view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("vegetation_layout_mip_source_view"), // 当前父级只读视图。
                base_mip_level: dispatch.source_level,            // 精确选择一个父Mip。
                mip_level_count: Some(1),                         // 禁止视图意外包含目标级。
                ..Default::default()
            });
            let target_view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("vegetation_layout_mip_target_view"), // 当前子级只写视图。
                base_mip_level: dispatch.target_level,            // 精确选择相邻目标Mip。
                mip_level_count: Some(1),                         // 禁止视图与源级重叠。
                ..Default::default()
            });
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vegetation_layout_mip_uniform"), // 每级固定16字节尺寸ABI。
                size: 16,                                     // 两个vec2<u32>。
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, // Queue写入后只供Compute读取。
                mapped_at_creation: false, // 通过统一队列写入保持代码路径清晰。
            });
            queue.write_buffer(&uniform, 0, &dispatch.uniform_bytes()); // 在提交Compute前写入当前级尺寸。
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vegetation_layout_mip_bind_group"), // 一个绑定组对应一次相邻级调度。
                layout,                                          // 使用正式WGSL自动推导的布局。
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,                                                 // `source_density`。
                        resource: wgpu::BindingResource::TextureView(&source_view), // 只读当前父级。
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,                                                 // `target_density`。
                        resource: wgpu::BindingResource::TextureView(&target_view), // 只写相邻子级。
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,                            // `params`。
                        resource: uniform.as_entire_binding(), // 完整16字节尺寸uniform。
                    },
                ],
            }) // BindGroup内部持有纹理视图与uniform资源直到GPU提交完成。
        })
        .collect() // 调度顺序与CPU参考Mip顺序完全相同。
}

/// 把每个纹理Mip复制到共享MAP_READ缓冲。
fn encode_all_mip_copies(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    readback: &wgpu::Buffer,
    plan: &GpuMipReadbackPlan,
) {
    for level in plan.levels() {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,                          // 读取GPU生成后的正式格式图集。
                mip_level: level.mip_level,       // 包括基础级在内逐级复制。
                origin: wgpu::Origin3d::ZERO,     // 每级从左上角开始。
                aspect: wgpu::TextureAspect::All, // RGBA颜色纹理读取全部颜色方面。
            },
            wgpu::TexelCopyBufferInfo {
                buffer: readback, // 所有级共享一个异步映射缓冲。
                layout: wgpu::TexelCopyBufferLayout {
                    offset: level.offset,                        // 使用预先对齐的独立区域。
                    bytes_per_row: Some(level.padded_row_bytes), // 满足256字节复制约束。
                    rows_per_image: Some(level.height),          // 当前二维Mip的完整高度。
                },
            },
            wgpu::Extent3d {
                width: level.width,       // 复制当前级全部有效列。
                height: level.height,     // 复制当前级全部有效行。
                depth_or_array_layers: 1, // 单个二维纹理层。
            },
        );
    }
}

/// 把紧凑R8参考级扩展为正式RGBA8上传/回读字节。
fn rgba_bytes(level: &DensityMipLevel) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(level.pixels.len() * 4); // 一次分配当前级全部通道。
    for density in &level.pixels {
        rgba.extend_from_slice(&[*density, *density, *density, 255]); // 与`ResolvedDensityAtlas::to_image`保持完全相同。
    }
    rgba // 返回无行填充的紧凑比较数据。
}
