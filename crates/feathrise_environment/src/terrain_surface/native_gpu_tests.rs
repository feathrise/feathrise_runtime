//! 原生体素生产片元的真实GPU烟雾测试；不创建窗口，不读取或修改用户地图。
use std::borrow::Cow;
use wgpu::util::DeviceExt;
mod blend; // 高度重加权、覆盖回退与颜色/PBR/法线数值查询。
mod indexed; // 真正2×2权重/索引纹理与十六候选查询断言。
mod layout; // 完整项目函数布局约束，单独保留标准PBR宿主。

fn production_source() -> String {
    // 查询使用正式稳定ID编码与解析表，不能用测试侧的层索引替代材质查找。
    let merge_keys = [11, 14, 20, 21, 22, 23]
        .map(|index| {
            format!(
                "const TEST_KEY_{index}: f32 = {}.0;",
                super::volume_surface_key(&format!("native-merge-{index}"))
            )
        })
        .join("\n");
    format!(
        "{}\n{merge_keys}\nconst TEST_MISSING_KEY: f32 = {}.0;\n{}\n{}",
        super::wet_surface_tests::native_forward_source(),
        super::volume_surface_key("native-merge-missing"),
        blend::QUERY,
        NATIVE_QUERY
    )
}
const NATIVE_QUERY: &str = r#"
@vertex fn native_test_vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    // 与正式阶段输出一样只用SSA向量，不把带ArrayStride的固定数组置于Function变量。
    let position=vec2<f32>(select(-1.0,3.0,index==1u),select(-1.0,3.0,index==2u));
    return VertexOutput(vec4<f32>(position,0.0,1.0),
        vec4<f32>(position.x,0.0,position.y,1.0),
        vec3<f32>(0.0,1.0,0.0),vec4<f32>(1.0),position,position);
}
fn write_native_merge(offset: u32, value: IndexedSurfaceWeights) {
    native_results[offset]=f32(value.indices[0]); native_results[offset+1u]=f32(value.indices[1]);
    native_results[offset+2u]=f32(value.indices[2]); native_results[offset+3u]=f32(value.indices[3]);
    native_results[offset+4u]=value.weights[0]; native_results[offset+5u]=value.weights[1];
    native_results[offset+6u]=value.weights[2]; native_results[offset+7u]=value.weights[3];
}
fn write_native_weights(offset: u32, value: mat2x4<f32>) {
    for(var slot=0u;slot<8u;slot=slot+1u) { native_results[offset+slot]=surface_weight(value,slot); }
}
@compute @workgroup_size(1) fn verify_native_fields() {
    let painted=volume_paint_at(vec3<f32>(-1.0,0.5,2.0),vec3<f32>(0.0,1.0,0.0));
    let back=volume_paint_at(vec3<f32>(-1.0,0.5,2.0),vec3<f32>(0.0,-1.0,0.0));
    let far=volume_paint_at(vec3<f32>(1000.0),vec3<f32>(0.0,1.0,0.0));
    native_results[0]=painted.keys.x; native_results[1]=painted.weights.x;
    native_results[2]=painted.puddles; native_results[3]=select(0.0,1.0,painted.found);
    native_results[4]=back.puddles; native_results[5]=select(0.0,1.0,back.found);
    native_results[6]=far.puddles; native_results[7]=select(0.0,1.0,far.found);
    let base=IndexedSurfaceWeights(vec4<u32>(10u,11u,12u,13u),vec4<f32>(0.125,0.125,0.25,0.5));
    let keys=vec4<f32>(TEST_KEY_20,TEST_KEY_21,TEST_KEY_22,TEST_KEY_23);
    // 八键均不同且含并列贡献，后半组第三/四槽都必须参加前四名选择。
    write_native_merge(8u,merge_volume_weights(base,keys,vec4<f32>(0.0625,0.0625,0.125,0.25)));
    // 相同材质跨两组、三维组内重复时先合并，再进行四槽截断。
    let duplicate_base=IndexedSurfaceWeights(vec4<u32>(10u,11u,12u,13u),vec4<f32>(0.4,0.3,0.2,0.1));
    write_native_merge(16u,merge_volume_weights(duplicate_base,
        vec4<f32>(TEST_KEY_11,TEST_KEY_11,TEST_KEY_14,0.0),vec4<f32>(0.2,0.1,0.1,0.9)));
    // 缺失ID、零ID和负权重不占覆盖量；完整覆盖仍保持原来的零权重并列顺序。
    write_native_merge(24u,merge_volume_weights(base,
        vec4<f32>(TEST_KEY_20,TEST_KEY_21,TEST_MISSING_KEY,0.0),vec4<f32>(0.25,-0.5,0.9,0.9)));
    write_native_merge(32u,merge_volume_weights(base,keys,vec4<f32>(0.75,0.5,0.0,0.0)));
    // 固定八层读取正式残余/归一化函数，第二列中的残余仍对应原第六层。
    write_native_weights(40u,normalize_surface_weights(mat2x4<f32>(
        vec4<f32>(0.0625,0.0625,0.125,0.125),vec4<f32>(0.125,0.25,0.875,0.0625)),8u,6u));
    write_native_weights(48u,normalize_surface_weights(mat2x4<f32>(
        vec4<f32>(0.5,0.25,0.25,0.5),vec4<f32>(0.25,0.25,0.9,0.5)),8u,6u));
    write_native_weights(56u,normalize_surface_weights(mat2x4<f32>(
        vec4<f32>(0.125,0.125,0.25,0.125),vec4<f32>(0.9,0.0,0.0,0.0)),5u,4u));
    write_native_weights(64u,normalize_surface_weights(mat2x4<f32>(vec4<f32>(0.0),vec4<f32>(0.0)),8u,7u));
    // 同一生产纹素聚合函数读取中心、四角及非对称四邻权重，不能截掉后面的候选。
    write_native_merge(72u,sample_indexed_surface_weights(vec2<f32>(0.5),vec2<f32>(0.0),vec2<f32>(0.0)));
    write_native_merge(80u,sample_indexed_surface_weights(vec2<f32>(0.25,0.25),vec2<f32>(0.0),vec2<f32>(0.0)));
    write_native_merge(88u,sample_indexed_surface_weights(vec2<f32>(0.75,0.25),vec2<f32>(0.0),vec2<f32>(0.0)));
    write_native_merge(96u,sample_indexed_surface_weights(vec2<f32>(0.25,0.75),vec2<f32>(0.0),vec2<f32>(0.0)));
    write_native_merge(104u,sample_indexed_surface_weights(vec2<f32>(0.75,0.75),vec2<f32>(0.0),vec2<f32>(0.0)));
    write_native_merge(112u,sample_indexed_surface_weights(vec2<f32>(0.375,0.625),vec2<f32>(0.0),vec2<f32>(0.0)));
    write_native_merge(120u,sample_indexed_surface_weights(vec2<f32>(-0.1,0.5),vec2<f32>(0.0),vec2<f32>(0.0)));
    verify_native_blends();
}
@compute @workgroup_size(1) fn verify_native_indexed_duplicates() {
    write_native_merge(128u,sample_indexed_surface_weights(vec2<f32>(0.5),vec2<f32>(0.0),vec2<f32>(0.0)));
}
"#;

#[test]
fn native_added_entries_keep_function_locals_layout_free() {
    let source = production_source();
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("生产地表与八候选GPU查询必须通过Naga语义验证");
    layout::assert_contract(&module); // 审计全部项目函数，避免仅移除Vulkan每次报告的第一处。
    if std::env::var_os("TERRAIN_SURFACE_SPIRV_DUMP").is_some() {
        dump_spirv(&source);
    }
}
fn dump_spirv(source: &str) {
    let module = wgpu::naga::front::wgsl::parse_str(source).unwrap();
    let info = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("生产地表Naga验证");
    let mut options = wgpu::naga::back::spv::Options::default();
    options
        .flags
        .insert(wgpu::naga::back::spv::WriterFlags::DEBUG);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    for (stage, entry, label) in [
        (
            wgpu::naga::ShaderStage::Vertex,
            "native_test_vertex",
            "vertex",
        ),
        (wgpu::naga::ShaderStage::Fragment, "fragment", "fragment"),
        (
            wgpu::naga::ShaderStage::Compute,
            "verify_native_fields",
            "compute",
        ),
    ] {
        let words = wgpu::naga::back::spv::write_vec(
            &module,
            &info,
            &options,
            Some(&wgpu::naga::back::spv::PipelineOptions {
                shader_stage: stage,
                entry_point: entry.into(),
            }),
        )
        .expect("原生地表SPIRV定位");
        let path = root.join(format!("native_surface_{label}.spv"));
        std::fs::write(
            &path,
            words
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .expect("写入构建目录SPIRV");
        eprintln!("Native surface SPIRV: {}", path.display());
    }
}

#[test]
fn native_surface_gpu_pipeline_and_field_queries() {
    let Ok(backend) = std::env::var("TERRAIN_SURFACE_GPU_BACKEND") else {
        return;
    };
    let (backends, expected) = match backend.as_str() {
        "dx12" => (wgpu::Backends::DX12, wgpu::Backend::Dx12),
        "vulkan" => (wgpu::Backends::VULKAN, wgpu::Backend::Vulkan),
        _ => panic!("TERRAIN_SURFACE_GPU_BACKEND必须是dx12或vulkan"),
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::VALIDATION,
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        display: None,
    });
    let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .expect("指定真实GPU必须可用");
    assert_eq!(adapter.get_info().backend, expected);
    let (device, queue) =
        bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("原生地表测试设备");
    let source = production_source();
    if std::env::var_os("TERRAIN_SURFACE_SPIRV_DUMP").is_some() {
        dump_spirv(&source);
    }
    let indexed = indexed::resources(&device, &queue, &source);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native_terrain_production_shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
    });
    // 驱动编译完整生产Forward片元；白盒桩仅代替Bevy标准PBR入口。
    let _forward = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("native_terrain_forward"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("native_test_vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("native_fields_readback"),
        layout: None,
        module: &shader,
        entry_point: Some("verify_native_fields"),
        compilation_options: Default::default(),
        cache: None,
    });
    let duplicate_compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("native_indexed_duplicate_readback"),
        layout: None,
        module: &shader,
        entry_point: Some("verify_native_indexed_duplicates"),
        compilation_options: Default::default(),
        cache: None,
    });
    let paint = vec![("native-soil".to_owned(), 192)];
    let bytes = super::build_volume_fields_with_puddles(
        0.25,
        std::iter::once(([-1., 0.5, 2.], [0., 1., 0.], paint.as_slice(), 191)),
    )
    .unwrap()
    .data
    .unwrap();
    let fields = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("real_native_field_table"),
        contents: &bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let origin_bytes: Vec<_> = [0f32, 0., 0., 0.25]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let origin = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("native_field_step"),
        contents: &origin_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let entries = [11, 14, 20, 21, 22, 23].map(|index| (format!("native-merge-{index}"), index));
    let lookup = super::build_volume_lookup(&entries, 10, 12).unwrap();
    assert!(entries.iter().all(|(id, _)| super::volume_surface_key(id)
        != super::volume_surface_key("native-merge-missing")));
    let lookup_bytes: Vec<_> = lookup
        .into_iter()
        .flat_map(|entry| entry.to_array())
        .flat_map(u32::to_le_bytes)
        .collect();
    let layers = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("real_native_material_lookup"),
        contents: &lookup_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let result_size = 156 * 4; // 字段、四槽/八层、真实纹素及两组颜色/PBR/法线混合。
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native_field_results"),
        size: result_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native_field_readback"),
        size: result_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("native_field_bindings"),
        layout: &compute.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 100,
                resource: indexed.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 101,
                resource: wgpu::BindingResource::TextureView(&indexed.weights),
            },
            wgpu::BindGroupEntry {
                binding: 111,
                resource: wgpu::BindingResource::TextureView(&indexed.indices),
            },
            wgpu::BindGroupEntry {
                binding: 113,
                resource: layers.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 114,
                resource: fields.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 115,
                resource: origin.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 118,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let duplicate_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("native_indexed_duplicate_bindings"),
        layout: &duplicate_compute.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 100,
                resource: indexed.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 101,
                resource: wgpu::BindingResource::TextureView(&indexed.weights),
            },
            wgpu::BindGroupEntry {
                binding: 111,
                resource: wgpu::BindingResource::TextureView(&indexed.duplicate_indices),
            },
            wgpu::BindGroupEntry {
                binding: 118,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&compute);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&duplicate_compute);
        pass.set_bind_group(0, &duplicate_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, result_size);
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .expect("原生字段GPU计算超时");
    receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("原生字段GPU回读超时")
        .expect("原生字段回读");
    let bytes = readback.slice(..).get_mapped_range();
    let values: Vec<_> = bytes
        .chunks_exact(4)
        .map(|value| f32::from_le_bytes(value.try_into().unwrap()))
        .collect();
    assert_eq!(values[0], super::volume_surface_key("native-soil") as f32);
    assert!((values[1] - 192.0 / 255.0).abs() < 0.00001);
    assert!((values[2] - 191.0 / 255.0).abs() < 0.00001);
    assert_eq!(values[3], 1.0); // 负格坐标必须命中真实三维笔触。
    assert_eq!(&values[4..8], &[0.; 4]); // 同XZ倒扣面和窗口外不能借用顶面材质或水坑。
    // 直接校验生产查询数值，不在CPU测试侧复制候选去重/排序算法。
    for (case, (indices, weights)) in [
        ([13., 23., 12., 22.], [0.25, 0.25, 0.125, 0.125]),
        ([11., 10., 12., 14.], [0.48, 0.24, 0.12, 0.1]),
        ([13., 20., 12., 10.], [0.375, 0.25, 0.1875, 0.09375]),
        ([20., 21., 10., 10.], [0.75, 0.5, 0., 0.]),
    ]
    .into_iter()
    .enumerate()
    {
        let offset = 8 + case * 8;
        assert_eq!(&values[offset..offset + 4], &indices, "八候选案例{case}");
        for (slot, expected) in weights.into_iter().enumerate() {
            assert!(
                (values[offset + 4 + slot] - expected).abs() < 0.00001,
                "八候选案例{case}的权重槽{slot}错误"
            );
        }
    }
    for (case, expected) in [
        [0.0625, 0.0625, 0.125, 0.125, 0.125, 0.25, 0.1875, 0.0625],
        [0.2, 0.1, 0.1, 0.2, 0.1, 0.1, 0., 0.2],
        [0.125, 0.125, 0.25, 0.125, 0.375, 0., 0., 0.],
        [0., 0., 0., 0., 0., 0., 0., 1.],
    ]
    .into_iter()
    .enumerate()
    {
        let offset = 40 + case * 8;
        for (slot, expected) in expected.into_iter().enumerate() {
            assert!(
                (values[offset + slot] - expected).abs() < 0.00001,
                "固定八层案例{case}的权重槽{slot}错误"
            );
        }
        assert!((values[offset..offset + 8].iter().sum::<f32>() - 1.0).abs() < 0.00001);
    }
    indexed::assert_queries(&values);
    blend::assert_queries(&values);
    drop(bytes);
    readback.unmap();
    eprintln!(
        "Native terrain Forward/fields: {:?} {}",
        adapter.get_info().backend,
        adapter.get_info().name
    );
}
