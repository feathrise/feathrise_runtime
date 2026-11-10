//! 覆盖层生产 WGSL 验证：替换 Bevy 绑定接口，保留真实高度、刷圈和缺页算法正文。
use super::*; // 固定参数布局和材质契约。
use std::borrow::Cow; // 驱动编译借用字符串。
use wgpu::util::DeviceExt; // 固定测试uniform和GPU回读初始化。

fn production_source() -> String {
    // 只模拟 Bevy shader 预处理和标准网格接口。
    let mut output = String::from(
        r#"
struct Vertex { @builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, }
struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) world_normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, }
struct FragmentOutput { @location(0) color: vec4<f32>, }
fn decompress_vertex(vertex: Vertex, instance_index: u32) -> Vertex { return vertex; }
fn get_world_from_local(index: u32) -> mat4x4<f32> { return mat4x4<f32>(vec4<f32>(1.0,0.0,0.0,0.0),vec4<f32>(0.0,1.0,0.0,0.0),vec4<f32>(0.0,0.0,1.0,0.0),vec4<f32>(0.0,0.0,0.0,1.0)); }
fn mesh_position_local_to_world(transform: mat4x4<f32>, p: vec4<f32>) -> vec4<f32> { return transform*p; }
fn position_world_to_clip(p: vec3<f32>) -> vec4<f32> { return vec4<f32>(p,1.0); }
"#,
    ); // 只替换宿主接口，生产算法逐字保留。
    let shared = feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false);
    output.push_str(&shared.replace("#{MATERIAL_BIND_GROUP}", "0"));
    for source in [
        include_str!("shaders/height.wesl"),
        include_str!("shaders/vegetation_overlay.wesl"),
    ] {
        let source = crate::shader_test_support::preprocess(source, &["VERTEX_UVS_A", "VERTEX_UVS_B"], 0)
            .replace("clipmap_height::", "")
            .replace("mesh_functions::", "");
        output.push_str(&source);
        output.push('\n');
    }
    output // 可供Naga和真实后端共用。
}

#[test]
fn clipmap_overlay_production_shader_validates() {
    let source = production_source(); // 与硬件验收完全相同正文。
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source))); // 语法错误给出准确行。
    let info = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("覆盖层生产WGSL必须通过完整验证"); // 检查导数、数组与各阶段调用。
    if std::env::var_os("CLIPMAP_OVERLAY_SPIRV_DUMP").is_some() {
        // 显式诊断开关不影响普通回归。
        let words = wgpu::naga::back::spv::write_vec(
            &module,
            &info,
            &Default::default(),
            Some(&wgpu::naga::back::spv::PipelineOptions {
                shader_stage: wgpu::naga::ShaderStage::Vertex,
                entry_point: "vertex".to_owned(),
            }),
        )
        .expect("导出生产顶点SPIRV"); // 与驱动相同Naga后端。
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/clipmap_overlay_vertex.spv"); // 诊断产物仅放构建目录，不进入源码。
        std::fs::write(
            &path,
            words
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .expect("写入临时SPIRV"); // 仅显式测试允许小文件IO。
        eprintln!("SPIRV dump: {}", path.display()); // 供spirv-dis准确定位装饰结构。
    }
    assert_eq!(OverlayUniform::min_size().get(), 304); // 固定48字节参数和256字节缺页表，不随场景增长。
    assert!(!OverlayExtension::enable_prepass() && !OverlayExtension::enable_shadows()); // 覆盖绝不污染正式深度和阴影。
    assert_eq!(CLIPMAP_VEGETATION_OVERLAY_LAYER, 31); // 与F12相机专属层一致。
}

#[test]
fn clipmap_overlay_gpu_pipeline_compiles() {
    let Ok(backend) = std::env::var("CLIPMAP_OVERLAY_GPU_BACKEND") else {
        return;
    }; // 普通CPU测试不启动显卡。
    let (backends, expected) = match backend.as_str() {
        "dx12" => (wgpu::Backends::DX12, wgpu::Backend::Dx12),
        "vulkan" => (wgpu::Backends::VULKAN, wgpu::Backend::Vulkan),
        _ => panic!("CLIPMAP_OVERLAY_GPU_BACKEND必须是dx12或vulkan"),
    }; // 必须使用指定后端。
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::VALIDATION,
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        display: None,
    }); // 无窗口真实设备。
    let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .expect("指定GPU可用"); // 不允许软件回退。
    assert_eq!(adapter.get_info().backend, expected); // 明确实际后端证据。
    let (device, queue) =
        bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("覆盖测试设备"); // 与正式普通透明材质同能力。
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("production_clipmap_overlay"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(production_source())),
    }); // 驱动编译真实生产算法。
    let attributes = wgpu::vertex_attr_array![0 => Float32x3, 2 => Float32x2, 3 => Float32x2]; // 与真实Clipmap和forward_io保持位置0、UV0位置2、UV1位置3。
    let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("clipmap_overlay_validation"), // 报错可定位。
        layout: None,                              // 根据真实各阶段绑定推导布局。
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 28,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        }, // 实际高度位移入口和双UV字段。
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }), // 真实透明混合片元。
        primitive: Default::default(),             // 三角列表。
        depth_stencil: None,                       // 测试驱动编译，不替代完整游戏深度验收。
        multisample: Default::default(),           // 单采样测试。
        multiview_mask: None,                      // 无多视图。
        cache: None,                               // 本次要求真实编译。
    }); // 任何验证错误令测试失败。
    verify_blank_atlas_parent_page_mask(&device, &queue); // 真实GPU验证旧平场景集不能遮蔽新父页缺失。
    eprintln!(
        "Clipmap overlay production pipeline: {:?} {:?}",
        adapter.get_info().backend,
        adapter.get_info().name
    ); // 记录真实硬件证据。
}

/// 回归从空白图集进入非平父页时，缺页必须按权威spec层级判断而不是旧图集maxMip=0。
fn verify_blank_atlas_parent_page_mask(device: &wgpu::Device, queue: &wgpu::Queue) {
    let source = format!(
        "{}\n@group(0) @binding(129) var<storage, read_write> results: array<f32>;\n@compute @workgroup_size(1) fn verify_missing() {{ results[0] = known_height(vec2<f32>(1.0), vec2<f32>(2.0,0.0)); results[1] = known_height(vec2<f32>(2048.0), vec2<f32>(2.0,0.0)); }}",
        production_source()
    ); // UV_1.x才是正式几何Mip；第2级近点命中缺页，远点仍命中同级其他页。
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("overlay_missing_page_regression"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
    }); // 使用相同shader模块。
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("overlay_missing_page_regression"),
        layout: None,
        module: &shader,
        entry_point: Some("verify_missing"),
        compilation_options: Default::default(),
        cache: None,
    }); // 仅诊断真实缺页查询函数。
    let height: [f32; 16] = [
        0.0, 0.0, 4096.0, 4096.0, 1.0, 1.0, 4096.0, 4096.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]; // 旧空白图集options.x仍为零。
    let height_bytes: Vec<u8> = height.into_iter()
        .chain(Mat4::IDENTITY.to_cols_array())
        .chain(Mat4::IDENTITY.to_cols_array())
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let mut prefix=bevy::render::render_resource::encase::UniformBuffer::new(Vec::new());
    prefix.write(&feathrise_environment::vegetation::blade::GpuBladeClipmapStitches::default()).unwrap();
    let mut packed=prefix.into_inner();packed.extend_from_slice(&height_bytes);let height_bytes=packed;
    let mut overlay_bytes = vec![0u8; 304]; // 正式ShaderType大小。
    overlay_bytes[36..40].copy_from_slice(&2.0f32.to_le_bytes()); // 权威spec.maxMip=2，独立于旧图集。
    overlay_bytes[80..84].copy_from_slice(&1u32.to_le_bytes()); // 第2级tile(0,0)缺页，其他区域仍为正式平地。
    let height_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("flat_atlas_height"),
        contents: &height_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    }); // 接缝4224字节加原参数192字节，与正式ShaderType布局一致。
    let overlay_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("parent_missing_overlay"),
        contents: &overlay_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    }); // 304字节覆盖参数。
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("missing_results"),
        size: 8,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    }); // 两个float结果。
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("missing_readback"),
        size: 8,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }); // 固定8字节回读。
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("missing_bindings"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 120,
                resource: height_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 126,
                resource: overlay_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 129,
                resource: output.as_entire_binding(),
            },
        ],
    }); // 诊断入口无需高度纹理。
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default()); // 一次测试派发。
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    } // 执行生产遮罩查询。
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 8); // 仅复制两个结果。
    let submission = queue.submit([encoder.finish()]); // 实际GPU提交。
    let (sender, receiver) = std::sync::mpsc::channel(); // 测试专用有限等待。
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        }); // 非阻塞映射回调。
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .expect("覆盖缺页回归GPU超时"); // 等待仅限测试。
    receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("覆盖读回回调超时")
        .expect("覆盖读回成功"); // 映射失败必须报错。
    let bytes = readback.slice(..).get_mapped_range().expect("读回缓冲映射范围必须有效"); // 读取8字节实际计算结果。
    assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 0.0); // 父页缺失必须隐藏，不能错误读取平地。
    assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1.0); // 未缺页区域仍保留显示。
    drop(bytes);
    readback.unmap(); // 释放测试映射。
}
