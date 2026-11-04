//! 使用真实GPU验证生产深度shader：内部区域只有左上三分之一，输出区域仍需得到正确遮挡。
#[test] // 需要真实GPU，由本任务显式运行。
#[ignore = "需要GPU设备；显式执行离屏验证"] // 普通无GPU CI不初始化设备。
fn gpu_depth_mapping_reads_internal_viewport() {
    // 只测试固定12×12纹理，不创建窗口。
    use wgpu::*; // 测试直接使用wgpu，验证实际shader与附件读写。
    bevy::tasks::block_on(async {
        // 同步等待仅限测试，不进入生产帧路径。
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::VULKAN,
            ..InstanceDescriptor::new_without_display_handle()
        }); // 与游戏相同Vulkan后端。
        let adapter = instance
            .request_adapter(&RequestAdapterOptions::default())
            .await
            .unwrap(); // 设备不可用应明确失败。
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor::default())
            .await
            .unwrap(); // 不需要扩展特性。
        let texture = |usage| {
            device.create_texture(&TextureDescriptor {
                label: None,
                size: Extent3d {
                    width: 12,
                    height: 12,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Depth32Float,
                usage,
                view_formats: &[],
            })
        }; // 与生产相同深度格式。
        let source = texture(TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING); // 模拟完整分配、内部viewport写入。
        let output = texture(TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC); // 回读仅为测试断言。
        let source_view = source.create_view(&Default::default()); // 单层深度视图。
        let output_view = output.create_view(&Default::default()); // 输出深度视图。
        let vertex = "struct FullscreenVertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, } @vertex fn vertex(@builtin(vertex_index) i: u32) -> FullscreenVertexOutput { var p = array<vec2<f32>, 3>(vec2<f32>(-1.0,-1.0), vec2<f32>(3.0,-1.0), vec2<f32>(-1.0,3.0)); var o: FullscreenVertexOutput; o.position = vec4<f32>(p[i],0.0,1.0); o.uv = vec2<f32>(0.0); return o; }"; // 全屏顶点夹具与Bevy接口一致。
        let body = include_str!("depth.wgsl")
            .lines()
            .filter(|line| !line.starts_with("#import"))
            .collect::<Vec<_>>()
            .join("\n"); // 实际生产片元代码。
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: None,
            source: ShaderSource::Wgsl(format!("{vertex}\n{body}").into()),
        }); // 让真实后端编译生产shader。
        let seed = device.create_shader_module(ShaderModuleDescriptor { label: None, source: ShaderSource::Wgsl(format!("{vertex}\n@fragment fn fragment(i: FullscreenVertexOutput) -> @builtin(frag_depth) f32 {{ return select(0.2, 0.8, i.position.x < 2.0); }}").into()) }); // 内部左半前景0.8、右半背景0.2。
        let pipeline = |module: &ShaderModule| {
            device.create_render_pipeline(&RenderPipelineDescriptor {
                label: None,
                layout: None,
                vertex: VertexState {
                    module,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(FragmentState {
                    module,
                    entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: Default::default(),
                depth_stencil: Some(DepthStencilState {
                    format: TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        }; // 与生产深度pass一致。
        let seed_pipeline = pipeline(&seed); // 固定场景深度夹具。
        let map_pipeline = pipeline(&shader); // 实际深度映射shader。
        let uniform = device.create_buffer(&BufferDescriptor {
            label: None,
            size: 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }); // 输出到内部1/3倍率。
        let parameters: Vec<u8> = [1.0_f32 / 3.0, 1.0 / 3.0, 0.0, 0.0]
            .into_iter()
            .flat_map(f32::to_ne_bytes)
            .collect(); // 不依赖额外序列化crate。
        queue.write_buffer(&uniform, 0, &parameters); // 写入DLSS超性能映射参数。
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &map_pipeline.get_bind_group_layout(0),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&source_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        }); // 与生产两项布局一致。
        let mut encoder = device.create_command_encoder(&Default::default()); // 保持深度写入、映射和回读的顺序。
        for (view, pipeline, internal) in [
            (&source_view, &seed_pipeline, true),
            (&output_view, &map_pipeline, false),
        ] {
            // 两次固定GPU绘制。
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachments: &[],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(Operations {
                        load: LoadOp::Clear(0.0),
                        store: StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }); // 同生产反向Z清零。
            pass.set_pipeline(pipeline); // 先写场景，后执行真实映射。
            if internal {
                pass.set_viewport(0.0, 0.0, 4.0, 4.0, 0.0, 1.0);
                pass.set_scissor_rect(0, 0, 4, 4);
            } else {
                pass.set_bind_group(0, &binding, &[]);
            } // 旧实现会错误读取4×4以外的清零区域。
            pass.draw(0..3, 0..1); // 每pass一个全屏三角形。
        } // pass结束后才能复制深度。
        let buffer = device.create_buffer(&BufferDescriptor {
            label: None,
            size: 256 * 12,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }); // 满足纹理回读行对齐。
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &output,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::DepthOnly,
            },
            TexelCopyBufferInfo {
                buffer: &buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(12),
                },
            },
            output.size(),
        ); // 回读原始深度而非截图猜测。
        queue.submit([encoder.finish()]); // 提交固定测试工作。
        let (send, receive) = std::sync::mpsc::channel(); // 仅测试线程等待回读。
        buffer.slice(..).map_async(MapMode::Read, move |result| {
            send.send(result).unwrap();
        }); // 驱动异步映射。
        device
            .poll(PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap(); // 测试等待有超时，不进入运行时。
        receive
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap(); // 明确失败，不忽略设备错误。
        let bytes = buffer.slice(..).get_mapped_range(); // 检查全输出而不只检查内部区域。
        for y in 0..12 {
            for x in 0..12 {
                let start = y * 256 + x * 4;
                let depth = f32::from_ne_bytes(bytes[start..start + 4].try_into().unwrap());
                assert!(
                    (depth - if x < 6 { 0.8 } else { 0.2 }).abs() < 0.00001,
                    "错误输出深度({x},{y})={depth}"
                );
            }
        } // 全144像素应继承正确前景/背景，不能读到零。
    }); // 固定离屏验证结束。
} // GPU回归结束。
