//! 无窗口双后端回归：生产草地管线编译，以及分页/接缝真实 WGSL 高度读回。
use super::clipmap_validation::sample_clipmap_surface_for_validation;
use crate::components::{GpuBladeClipmapBand, GpuBladeClipmapHeight}; // 公共已采用快照契约。
use bevy::prelude::*; // 与场景同型的诊断快照。
use std::{borrow::Cow, sync::mpsc, time::Duration}; // 有界测试等待与 WGSL 借用。
use wgpu::util::DeviceExt; // 测试固定小缓冲初始化。 // scene 实际 Mesh oracle 同时验证的 CPU 镜像。
#[path = "source_frame_tests.rs"]
mod source_frame_tests; // 普通Blade的草根与叶片共同继承倾斜源姿态。

const QUERY: &str = r#"
@group(1) @binding(0) var<storage, read> test_points: array<vec2<f32>>; // 固定测试查询点。
@group(1) @binding(1) var<storage, read_write> test_results: array<vec2<f32>>; // 高度和有效标记。
@compute @workgroup_size(64) fn validate_clipmap(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) local: u32) { // 只增加诊断入口，不复制生产算法。
    if local < 8u { clear_bucket_group(local); } // 验证生产工作组八槽初始化。
    if local < 3u { clear_stage_group(local); } // 验证生产三级统计初始化。
    workgroupBarrier(); // 与正式分类相同的清零屏障。
    let slot = add_bucket_group(local % 8u); // 每桶八线程竞争唯一局部槽。
    atomicAdd(&stage_count_0, 1u); // 三组统计分别应为完整64线程。
    atomicAdd(&stage_count_1, 1u); // 保持正式相同原子类型。
    atomicAdd(&stage_count_2, 1u); // 保持正式相同原子类型。
    workgroupBarrier(); // 等待所有组内原子完成。
    if local < 8u { store_bucket_base(local, load_bucket_group(local) * 10u); } // 每桶预留基址应精确为80。
    workgroupBarrier(); // 与正式基址发布相同的屏障。
    if id.x < arrayLength(&test_points) { // 尾组仍让所有线程参与此前屏障。
        var result = sample_clipmap_surface(test_points[id.x]); // 调用生产分页取高。
        if slot >= 8u || load_bucket_base(local % 8u) != 80u || load_stage_group(0u) != 64u || load_stage_group(1u) != 64u || load_stage_group(2u) != 64u { result = vec2<f32>(-9999.0, 2.0); } // 实际读回同时核对Naga兼容展开的原子与槽位语义。
        test_results[id.x] = result; // 任一工作组错误必与CPU三角真值不符。
    } // 完成当前测试点结果。
} // 完成正式取高与工作组兼容语义的同设备验证。
"#; // 不修改正式四个 Compute 入口。

fn fixed_bands(half_cells: u32, levels: u32) -> [GpuBladeClipmapBand; 16] {
    // GPU夹具保留旧固定模板作为可变环带的一种合法实例。
    let mut bands = [GpuBladeClipmapBand::default(); 16];
    for (level, band) in bands.iter_mut().enumerate().take(levels as usize) {
        *band = GpuBladeClipmapBand {
            inner_half_cells: if level == 0 {
                UVec2::ZERO
            } else {
                UVec2::splat(half_cells / 2)
            },
            outer_half_cells: UVec2::splat(half_cells),
            geometry_mip: level as u32,
        };
    }
    bands
}

#[test] // 默认不触发显卡；设置 BLADE_CLIPMAP_GPU_BACKEND=dx12/vulkan 后执行真实验收。
fn clipmap_gpu_matches_actual_topology_reference() {
    // 指定后端不得静默回退。
    let Ok(backend) = std::env::var("BLADE_CLIPMAP_GPU_BACKEND") else {
        return;
    }; // CI 可显式启用。
    let (backends, expected) = match backend.as_str() {
        // 只接受要求的两个正式后端。
        "dx12" => (wgpu::Backends::DX12, wgpu::Backend::Dx12), // 独立 DX12 验收。
        "vulkan" => (wgpu::Backends::VULKAN, wgpu::Backend::Vulkan), // 独立 Vulkan 验收。
        _ => panic!("BLADE_CLIPMAP_GPU_BACKEND 必须为 dx12 或 vulkan"), // 错配置不能伪装成跳过。
    }; // 完成后端选择。
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::VALIDATION,
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    }); // 不创建窗口。
    let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .expect("指定硬件后端可用"); // 不接受软件回退。
    let info = adapter.get_info(); // 报告实际硬件与后端。
    assert_eq!(info.backend, expected); // 明确验证真正使用指定后端。
    let (device, queue) = bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .expect("创建分页高度测试设备"); // 与正式 R16Unorm 纹理契约相同。
    let height_only=std::env::var("BLADE_CLIPMAP_GPU_ONLY").as_deref()==Ok("height");
    let source_frame_only=std::env::var("BLADE_CLIPMAP_GPU_ONLY").as_deref()==Ok("source_frame");
    if !height_only && !source_frame_only {
        super::volume_pose_tests::validate(&device, &queue); // 在同一真实设备核对绘制姿态，不能只验草根位置。
        super::volume_density_tests::validate(&device, &queue); // 同一作者概率不能因地形类型改变株数或草簇宽度。
    } // 高度专项读回可跳过独立体积回归；默认仍验证完整生产管线。
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("production_blade_clipmap"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(format!("{}\n{QUERY}\n{VOLUME_QUERY}\n{}",crate::shaders::compute_shader_source(),source_frame_tests::query()))),
    }); // 正式源码加只读诊断入口。
    if !height_only { source_frame_tests::validate(&device, &queue, &shader); }
    if source_frame_only {return;} // 姿态专项只读真实生产helper，跳过无关体积及数百组高度验证。
    if !height_only {for entry in [
        "cull_tiles",
        "finalize_tiles",
        "classify_and_compact",
        "finalize_indirect",
    ] {
        // 四阶段必须都能被实际驱动编译。
        let _pipeline = pipeline(&device, &shader, entry); // 覆盖完整 Shader 调用图及新版绑定。
    }} // 完成正式管线编译验证。
    let pipeline = pipeline(&device, &shader, "validate_clipmap"); // 测试入口复用生产取高算法。
    if !height_only {validate_volume_source(&device, &queue, &shader, 1, 1.0, 0.);
    let before = validate_volume_source(&device, &queue, &shader, 2, 1.0, 0.);
    let after = validate_volume_source(&device, &queue, &shader, 2, 1.0, 0.002);
    let stable = |v: Vec<[u32; 5]>| {
        v.into_iter()
            .filter(|p| p[4] == 0x7f007f00)
            .map(|p| (p[0], p[2], p[3]))
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        stable(before),
        stable(after),
        "显示重投射不能重抽草株位置、概率与外观"
    );
    validate_volume_source(&device, &queue, &shader, 2, 0.0, 0.);
    validate_volume_source_in_frame(
        &device,
        &queue,
        &shader,
        2,
        1.0,
        0.,
        Mat4::from_scale_rotation_translation(
            Vec3::new(2., 3., 0.5),
            Quat::from_rotation_y(0.7),
            Vec3::new(10., 5., -8.),
        ),
    );
    for frame in [Mat4::IDENTITY, Mat4::from_scale_rotation_translation(
        Vec3::new(2., 3., 0.5), Quat::from_rotation_z(0.3), Vec3::new(7., 4., -8.))] {
        validate_source_in_frame(&device, &queue, &shader, 1, 1.0, 0., frame, true);
    } // 模型候选不能因父变换或GPU生成阶段重新投到薄壁另一面。
    }
    let (pixels, entries) = fixture_pages(); // 四个已采用页覆盖跨页、父级和非方形场景。
    let atlas = texture(&device, wgpu::TextureFormat::R16Unorm, 257, 257, 4); // 正式二维数组格式。
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &atlas,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&pixels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(514),
            rows_per_image: Some(257),
        },
        wgpu::Extent3d {
            width: 257,
            height: 257,
            depth_or_array_layers: 4,
        },
    ); // 一次测试上传，运行时不复制图集。
    let table = texture(&device, wgpu::TextureFormat::Rgba32Uint, 2, 3, 1); // 按各级实际 Z 页数紧凑排列。
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &table,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&entries),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(32),
            rows_per_image: Some(3),
        },
        wgpu::Extent3d {
            width: 2,
            height: 3,
            depth_or_array_layers: 1,
        },
    ); // 确保跨基础页和父页读取同一布局。
    let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    }); // 单层或多层都用数组视图。
    let table_view = table.create_view(&wgpu::TextureViewDescriptor::default()); // 页表二维整数视图。
    let centers = [
        Vec2::new(253.0, 148.5),
        Vec2::new(1.0, 1.5),
        Vec2::new(508.0, 300.0),
    ]; // 页边、场景近端、非整除远端。
    for (center, missing_mip, stitched) in centers.into_iter().flat_map(|center| {
        [None, Some(0usize), Some(1usize), Some(2usize)].into_iter().flat_map(move |mip|[false,true].map(move |active|(center,mip,active)))
    }) {
        // 每次中心采用同时切换全套参数。
        let mut height = GpuBladeClipmapHeight {stitches: Default::default(),
            holes: [Vec4::ZERO; 32],
            hole_count: 0,
            // 与 scene oracle 共用相同快照类型。
            atlas: Handle::default(),
            page_table: Handle::default(), // CPU 镜像无需资产表。
            world_min: Vec2::ZERO,
            world_max: Vec2::new(509.0, 301.5), // 非方形且粗格不能整除的边界。
            cell_size: Vec2::new(1.0, 1.5),
            logical_cells: UVec2::new(509, 201), // 不同 X/Z 格距。
            max_mip: 2,
            fine_center: center,
            bands: fixed_bands(4, 4),
            band_count: 4, // 几何最远级大于持久最高页，覆盖 mip 钳制。
            missing_pages: [UVec4::ZERO; 16], // 每个视图分别验证完整驻留和0/1/2级局部缺页。
            height_min: -20.0,
            height_max: 60.0,
            revision: [1, 1, 1],
            ready: true, // 完整已采用身份。
        }; // 完成诊断输入。
        if stitched {
            let mut constraints=crate::components::GpuBladeClipmapStitches::default();
            for edge in 0..4 {
                let axis=if edge<2 {1}else{0};let normal=1-axis;
                let step=height.world_max[axis]/16.;
                constraints.edges[edge]=Vec4::new(17.,step,8.,if edge<2 {2.}else{1.});
                for i in 0..=16 {
                    let mut p=Vec2::ZERO;p[axis]=i as f32*step;p[normal]=if edge%2==0 {0.}else{height.world_max[normal]};
                    constraints.heights[edge][i/4][i%4]=5.+(p.x*0.03).sin()*2.+p.y*0.02;
                }
            }
            height.stitches=std::sync::Arc::new(constraints);
        } // 粗共同边格可远大于近场格，必须真正覆盖伸长三角形和角点。
        height.holes[0] = Vec4::new(
            center.x - 1.5,
            center.y - 1.5,
            center.x + 1.5,
            center.y + 1.5,
        );
        height.hole_count = 1; // 真正GPU读取必须拒绝洞内草根，边界仍保留。
        let unmasked = height.clone(); // 已采用相同内容的完整基线，验证缺页只影响局部候选。
        if let Some(mip) = missing_mip {
            height.missing_pages[mip].x = 1;
        } // tile(0,0)+1，仅该实际采样级的该页待换入。
        let mut points = Vec::<[f32; 2]>::new(); // 测试可同步生成少量 CPU 查询点。
        for z in -64..65 {
            for x in -64..65 {
                // 密集扫过全部扇形、边缘角点及普通格。
                points.push(
                    (center + Vec2::new(x as f32 * 0.5 + 0.137, z as f32 * 0.75 + 0.193))
                        .to_array(),
                ); // 避开共享边以唯一识别三角。
            }
        } // 完成固定规模验收样本。
        points.extend([
            center.to_array(),
            (center - Vec2::splat(1.5)).to_array(),
            (center + Vec2::splat(1.5)).to_array(),
            [0.0, 0.0],
            [509.0, 301.5],
            [256.0, 148.5],
        ]); // 显式检查精确场景与页边。
        let encoded = super::clipmap::encode_clipmap(&height); // 与正式准备阶段使用同一参数编码。
        let mut packed=bevy::render::render_resource::encase::UniformBuffer::new(Vec::new());
        packed.write(&encoded).unwrap();let uniform_bytes=packed.into_inner();
        assert_eq!(uniform_bytes.len(),5344); // 共同边界与原分页字段通过正式ShaderType编码。
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("clipmap_test_uniform"),
            contents: &uniform_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        }); // 固定5344字节，不随场景大小增长。
        let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("clipmap_test_height"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(&table_view),
                },
            ],
        }); // 诊断入口只需要三个高度绑定。
        let actual = run_query(&device, &queue, &pipeline, &group0, &points); // 真实 GPU 派发和有界回读。
        let mut visible = 0; // 已驻留区域应继续存在。
        let mut locally_hidden = 0; // 缺页区域必须禁止错误零高草。
        for (point, result) in points.iter().zip(&actual) {
            // 与已由 scene 真网格验证的 CPU 拓扑镜像比较。
            let reference = sample_clipmap_surface_for_validation(
                &height,
                Vec2::from_array(*point),
                |p, mip| fixture_height(p, mip, &pixels),
            ); // 同一页量化数据，避免精度差误判。
            visible += usize::from(reference.is_some()); // 统计当前缺页掩码下仍可绘制的真实面。
            if reference.is_none()
                && sample_clipmap_surface_for_validation(
                    &unmasked,
                    Vec2::from_array(*point),
                    |p, mip| fixture_height(p, mip, &pixels),
                )
                .is_some()
            {
                locally_hidden += 1;
            } // 只统计确实因当前缺页被抑制的候选。
            assert_eq!(
                result[1] > 0.5,
                reference.is_some(),
                "中心 {center:?} 点 {point:?} 覆盖错误"
            ); // 不允许漏草或图外草。
            if let Some(y) = reference {
                assert!(
                    (result[0] - y).abs() < 0.005,
                    "{backend} 中心 {center:?} 拼接{stitched} 缺页{missing_mip:?} 点 {point:?} GPU {} != mesh镜像 {y}",
                    result[0]
                );
            } // 最大误差低于五毫米。
        } // 完成当前视图实际三角高度比较。
        if center == centers[0] && missing_mip.is_some() {
            // 中央跨页视图同时覆盖缺页和驻留区域。
            assert!(
                visible > 0 && locally_hidden > 0,
                "局部缺页不能全域闪黑或完全漏掉抑制，mip={missing_mip:?}"
            ); // GPU/CPU比对已保证实际读回同样满足。
        } // 完成局部缺页闭环验收。
    } // 完成不同中心采用情况。
    eprintln!(
        "Clipmap Blade GPU验证通过：{:?} {}，{}；3视图×4种缺页×2种拼接状态，每组16647点",
        info.backend, info.name,if height_only {"高度专项"} else {"完整管线"}
    ); // 保留可审计的实际硬件记录。
} // 完成双后端回归入口。

fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    // 所有入口使用同一编译配置。
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: None,
        module: shader,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    }) // 驱动同步验证仅限测试。
} // 完成测试管线创建。
fn texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    layers: u32,
) -> wgpu::Texture {
    // 测试明确控制格式和页数。
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("clipmap_test_pages"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    }) // 与正式页布局相同。
} // 完成固定测试纹理创建。
fn fixture_pages() -> (Vec<u16>, Vec<[u32; 4]>) {
    // 页间连续、不同级带明确差异，暴露错误 mip。
    let mut pixels = Vec::new(); // 四个 257² 页的测试载荷。
    for (level, tile_x) in [(0, 0), (0, 1), (1, 0), (2, 0)] {
        // 基础两页和两个父级各一页。
        for z in 0..257 {
            for x in 0..257 {
                // 作者格点量化为正式 R16。
                let gx = (tile_x * 256 + x) as f32 * 2.0_f32.powi(level); // 该格的基础逻辑 X。
                let gz = z as f32 * 2.0_f32.powi(level); // 该格的基础逻辑 Z。
                let raw = 0.5 + 0.21 * (gx * 0.19).sin() * (gz * 0.27).cos() + level as f32 * 0.03; // 鞍形和粗级差异同时存在。
                pixels.push((raw * 65535.0).round() as u16); // 精确保留 CPU/GPU 共用量化值。
            }
        } // 完成当前页。
    } // 完成四页。
    let entry = |layer| [layer, (-20.0f32).to_bits(), 80.0f32.to_bits(), 0]; // 与正式 min/span 页表 ABI 相同。
    (
        pixels,
        vec![entry(1), entry(2), entry(3), [0; 4], entry(4), [0; 4]],
    ) // 紧凑父级行未使用列保持零。
} // 完成测试分页载荷。
fn fixture_height(p: Vec2, mip: u32, pixels: &[u16]) -> f32 {
    // 独立模拟硬件线性页内采样。
    let logical = p / Vec2::new(1.0, 1.5) / 2.0_f32.powi(mip as i32); // 转父级逻辑格。
    let tile = if mip == 0 {
        (logical.x / 256.0).floor().min(1.0) as usize
    } else {
        0
    }; // 只有基础层横跨两页。
    let layer = if mip == 0 { tile } else { mip as usize + 1 }; // 对应上传的四层。
    let q = logical - Vec2::new(tile as f32 * 256.0, 0.0); // 页内坐标。
    let a = q.floor().as_uvec2().min(UVec2::splat(256)); // 首整数 texel。
    let b = (a + UVec2::ONE).min(UVec2::splat(256)); // clamp-to-edge。
    let f = q.fract(); // 双线性权重。
    let read =
        |x: u32, z: u32| pixels[layer * 257 * 257 + z as usize * 257 + x as usize] as f32 / 65535.0; // 正式 R16 解码。
    let low = read(a.x, a.y) * (1.0 - f.x) + read(b.x, a.y) * f.x; // 下边插值。
    let high = read(a.x, b.y) * (1.0 - f.x) + read(b.x, b.y) * f.x; // 上边插值。
    -20.0 + (low * (1.0 - f.y) + high * f.y) * 80.0 // 页面 min/span 解码。
} // 完成独立页面高度参考。
fn run_query(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    heights: &wgpu::BindGroup,
    points: &[[f32; 2]],
) -> Vec<[f32; 2]> {
    // 测试专用有限等待回读。
    let size = (points.len() * 8) as u64; // 每点两个 f32。
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("clipmap_points"),
        contents: bytemuck::cast_slice(points),
        usage: wgpu::BufferUsages::STORAGE,
    }); // 固定测试输入。
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("clipmap_results"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    }); // GPU 取高输出。
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("clipmap_readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    }); // 测试专用回读。
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    }); // 查询输入和输出独立分组。
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default()); // 单提交保持高度版本一致。
    {
        // 派发结束再进行输出复制。
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default()); // 无窗口 Compute。
        pass.set_pipeline(pipeline); // 正式取高函数的诊断入口。
        pass.set_bind_group(0, heights, &[]); // 完整已采用图集/页表/参数。
        pass.set_bind_group(1, &group, &[]); // 测试查询点。
        pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1); // 固定小工作量。
    } // 结束计算通道。
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size); // 同提交获取完整结果。
    let submission = queue.submit([encoder.finish()]); // 记录有限等待身份。
    let (sender, receiver) = mpsc::sync_channel(1); // 测试回调只发送一次。
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        }); // 使用标准异步映射。
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })
        .expect("Clipmap GPU 查询超时"); // 同步等待严格限制在测试。
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("映射回调缺失")
        .expect("读回失败"); // 失败不得报告通过。
    let bytes = readback.slice(..).get_mapped_range().expect("读回缓冲映射范围必须有效"); // 读取完整 GPU 结果。
    bytemuck::cast_slice(&bytes).to_vec() // 拷贝固定测试输出，映射随局部资源释放。
} // 完成测试派发/读回。

const VOLUME_QUERY: &str = r#"
@compute @workgroup_size(1) fn validate_volume(@builtin(global_invocation_id) id:vec3<u32>){
    if (grass_patch.grid.w&0x20000000u)!=0u {
        // 独立遍历室外世界母格作为覆盖参照，不能沿三维作者格的划分复制漏格。
        let spacing=grass_patch.density_world.y;
        let columns=u32(ceil(0.5/spacing))+2u;
        let cell=vec2<i32>(i32(id.x%columns)-1,i32(id.x/columns)-1);
        let root=canonical_lod_root(cell,0u,spacing);
        if all(root>=vec2<f32>(-0.125)) && all(root<vec2<f32>(0.375,0.125)) {
            compact_grass[id.x]=CompactGrassInstance(root.x,2.0,root.y,0u,0x7f007f00u);
        }
    } else {classify_volume_candidate(id.x);}
}
@compute @workgroup_size(1) fn validate_model_surface(@builtin(global_invocation_id) id:vec3<u32>){
    classify_volume_candidate(id.x);
}
"#;
// 测试只添加入口；位置、密度、LOD、法线打包全部运行正式生产函数。
fn validate_volume_source(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shader: &wgpu::ShaderModule,
    divisions: u32,
    quality_density: f32,
    height_offset: f32,
) -> Vec<[u32; 5]> {
    validate_volume_source_in_frame(
        device,
        queue,
        shader,
        divisions,
        quality_density,
        height_offset,
        Mat4::IDENTITY,
    )
}
fn validate_volume_source_in_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shader: &wgpu::ShaderModule,
    divisions: u32,
    quality_density: f32,
    height_offset: f32,
    frame: Mat4,
) -> Vec<[u32; 5]> {
    validate_source_in_frame(device, queue, shader, divisions, quality_density, height_offset, frame, false)
}
fn validate_source_in_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shader: &wgpu::ShaderModule,
    divisions: u32,
    quality_density: f32,
    height_offset: f32,
    frame: Mat4,
    model_surface: bool,
) -> Vec<[u32; 5]> {
    let compute = pipeline(device, shader, if model_surface { "validate_model_surface" } else { "validate_volume" });
    let mut patch = [[0u32; 4]; 26]; // 416字节正式ABI必须包含两份对象矩阵。
    patch[18..22].copy_from_slice(
        &frame
            .to_cols_array_2d()
            .map(|column| column.map(f32::to_bits)),
    );
    patch[22..26].copy_from_slice(
        &frame
            .inverse()
            .to_cols_array_2d()
            .map(|column| column.map(f32::to_bits)),
    );
    let candidates = divisions + 2;
    patch[1] = [3 * candidates, 5 * candidates * candidates, 17, 1 << 30];
    if model_surface { patch[1] = [3, 5, 17, (1 << 30) | (1 << 29)]; }
    patch[2] = [0, 0, 0, 32f32.to_bits()];
    patch[4][3] = 1f32.to_bits();
    patch[6][3] = 1f32.to_bits();
    patch[7] = [1f32.to_bits(); 4];
    patch[5][0] = (candidates as f32).to_bits();
    patch[5][1] = (0.25 / divisions as f32).to_bits();
    let buffer = |label, bytes: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        })
    };
    let mut reference_patch = patch;
    let patch = buffer(
        "volume_patch",
        bytemuck::cast_slice(&patch),
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let mut view_data = [[0u32; 4]; 19];
    view_data[13] = [quality_density.to_bits(), 1f32.to_bits(), 1f32.to_bits(), 0]; // 正式304字节视图的画质倍率槽。
    let view = buffer(
        "volume_view",
        bytemuck::cast_slice(&view_data),
        wgpu::BufferUsages::UNIFORM,
    );
    let state = buffer("volume_state", &[0; 80], wgpu::BufferUsages::STORAGE);
    let compact = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("volume_compact_20B"),
        size: 2560,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let buckets = super::helpers::encode_volume_depth_bucket_draws(3, 0, 0, 128);
    let buckets = buffer("volume_buckets", &buckets, wgpu::BufferUsages::STORAGE);
    let source = texture(device, wgpu::TextureFormat::Rgba32Float, 15, 1, 1);
    let cells: [[f32; 4]; 15] = if model_surface { [
        [0.01, 2., 0., 1.], [1., 0., 0., 0.25], [0.01, 2., 0., 0.],
        [-0.01, 2., 0., 1.], [-1., 0., 0., 0.25], [-0.01, 2., 0., 0.],
        [0., 0.05, 0., 1.], [0., 1., 0., 0.25], [0., 0.05, 0., 1.],
        [0., 0., 0., 1.], [0., -1., 0., 0.25], [0., 0., 0., 1.],
        [100., 0., 0., 1.], [0., 1., 0., 0.25], [100., 0., 0., 1.],
    ] } else { [
        [height_offset, 2. + height_offset, height_offset, 1.],
        [0., 1., 0., 0.25],
        [0., 2., 0., 1.],
        [height_offset, 8. + height_offset, height_offset, 1.],
        [0., 1., 0., 0.25],
        [0., 8., 0., 5.],
        [1., 5., 0., 1.],
        [1., 0., 0., 0.25],
        [1., 5., 0., 8.],
        [100., 0., 0., 1.],
        [0., 1., 0., 0.25],
        [100., 0., 0., 13.],
        [
            0.25 + height_offset,
            2.1875 + height_offset,
            height_offset,
            1.,
        ],
        [-0.6, 0.8, 0., 0.25],
        [0.25, 2.25, 0., 17.], // 相邻斜面和水平面必须共用完整世界草格，不留空带。
    ] };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &source,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&cells),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(240),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 15,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
    // 三维路径现在还读取共享室外密度/拓扑；旧无头源保持分页禁用，但布局必须提供合法绑定。
    let mut disabled=bevy::render::render_resource::encase::UniformBuffer::new(Vec::new());
    disabled.write(&super::clipmap_resources::ClipmapUniform::default()).unwrap();
    let clipmap = buffer(
        "volume_clipmap_disabled",
        &disabled.into_inner(),
        wgpu::BufferUsages::UNIFORM,
    );
    let atlas = texture(device, wgpu::TextureFormat::R32Float, 1, 1, 1);
    let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..default()
    });
    let table = texture(device, wgpu::TextureFormat::Rgba32Uint, 1, 1, 1);
    let table_view = table.create_view(&default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("volume_bind"),
        layout: &compute.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: patch.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&source_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: compact.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: state.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: view.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: buckets.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: clipmap.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(&table_view),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: wgpu::BindingResource::TextureView(&source_view),
            },
        ],
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("volume_readback"),
        size: 2560,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&compute);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(5 * candidates * candidates, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&compact, 0, &read, 0, 2560);
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let data = read.slice(..).get_mapped_range().expect("读回缓冲映射范围必须有效");
    let instances: Vec<[u32; 5]> = data
        .chunks_exact(20)
        .map(|v| {
            std::array::from_fn(|i| u32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .filter(|v| v[4] != 0)
        .collect();
    if model_surface {
        assert_eq!(instances.len(), 4, "近处四个独立面样本都应保留，远处样本应正常剔除");
        let normal_frame = Mat3::from_mat4(frame).inverse().transpose();
        for sample in cells.chunks_exact(3).take(4) {
            let point = frame.transform_point3(Vec3::from_slice(&sample[0]));
            let normal = (normal_frame * Vec3::from_slice(&sample[1])).normalize();
            let packed = normal.extend(1.).to_array()
                .map(|v| (v.clamp(-1., 1.) * 127.).round() as i8 as u8);
            assert!(instances.iter().any(|record| {
                let actual = Vec3::new(f32::from_bits(record[0]), f32::from_bits(record[1]), f32::from_bits(record[2]));
                actual.abs_diff_eq(point, 0.0001) && record[4] == u32::from_le_bytes(packed)
            }), "GPU不能重投射根点或复用薄壁另一侧法线");
        }
        drop(data);
        read.unmap();
        return instances;
    }
    if quality_density == 0.0 {
        assert!(instances.is_empty(), "全局密度为零时三维Blade仍生成了实例");
        return instances;
    }
    assert!(instances.len() >= 3 * divisions as usize);
    assert!(
        instances
            .iter()
            .any(|p| (source_y(p, frame) - (2. + height_offset)).abs() < 0.00001)
    );
    assert!(
        instances
            .iter()
            .any(|p| (source_y(p, frame) - (8. + height_offset)).abs() < 0.00001)
    );
    let normal = Mat3::from_mat4(frame.inverse()).transpose() * Vec3::X;
    let packed = normal
        .normalize()
        .extend(1.)
        .to_array()
        .map(|value| (value.clamp(-1., 1.) * 127.).round() as i8 as u8);
    assert!(instances.iter().any(|p| p[4] == u32::from_le_bytes(packed))); // 壁面法线按完整逆转置变换。
    drop(data);
    read.unmap();
    if frame != Mat4::IDENTITY {
        assert!(instances.iter().all(|p| {
            Vec3::new(
                f32::from_bits(p[0]),
                f32::from_bits(p[1]),
                f32::from_bits(p[2]),
            )
            .is_finite()
        }));
        return instances; // 室外母格的逐位覆盖另由上方identity路径验证。
    }
    reference_patch[1][3] |= 1 << 29;
    queue.write_buffer(&patch, 0, bytemuck::cast_slice(&reference_patch));
    queue.write_buffer(&compact, 0, &[0; 2560]);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&compute);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(5 * candidates * candidates, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&compact, 0, &read, 0, 2560);
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let data = read.slice(..).get_mapped_range().expect("读回缓冲映射范围必须有效");
    let expected: std::collections::BTreeSet<_> = data
        .chunks_exact(20)
        .filter_map(|p| {
            let v: [u32; 5] = std::array::from_fn(|i| {
                u32::from_le_bytes(p[i * 4..i * 4 + 4].try_into().unwrap())
            });
            (v[4] != 0).then_some((v[0], v[2]))
        })
        .collect();
    let actual: Vec<_> = instances
        .iter()
        .filter(|p| f32::from_bits(p[1]) < 3.)
        .map(|p| (p[0], p[2]))
        .collect();
    let unique: std::collections::BTreeSet<_> = actual.iter().copied().collect();
    assert_eq!(actual.len(), unique.len(), "相邻作者面重复生成同一草株");
    assert_eq!(unique, expected, "水平面/斜面交界的草覆盖与室外母格不一致");
    instances
}
fn source_y(instance: &[u32; 5], frame: Mat4) -> f32 {
    frame
        .inverse()
        .transform_point3(Vec3::new(
            f32::from_bits(instance[0]),
            f32::from_bits(instance[1]),
            f32::from_bits(instance[2]),
        ))
        .y
}
