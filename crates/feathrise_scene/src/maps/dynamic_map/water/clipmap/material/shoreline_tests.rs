//! 静水坡岸的真实 GPU 像素回归：共享正式高度页、水波及主/预通道正文。
//! 夹具仅替换宿主相机和 PBR 输出，避免把驱动建管线成功误报为岸线图像正确。

use std::borrow::Cow;
use std::time::Duration;

use bevy::math::{Mat4, UVec4, Vec4};
use bevy::render::render_resource::{AsBindGroup, encase::UniformBuffer};
use wgpu::util::DeviceExt;

use super::tests::{common_gpu_fixture, preprocess_gpu_source};
use super::{ClipmapHeightUniform, ClipmapRainHeightMaterial, ClipmapWaterUniform};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 64;
const HEIGHT_SIZE: u32 = 257;
const WATER: &str = include_str!("../../../../../shared/large_map/water/shaders/water.wesl");
const HEIGHT_SHADER: &str = include_str!("../../../clipmap/shaders/height.wesl");
const COVERAGE: &str = include_str!("../../shaders/clipmap_water.wesl");

#[derive(Clone, Copy)]
enum Probe {
    Coverage,
    Main,
    History,
    Prepass,
    Rain,
}

fn production_source(probe: Probe) -> String {
    let prepass = matches!(probe, Probe::History | Probe::Prepass);
    let output = if prepass {
        "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) uv_b: vec2<f32>, @location(2) world_normal: vec3<f32>, @location(4) world_position: vec4<f32>, @location(5) previous_world_position: vec4<f32>, }\nstruct FragmentOutput { @location(0) normal: vec4<f32>, @location(1) motion_vector: vec2<f32>, }\nfn calculate_motion_vector(current: vec4<f32>, previous: vec4<f32>) -> vec2<f32> { return current.xy - previous.xy; }\n"
    } else {
        "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) world_normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, }\n"
    };
    let (uv_a, uv_b) = if prepass { (1, 2) } else { (2, 3) };
    let mut source = common_gpu_fixture(output, uv_a, uv_b).replace(
        "return vec4<f32>(position, 1.0);",
        "return vec4<f32>(position.x, position.z, 0.5, 1.0);",
    ); // 正交俯视只改变宿主投影，生产顶点的高度、法线和水平位移完整保留。
    let mut definitions = vec!["VERTEX_UVS_A", "VERTEX_UVS_B"];
    if prepass {
        definitions.extend([
            "PREPASS_FRAGMENT", "NORMAL_PREPASS", "NORMAL_PREPASS_OR_DEFERRED_PREPASS",
            "MOTION_VECTOR_PREPASS",
        ]);
    }
    for shader in [WATER, HEIGHT_SHADER, COVERAGE] {
        source.push_str(&preprocess_gpu_source(shader, &definitions));
    }
    if matches!(probe, Probe::Coverage) {
        source.push_str(r#"
@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    // 避免Naga把函数局部数组的ArrayStride装饰带入Vulkan Function存储类。
    var p = vec2<f32>(-1.0,-1.0);
    if index == 1u { p = vec2<f32>(3.0,-1.0); }
    if index == 2u { p = vec2<f32>(-1.0,3.0); }
    return VertexOutput(vec4<f32>(p,0.5,1.0), vec4<f32>(0.0), vec3<f32>(0.0,1.0,0.0), p*0.5+0.5, vec2<f32>(0.0));
}
@fragment fn probe_fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let world = mix(clipmap_params.map_bounds.xy, clipmap_params.map_bounds.zw, in.uv);
    let ground = height_at(world, vec2<f32>(0.0));
    let known = known_height(world, vec2<f32>(0.0));
    return vec4<f32>(surface_coverage(in.uv, vec2<f32>(ground,known)), water_level()-ground, known, 1.0);
}
"#);
    } else if prepass {
        source.push_str(&preprocess_gpu_source(
            include_str!("../../shaders/clipmap_water_prepass.wesl"), &definitions,
        ));
        source.push_str(&preprocess_gpu_source(
            include_str!("../../shaders/clipmap_water_prepass_fragment.wesl"), &definitions,
        ));
        source.push_str(r#"
@fragment fn probe_fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.world_position.y, in.uv_b.x, in.previous_world_position.y, surface_coverage(in.uv,in.uv_b));
}
"#);
    } else {
        source.push_str(&preprocess_gpu_source(
            include_str!("../../shaders/clipmap_water_vertex.wesl"), &definitions,
        ));
        if matches!(probe, Probe::Rain) {
            source.push_str(&preprocess_gpu_source(
                include_str!("../../shaders/clipmap_water_height.wesl"), &definitions,
            ));
        }
        source.push_str(r#"
@fragment fn probe_fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let rest_x = mix(clipmap_params.map_bounds.x,clipmap_params.map_bounds.z,in.uv.x);
    return vec4<f32>(in.world_position.y, in.uv_b.x, in.world_position.x-rest_x, surface_coverage(in.uv,in.uv_b));
}
"#);
    }
    source
}

fn previous_depth_cutoff_source() -> String {
    let source = production_source(Probe::Coverage);
    let current = "let depth_coverage = smoothstep(0.0, depth_width, depth);";
    let current_width = "let depth_width = max(fwidth(depth) * 1.5, 0.0001);";
    assert!(source.contains(current) && source.contains(current_width), "旧版对照只替换水深阈值与固定淡出宽度");
    source.replace(current, "let depth_coverage = smoothstep(clipmap_water_params.surface.y, clipmap_water_params.surface.y + depth_width, depth);")
        .replace(current_width, "let depth_width = max(fwidth(depth) * 1.5, 0.02);")
}

fn unattenuated_wave_source() -> String {
    let source = production_source(Probe::Main);
    let attenuation = "let shore_fade = shore_wave_fade(terrain_height, known);";
    assert!(source.contains(attenuation), "完整波幅对照只替换生产顶点的岸带权重");
    source.replace(attenuation, "let shore_fade = 1.0;")
}

#[test]
fn clipmap_water_shoreline_diagnostic_sources_validate() {
    for probe in [Probe::Coverage, Probe::Main, Probe::History, Probe::Rain] {
        let source = production_source(probe);
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|error| panic!("岸线生产 Shader 展平失败：{}", error.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all(),
        ).validate(&module).expect("岸线生产 Shader 接口与正文合法");
    }
}

#[test]
fn clipmap_water_shoreline_gpu_pixels_and_wave_history() {
    let Ok(backend) = std::env::var("CLIPMAP_WATER_GPU_BACKEND") else { return; };
    let (backends, expected) = match backend.to_ascii_lowercase().as_str() {
        "dx12" => (wgpu::Backends::DX12, wgpu::Backend::Dx12),
        "vulkan" => (wgpu::Backends::VULKAN, wgpu::Backend::Vulkan),
        _ => panic!("CLIPMAP_WATER_GPU_BACKEND 必须是 dx12 或 vulkan"),
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends, flags: wgpu::InstanceFlags::VALIDATION,
        memory_budget_thresholds: Default::default(), backend_options: Default::default(), display: None,
    });
    let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false, compatible_surface: None, apply_limit_buckets: false,
    })).expect("岸线回归要求指定真实 GPU 后端");
    assert_eq!(adapter.get_info().backend, expected);
    let (device, queue) = bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
        ..Default::default()
    })).expect("岸线回归使用正式 R16_UNORM 高度页");
    let resources = Resources::new(&device, &queue);

    // 坡面从左端高于水位 0.20m 下降到右端低于水位 0.60m，覆盖浅岸和完整波幅深水。
    let coverage = render(&device, &queue, &resources, Probe::Coverage);
    let before = render_source(&device, &queue, &resources, Probe::Coverage, previous_depth_cutoff_source());
    for x in [42, 56, 62] {
        let pixel = at(&coverage, x);
        assert!((0.0..0.2).contains(&pixel[1]));
        assert!(pixel[0] > 0.95, "浅水深度 {}m 被退岸裁掉：{}", pixel[1], pixel[0]);
        assert_eq!(at(&before, x)[0], 0.0, "旧安全深度阈值必须复现同位置的空白带");
    } // 旧版以 >=0.20m 乃至预设安全深度裁切，这三个真实像素均为零，明确捕获原缺陷。
    assert!(at(&before, 108)[0] > 0.95, "旧版对照也必须有深水，不能以整图空白替代退岸复现");
    assert_eq!(at(&coverage, 16)[0], 0.0, "高于静止水位的干地不能出现水面");
    export_coverage_image(&coverage, &backend, "after");
    export_coverage_image(&before, &backend, "before");
    resources.write_mask(&queue, 128);
    let half = render(&device, &queue, &resources, Probe::Coverage);
    assert!((at(&half, 80)[0] - 128.0 / 255.0).abs() < 0.003);
    resources.write_mask(&queue, 0);
    let empty = render(&device, &queue, &resources, Probe::Coverage);
    assert!(empty.iter().all(|pixel| pixel[0] == 0.0), "零 R8 不能产生水面");
    resources.write_mask(&queue, 255);
    queue.write_buffer(&resources.water, 0, &water_bytes(true));
    let missing = render(&device, &queue, &resources, Probe::Coverage);
    assert!(missing.iter().all(|pixel| pixel[0] == 0.0 && pixel[2] == 0.0), "缺页不能借用平地生成水面");
    queue.write_buffer(&resources.water, 0, &water_bytes(false));

    let main = render(&device, &queue, &resources, Probe::Main);
    let unattenuated = render_source(&device, &queue, &resources, Probe::Main, unattenuated_wave_source());
    let history = render(&device, &queue, &resources, Probe::History);
    let prepass = render(&device, &queue, &resources, Probe::Prepass);
    for x in 42..112 {
        let current = at(&main, x);
        let previous = at(&history, x);
        assert!(current[3] > 0.95, "正式顶点的浅岸像素必须有连续覆盖");
        assert!(current[0] >= current[1] - 0.002, "岸边风浪波谷穿入水底：{current:?}");
        assert!(previous[2] >= previous[1] - 0.002, "历史帧波谷也必须沿相同岸坡衰减：{previous:?}");
        for channel in [0, 1, 3] {
            assert!((current[channel] - previous[channel]).abs() < 0.00002,
                "主画面与预通道当前表面不一致：{current:?} / {previous:?}");
        }
        assert_eq!(at(&prepass, x)[3], 1.0, "生产预通道片元不能再次裁掉浅水");
    }
    let shallow = at(&main, 42);
    let deep = at(&main, 108);
    assert!(-deep[1] > 0.4, "深水采样必须超过完整波幅的安全深度");
    assert!(deep[0].abs() > 0.1, "深水中必须实际存在生产宏观波");
    for channel in 0..4 {
        assert!((deep[channel] - at(&unattenuated, 108)[channel]).abs() < 0.00002,
            "安全深度外应恢复完整生产波幅及水平位移");
    }
    assert!(shallow[0].abs() < deep[0].abs() * 0.5, "浅岸垂直波高必须衰减");
    assert!(shallow[2].abs() < deep[2].abs() * 0.5, "Gerstner 水平位移必须同步沿岸衰减");
    assert_eq!(at(&prepass, 16), [0.0; 4], "生产预通道不能覆盖干地");
    assert!(at(&history, 80)[0] - at(&history, 80)[2] < -0.005,
        "历史帧必须实际使用独立的上一帧波参数，不能复制当前水面");

    resources.write_mask(&queue, 128);
    let half_main = render(&device, &queue, &resources, Probe::Main);
    assert!((at(&half_main, 80)[0] - at(&main, 80)[0]).abs() < 0.00002,
        "R8 强度只定义覆盖，不能改写既定自由波幅语义");
    resources.write_mask(&queue, 0);
    let zero_prepass = render(&device, &queue, &resources, Probe::Prepass);
    assert!(zero_prepass.iter().all(|pixel| *pixel == [0.0; 4]));

    // 雨水捕获逐字使用正式高度片元，不能仅凭主画面覆盖通过就推断代理合同相同。
    resources.write_mask(&queue, 255);
    let rain = render(&device, &queue, &resources, Probe::Rain);
    for x in [42, 56, 62, 108] {
        let pixel = at(&rain, x);
        assert_eq!(pixel[3], 1.0, "雨水代理必须捕获同一浅岸与深水");
        assert!((pixel[2] - at(&main, x)[3]).abs() < 0.00002);
        assert!(((pixel[0] + pixel[1]) * 16.0 - at(&main, x)[0]).abs() < 0.00002,
            "雨水高度编码必须是主画面真实波形Y");
    }
    assert_eq!(at(&rain, 16), [0.0; 4], "雨水代理不能覆盖干地");
    resources.write_mask(&queue, 0);
    let zero_rain = render(&device, &queue, &resources, Probe::Rain);
    assert!(zero_rain.iter().all(|pixel| *pixel == [0.0; 4]));
    resources.write_mask(&queue, 255);
    queue.write_buffer(&resources.water, 0, &water_bytes(true));
    let missing_rain = render(&device, &queue, &resources, Probe::Rain);
    assert!(missing_rain.iter().all(|pixel| *pixel == [0.0; 4]), "雨水代理不能在缺页处猜水高");
    queue.write_buffer(&resources.water, 0, &water_bytes(false));
    let mut holes = [0_u8; 528];
    for (index, value) in [-0.4_f32, -0.5, 0.05, 0.5].into_iter().enumerate() {
        holes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    holes[512..516].copy_from_slice(&1.0_f32.to_le_bytes());
    queue.write_buffer(&resources.holes, 0, &holes);
    let hole_rain = render(&device, &queue, &resources, Probe::Rain);
    for x in [42, 56, 62] {
        assert_eq!(at(&hole_rain, x), [0.0; 4], "雨水代理必须遵循正式洞口遮罩");
    }
    assert_eq!(at(&hole_rain, 108)[3], 1.0, "洞口外的水域不能被一并隐藏");
    queue.write_buffer(&resources.rain_source, 0, &[0_u8; 16]);
    let retired_rain = render(&device, &queue, &resources, Probe::Rain);
    assert!(retired_rain.iter().all(|pixel| *pixel == [0.0; 4]), "撤销来源信用后不得保留捕获覆盖");
    eprintln!("Clipmap shoreline pixels and production wave history: {:?} {}", expected, adapter.get_info().name);
}

fn at(pixels: &[[f32; 4]], x: u32) -> [f32; 4] {
    pixels[(HEIGHT / 2 * WIDTH + x) as usize]
}

fn buffer(device: &wgpu::Device, label: &'static str, bytes: &[u8]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label), contents: bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

fn water_bytes(missing: bool) -> Vec<u8> {
    let mut water = ClipmapWaterUniform {
        surface: Vec4::new(0.0, 0.4, 0.0, 0.0), missing_pages: [UVec4::ZERO; 16],
    }; // 0.4m 是波幅安全带，只允许衰减，不能再变成岸线的空白带。
    if missing { water.missing_pages[0].x = 1; }
    let mut encoded = UniformBuffer::new(Vec::new());
    encoded.write(&water).unwrap();
    encoded.into_inner()
}

fn texture(device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shoreline_numeric_texture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, bytes: &[u8], width: u32, height: u32, stride: u32) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        bytes, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(height) },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
}

struct Resources {
    height: wgpu::Buffer,
    water: wgpu::Buffer,
    waves: wgpu::Buffer,
    globals: wgpu::Buffer,
    holes: wgpu::Buffer,
    rain_source: wgpu::Buffer,
    heights: wgpu::TextureView,
    pages: wgpu::TextureView,
    mask_texture: wgpu::Texture,
    mask: wgpu::TextureView,
    sampler: wgpu::Sampler,
    vertices: wgpu::Buffer,
    vertex_count: u32,
}

impl Resources {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let height = ClipmapHeightUniform {
            stitches: Default::default(), map_bounds: Vec4::new(-1.0, -1.0, 1.0, 1.0),
            grid: Vec4::new(2.0 / 256.0, 2.0 / 256.0, 256.0, 256.0),
            options: Vec4::new(0.0, 1.0, 0.0, 0.0), reserved: Vec4::ZERO,
            source_to_world: Mat4::IDENTITY, world_to_source: Mat4::IDENTITY,
        };
        let mut encoded = UniformBuffer::new(Vec::new());
        encoded.write(&height).unwrap();
        let height_buffer = buffer(device, "shoreline_formal_height_uniform", &encoded.into_inner());
        let heights = texture(device, wgpu::TextureFormat::R16Unorm, HEIGHT_SIZE, HEIGHT_SIZE);
        let heights_bytes: Vec<u8> = (0..HEIGHT_SIZE).flat_map(|_| (0..HEIGHT_SIZE).flat_map(|x| {
            (((1.0 - x as f64 / 256.0) * u16::MAX as f64).round() as u16).to_le_bytes()
        })).collect();
        upload(queue, &heights, &heights_bytes, HEIGHT_SIZE, HEIGHT_SIZE, HEIGHT_SIZE * 2);
        let pages = texture(device, wgpu::TextureFormat::Rgba32Uint, 1, 1);
        let page_bytes: Vec<u8> = [1, (-0.6_f32).to_bits(), 0.8_f32.to_bits(), 0]
            .into_iter().flat_map(u32::to_le_bytes).collect();
        upload(queue, &pages, &page_bytes, 1, 1, 16);
        let mask_texture = texture(device, wgpu::TextureFormat::R8Unorm, 2, 2);
        upload(queue, &mask_texture, &[255; 4], 2, 2, 2);

        // 两帧采用不同振幅，保证运动回归确实消费历史参数；相位使本次采样落在波谷。
        let mut waves = vec![0.0_f32; 160];
        for (offset, amplitude) in [(0, 0.18), (80, 0.12)] {
            waves[offset + 2] = 1.0;
            waves[offset + 13] = 1.0;
            waves[offset + 16..offset + 20].copy_from_slice(&[1.0, 0.0, amplitude, 0.6]);
            waves[offset + 32] = 1.0;
            waves[offset + 36] = 0.35;
        }
        let waves_bytes: Vec<_> = waves.into_iter().flat_map(f32::to_le_bytes).collect();
        let globals_bytes: Vec<_> = [2.6_f32, 0.1, 0.0, 0.0].into_iter().flat_map(f32::to_le_bytes).collect();
        let mut vertices = Vec::new();
        for z in 0..8 {
            for x in 0..64 {
                for (vx, vz) in [(x,z), (x,z+1), (x+1,z), (x+1,z), (x,z+1), (x+1,z+1)] {
                    vertices.extend_from_slice(&[vx as f32 / 32.0 - 1.0, 0.0, vz as f32 / 4.0 - 1.0, 0.0, 0.0, 0.0, 0.0]);
                }
            }
        }
        let vertex_count = vertices.len() as u32 / 7;
        let vertex_bytes: Vec<_> = vertices.into_iter().flat_map(f32::to_le_bytes).collect();
        Self {
            height: height_buffer, water: buffer(device, "shoreline_water_uniform", &water_bytes(false)),
            waves: buffer(device, "shoreline_actual_wave_frames", &waves_bytes),
            globals: buffer(device, "shoreline_host_time", &globals_bytes),
            holes: buffer(device, "shoreline_no_cutouts", &[0; 528]),
            rain_source: buffer(device, "shoreline_rain_source_credit", &[
                1.0_f32.to_le_bytes(), 0.0_f32.to_le_bytes(), 0.0_f32.to_le_bytes(), 0.0_f32.to_le_bytes(),
            ].concat()),
            heights: heights.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() }),
            pages: pages.create_view(&Default::default()), mask: mask_texture.create_view(&Default::default()), mask_texture,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default()
            }),
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shoreline_real_triangles"), contents: &vertex_bytes, usage: wgpu::BufferUsages::VERTEX,
            }), vertex_count,
        }
    }

    fn write_mask(&self, queue: &wgpu::Queue, value: u8) {
        upload(queue, &self.mask_texture, &[value; 4], 2, 2, 2);
    }

    fn entries(&self, probe: Probe) -> Vec<wgpu::BindGroupEntry<'_>> {
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 120, resource: self.height.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 121, resource: wgpu::BindingResource::TextureView(&self.heights) },
            wgpu::BindGroupEntry { binding: 122, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry { binding: 123, resource: wgpu::BindingResource::TextureView(&self.pages) },
            wgpu::BindGroupEntry { binding: 124, resource: self.water.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 125, resource: wgpu::BindingResource::TextureView(&self.mask) },
            wgpu::BindGroupEntry { binding: 126, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry { binding: 129, resource: self.holes.as_entire_binding() },
        ];
        if !matches!(probe, Probe::Coverage) {
            entries.extend([
                wgpu::BindGroupEntry { binding: 100, resource: self.waves.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 119, resource: self.globals.as_entire_binding() },
            ]);
        }
        if matches!(probe, Probe::Rain) {
            entries.push(wgpu::BindGroupEntry { binding: 127, resource: self.rain_source.as_entire_binding() });
        }
        entries
    }
}

fn render(device: &wgpu::Device, queue: &wgpu::Queue, resources: &Resources, probe: Probe) -> Vec<[f32; 4]> {
    render_source(device, queue, resources, probe, production_source(probe))
}

fn render_source(device: &wgpu::Device, queue: &wgpu::Queue, resources: &Resources, probe: Probe, source: String) -> Vec<[f32; 4]> {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shoreline_production_pixel_probe"), source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
    });
    let main_attributes = wgpu::vertex_attr_array![0 => Float32x3, 2 => Float32x2, 3 => Float32x2];
    let prepass_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2];
    let vertex_layout = [Some(wgpu::VertexBufferLayout {
        array_stride: 28, step_mode: wgpu::VertexStepMode::Vertex,
        attributes: if matches!(probe, Probe::History | Probe::Prepass) { &prepass_attributes } else { &main_attributes },
    })];
    let target = Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba32Float, blend: None, write_mask: wgpu::ColorWrites::ALL });
    let mut targets = vec![target.clone()];
    if matches!(probe, Probe::Prepass) {
        targets.push(Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rg32Float, blend: None, write_mask: wgpu::ColorWrites::ALL }));
    }
    let rain_layout = matches!(probe, Probe::Rain).then(|| {
        // 真实 AsBindGroup 布局必须满足生产 Shader；手写129会掩盖材质漏绑定洞口的回归。
        let render_device: bevy::render::renderer::RenderDevice = device.clone().into();
        let mut entries = ClipmapRainHeightMaterial::bind_group_layout_entries(&render_device, false);
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 119, visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(16),
            }, count: None,
        }); // 119仅为展平后宿主Globals，正式材质绑定全部直接来自Rust派生布局。
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shoreline_actual_rain_material_layout"), entries: &entries,
        });
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shoreline_actual_rain_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        })
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shoreline_real_pixel_readback"), layout: rain_layout.as_ref(),
        vertex: wgpu::VertexState {
            module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(),
            buffers: if matches!(probe, Probe::Coverage) { &[] } else { &vertex_layout },
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader, entry_point: Some(if matches!(probe, Probe::Prepass | Probe::Rain) { "fragment" } else { "probe_fragment" }),
            compilation_options: Default::default(), targets: &targets,
        }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shoreline_production_bindings"), layout: &pipeline.get_bind_group_layout(0), entries: &resources.entries(probe),
    });
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shoreline_float_pixels"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let output_view = output.create_view(&Default::default());
    let motion = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shoreline_production_motion_target"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
    });
    let motion_view = motion.create_view(&Default::default());
    let attachment = |view| Some(wgpu::RenderPassColorAttachment {
        view, depth_slice: None, resolve_target: None,
        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
    });
    let mut attachments = vec![attachment(&output_view)];
    if matches!(probe, Probe::Prepass) { attachments.push(attachment(&motion_view)); }
    let size = u64::from(WIDTH * HEIGHT * 16);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shoreline_pixel_readback"), size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shoreline_offscreen_render"), color_attachments: &attachments,
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        if matches!(probe, Probe::Coverage) { pass.draw(0..3, 0..1); }
        else { pass.set_vertex_buffer(0, resources.vertices.slice(..)); pass.draw(0..resources.vertex_count, 0..1); }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(WIDTH * 16), rows_per_image: Some(HEIGHT) } },
        wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
    ); // 128×RGBA32F 的行宽自然满足 256 字节对齐；读回结果用于断言而非只验证管线创建。
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _ = sender.send(result); });
    device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(Duration::from_secs(30)) })
        .expect("岸线像素渲染超时");
    receiver.recv_timeout(Duration::from_secs(30)).expect("岸线像素映射超时").expect("岸线像素映射失败");
    let bytes = readback.slice(..).get_mapped_range().expect("读回缓冲映射范围必须有效");
    let pixels = bytes.chunks_exact(16).map(|pixel| std::array::from_fn(|channel| {
        f32::from_le_bytes(pixel[channel*4..channel*4+4].try_into().unwrap())
    })).collect();
    drop(bytes);
    readback.unmap();
    pixels
}

fn export_coverage_image(pixels: &[[f32; 4]], backend: &str, version: &str) {
    let Some(directory) = std::env::var_os("CLIPMAP_WATER_SHORELINE_IMAGE_DIR") else { return; };
    let directory = std::fs::canonicalize(directory).expect("显式岸线图像输出目录必须已存在");
    let validation = std::fs::canonicalize(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local/validation"))
        .expect("项目验证目录必须存在");
    assert!(directory.starts_with(validation), "岸线夹具只允许输出到项目 .local/validation 内");
    let image = image::RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let coverage = pixels[(y * WIDTH + x) as usize][0].clamp(0.0, 1.0);
        let dry = [110.0, 83.0, 53.0];
        let wet = [31.0, 130.0, 173.0];
        image::Rgba([
            (dry[0] + (wet[0]-dry[0])*coverage) as u8,
            (dry[1] + (wet[1]-dry[1])*coverage) as u8,
            (dry[2] + (wet[2]-dry[2])*coverage) as u8, 255,
        ])
    });
    image.save(directory.join(format!("clipmap-water-shoreline-{backend}-{version}.png"))).expect("保存真实覆盖回读图像");
}
