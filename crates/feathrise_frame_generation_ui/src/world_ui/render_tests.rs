//! 使用离屏Bevy主视图运行正式WorldUI插件，验证中性实例shader、派生管线与同帧回退。
use bevy::{
    prelude::*,
    render::{RenderApp, render_resource::AsBindGroup, storage::ShaderBuffer},
}; // 与生产渲染插件使用相同引擎。
#[derive(Asset, TypePath, AsBindGroup, Clone)] // 中性夹具仍运行真实实例材质绑定，不引入私有游戏UI。
struct TestWorldLabel {
    // shader从本包夹具内嵌读取。
    #[storage(0, read_only)] // 每实例标签颜色。
    data: Handle<ShaderBuffer>, // 一个测试实例。
    #[uniform(100)] // 与storage同时验证真实材质绑定。
    opacity: f32, // 透明度在实际片元计算中使用。
} // 标签夹具材质结束。
const LABEL_SHADER_SOURCE: &str = include_str!("fixtures/instanced_label.wgsl");
const LABEL_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("73038648-5917-4de0-9e6d-cb5af9daaf96"); // 测试shader身份。
impl Material for TestWorldLabel {
    // 使用本包独立维护的完整中性实例shader。
    fn vertex_shader() -> bevy::shader::ShaderRef {
        LABEL_SHADER.into()
    } // 验证真实顶点导数扩边。
    fn fragment_shader() -> bevy::shader::ShaderRef {
        LABEL_SHADER.into()
    } // 验证真实覆盖率着色。
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    } // 透明标签同时参与真实深度测试。
} // 材质夹具结束。
#[test] // GPU实机测试需要显式运行。
#[ignore = "需要真实GPU；运行Bevy离屏渲染链路"] // 默认CI不初始化图形设备。
fn bevy_world_ui_split_and_fallback_render() {
    // 不加载地图、不修改settings、不创建窗口。
    let mut app = App::new(); // 测试独占离屏App。
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
            .disable::<bevy::audio::AudioPlugin>(),
    ); // 使用单线程提取便于检查RenderWorld。
    app.add_plugins((
        feathrise_billboard_text::BillboardTextPlugin,
        crate::FrameGenerationUiPlugin,
        MaterialPlugin::<TestWorldLabel>::default(),
    )); // 正式插件注册路径。
    app.world_mut()
        .resource_mut::<Assets<Shader>>()
        .insert(
            LABEL_SHADER.id(),
            Shader::from_wgsl(LABEL_SHADER_SOURCE, "instanced_label_test.wgsl"),
        )
        .unwrap(); // SDK自带实际中性shader，不通过相对路径偷读游戏资产。
    while app.plugins_state() != bevy::app::PluginsState::Ready {
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(std::time::Duration::from_millis(10));
    } // 测试启动阶段等待设备。
    app.finish();
    app.cleanup(); // 完成真正RenderStartup所需资源。
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            96,
            96,
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            None,
        )); // 小尺寸离屏目标。
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            bevy::camera::RenderTarget::Image(target.into()),
            bevy::camera::Hdr,
            Msaa::Off,
            bevy::camera::MainPassResolutionOverride(UVec2::new(32, 32)),
            bevy::render::camera::TemporalJitter::default(),
            feathrise_scene_core::view::PrimarySceneView,
            feathrise_billboard_text::BillboardCamera,
            bevy::core_pipeline::prepass::DeferredPrepass,
            bevy::core_pipeline::prepass::MotionVectorPrepass,
        ))
        .id(); // 模拟超性能实际viewport。
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Rectangle::new(1.0, 0.2)); // 标准标签四边形。
    let guide = app
        .world_mut()
        .resource_mut::<Assets<feathrise_billboard_text::BillboardTemporalGuideMaterial>>()
        .add(feathrise_billboard_text::BillboardTemporalGuideMaterial {
            base: StandardMaterial {
                unlit: true,
                alpha_mode: AlphaMode::Mask(0.5),
                opaque_render_method: bevy::material::OpaqueRendererMethod::Deferred,
                ..default()
            },
            extension: default(),
        }); // 验证引导主/预通道实际绑定。
    app.world_mut().spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(guide),
        Transform::from_xyz(0.0, 0.4, -3.0),
        feathrise_billboard_text::BillboardTemporalGuide,
    )); // 普通引导保持ECS可见，实际分流时GPU裁掉。
    let material = app
        .world_mut()
        .resource_mut::<Assets<feathrise_billboard_text::BillboardColorMaterial>>()
        .add(feathrise_billboard_text::BillboardColorMaterial {
            base: StandardMaterial {
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            },
            extension: default(),
        }); // 真实文字材质管线。
    app.world_mut().spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, 0.4, -3.0),
        feathrise_scene_core::view::FrameGenerationUi,
    )); // 真实独立文字项。
    let data = app
        .world_mut()
        .resource_mut::<Assets<ShaderBuffer>>()
        .add(ShaderBuffer::from(vec![Vec4::new(0.25, 0.65, 0.9, 0.8)])); // 实际半透明实例颜色。
    let label = app
        .world_mut()
        .resource_mut::<Assets<TestWorldLabel>>()
        .add(TestWorldLabel {
            data,
            opacity: 0.75,
        }); // 同时使用storage与uniform。
    let label_entity = app
        .world_mut()
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(label),
            bevy::mesh::MeshTag(0),
            Transform::from_xyz(0.0, 0.0, -3.0),
            feathrise_scene_core::view::FrameGenerationUi,
        ))
        .id(); // 真实中性实例项；后续核对它确实进入正式独立队列和回退队列。
    app.world_mut()
        .resource_mut::<crate::FrameGenerationUiRequest>()
        .update_for(
            crate::FrameGenerationUiBackend::Dlss,
            true,
            Some(camera),
            true,
        ); // 模拟后端请求，不调用SDK。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30); // 所有等待仅限测试并设置上限。
    loop {
        // 等待异步管线完成并取得一次真实独立绘制。
        app.update(); // 执行正式提取、队列、GPU批次与Core3d。
        let render = app.sub_app_mut(RenderApp).world_mut(); // 检查实际当前帧层。
        let drawn = render
            .query::<&super::components::WorldUiLayer>()
            .iter(render)
            .any(|layer| layer.is_drawn() && !layer.has_failed()); // 不以编译或空纹理冒充成功。
        if drawn && label_is_in_world_ui(render, label_entity) {
            break;
        } // 实际绘制后再测试回退。
        assert!(
            std::time::Instant::now() < deadline,
            "独立WorldUI未完成中性实例的真实绘制"
        ); // 捕获shader/调度未就绪问题。
        std::thread::sleep(std::time::Duration::from_millis(10)); // 测试让异步编译有执行机会。
    } // 首次独立绘制确认结束。
    assert_eq!(read_guide_gate(&mut app), 1.0); // 真正GPU缓冲已暂停场景引导。
    for _ in 0..5 {
        app.update();
    } // 让引导预通道异步完成，检查实际导入shader错误。
    let cache = app
        .sub_app(RenderApp)
        .world()
        .resource::<bevy::render::render_resource::PipelineCache>(); // 检查真实缓存而非字符串占位。
    for pipeline in cache.pipelines() {
        assert!(
            !matches!(
                pipeline.state,
                bevy::render::render_resource::CachedPipelineState::Err(_)
            ),
            "离屏管线编译失败：{:?}",
            pipeline.state
        );
    } // 不允许未使用变体掩盖错误。
    app.world_mut()
        .resource_mut::<crate::FrameGenerationUiRequest>()
        .update_for(crate::FrameGenerationUiBackend::Dlss, false, None, false); // 只关闭FG，保留超性能viewport和时间重建相机。
    for _ in 0..3 {
        // 连续帧验证，防止错误消费上一帧独立纹理。
        app.update(); // 运行真实队列和GPU绘制。
        let render = app.sub_app_mut(RenderApp).world_mut(); // 当前RenderWorld。
        let frame = render.resource::<bevy::diagnostic::FrameCount>().0; // 当前帧身份。
        assert!(
            render
                .resource::<super::resources::WorldUiFrameGate>()
                .split
        ); // 关闭FG不撤销高清名牌。
        assert!(
            render
                .query::<&super::components::WorldUiLayer>()
                .iter(render)
                .any(|layer| layer.frame == frame
                    && layer.is_drawn()
                    && !layer.has_failed()
                    && layer.render_size == UVec2::new(96, 96))
        ); // 真正以输出分辨率绘制，不是复用旧帧。
        assert!(
            !render
                .resource::<crate::UiFramePermission>()
                .allows(frame, camera)
        ); // 不伪造SDK捕获资格。
        assert_eq!(read_guide_gate(&mut app), 1.0); // 分流后不能把字形引导残留到DLSS输入。
    } // 仅超分路径验证结束。
    app.world_mut()
        .resource_mut::<crate::FrameGenerationUiRequest>()
        .update_for(
            crate::FrameGenerationUiBackend::Dlss,
            true,
            Some(camera),
            false,
        ); // 模拟SDK故障，高清层仍不依赖后端。
    app.update(); // 时间重建相机仍存在。
    assert!(
        app.sub_app(RenderApp)
            .world()
            .resource::<super::resources::WorldUiFrameGate>()
            .split
    ); // SDK故障不迫使名字进入低清场景。
    app.world_mut()
        .entity_mut(camera)
        .remove::<bevy::render::camera::TemporalJitter>(); // 同时关闭时间重建且后端不可用才应恢复普通队列。
    app.update(); // 本帧必须恢复普通队列。
    assert!(
        !app.sub_app(RenderApp)
            .world()
            .resource::<super::resources::WorldUiFrameGate>()
            .split
    ); // 失败没有残留分流资格。
    assert!(
        app.world()
            .get::<feathrise_billboard_text::BillboardTemporalGuideSuppressed>(camera)
            .is_none()
    ); // 普通引导没有被主世界提前隐藏。
    assert_eq!(read_guide_gate(&mut app), 0.0); // 后端失败同帧恢复真实GPU引导信号。
    let render = app.sub_app(RenderApp).world(); // 检查归还后的主透明管线身份。
    let variants = render.resource::<super::resources::WorldUiPipelines>(); // 知道哪些管线是独立变体。
    let original = render.resource::<bevy::render::render_phase::ViewSortedRenderPhases<
        bevy::core_pipeline::core_3d::Transparent3d,
    >>(); // 真正普通透明队列。
    assert!(original.values().any(|phase| {
        phase
            .items
            .values()
            .any(|item| item.entity.1 == bevy::render::sync_world::MainEntity::from(label_entity))
    })); // 必须归还中性实例本身，另一项文字成功不能掩盖夹具丢失。
    assert!(
        original
            .values()
            .flat_map(|phase| phase.items.values())
            .all(|item| !variants.0.values().any(|variant| *variant == item.pipeline))
    ); // 回退队列不能残留无抖动变体。
    app.world_mut()
        .entity_mut(camera)
        .insert(bevy::render::camera::TemporalJitter::default()); // 恢复原始时间重建管线规格，避免把新规格异步编译误当作恢复失败。
    app.world_mut()
        .resource_mut::<crate::FrameGenerationUiRequest>()
        .update_for(
            crate::FrameGenerationUiBackend::Dlss,
            true,
            Some(camera),
            true,
        ); // 恢复后端。
    app.update(); // 已完成变体应可立即恢复分流。
    assert!(
        app.sub_app(RenderApp)
            .world()
            .resource::<super::resources::WorldUiFrameGate>()
            .split
    ); // 保证回退没有丢失retained项。
    assert_eq!(read_guide_gate(&mut app), 1.0); // 同步恢复GPU暂停状态。
    assert!(label_is_in_world_ui(
        app.sub_app(RenderApp).world(),
        label_entity
    )); // 恢复后同一实例重新分流。
} // Bevy完整链路回归结束。

fn label_is_in_world_ui(render: &World, entity: Entity) -> bool {
    // 只检查真实retained phase，不以提取标记或shader已注册冒充成功绘制。
    render
        .resource::<bevy::render::render_phase::ViewSortedRenderPhases<super::types::WorldUi3d>>()
        .values()
        .any(|phase| {
            phase.items.values().any(|item| {
                item.item.entity.1 == bevy::render::sync_world::MainEntity::from(entity)
            })
        })
}

#[test]
fn instanced_label_shader_validates_for_split_and_fallback() {
    // 默认CPU测试验证GPU回归所用的同一份完整shader；实际Bevy导入/布局仍由显式GPU回归覆盖。
    for world_ui_pass in [false, true] {
        let mut enabled = true;
        let mut source = String::new();
        for line in LABEL_SHADER_SOURCE.lines() {
            match line.trim() {
                "#import bevy_pbr::mesh_functions"
                | "#import bevy_pbr::mesh_view_bindings::view" => {}
                "#ifdef WORLD_UI_PASS" => enabled = world_ui_pass,
                "#else" => enabled = !world_ui_pass,
                "#endif" => enabled = true,
                directive if directive.starts_with('#') => {
                    panic!("未覆盖的夹具shader预处理指令：{directive}")
                }
                _ if enabled => {
                    source.push_str(line);
                    source.push('\n');
                }
                _ => {}
            }
        }
        // 仅替换Bevy提供的接口，保留真实实例storage、投影、扩边、导数与透明片元代码。
        let source = source
            .replace("#{MATERIAL_BIND_GROUP}", "2")
            .replace("mesh_functions::get_tag", "fixture_get_tag")
            .replace(
                "mesh_functions::get_world_from_local",
                "fixture_get_world_from_local",
            );
        let source = format!(
            "{}\n{source}",
            r#"
struct FixtureView {
    world_from_view: mat4x4<f32>,
    clip_from_world: mat4x4<f32>,
    unjittered_clip_from_world: mat4x4<f32>,
    viewport: vec4<f32>,
    main_pass_viewport: vec4<f32>,
}
@group(0) @binding(0) var<uniform> view: FixtureView;
fn fixture_get_tag(index: u32) -> u32 { return index; }
fn fixture_get_world_from_local(index: u32) -> mat4x4<f32> {
    return mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
}
"#
        );
        let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| {
            panic!("实例标签WGSL解析失败，独立pass={world_ui_pass}：{error:?}")
        });
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| {
            panic!("实例标签WGSL类型/绑定无效，独立pass={world_ui_pass}：{error:?}")
        });
    }
}

fn read_guide_gate(app: &mut App) -> f32 {
    // 仅离屏测试使用GPU回读，运行时没有等待。
    let render = app.sub_app(RenderApp).world(); // 已完成当前帧命令提交。
    let device = render.resource::<bevy::render::renderer::RenderDevice>(); // 使用同一真实设备。
    let queue = render.resource::<bevy::render::renderer::RenderQueue>(); // 同一提交队列。
    let buffers = render.resource::<bevy::render::render_asset::RenderAssets<bevy::render::storage::GpuShaderBuffer>>(); // 实际引导材质绑定的GPU资产。
    let source = &buffers
        .get(feathrise_billboard_text::BILLBOARD_GUIDE_GATE.id())
        .unwrap()
        .buffer; // 不能检查主世界默认值冒充GPU事实。
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    }); // 固定四字节测试结果。
    let mut encoder = device.create_command_encoder(&default()); // 排在本帧实际渲染后。
    encoder.copy_buffer_to_buffer(source, 0, &buffer, 0, 4); // 捕获实际已使用的信号。
    queue.submit([encoder.finish()]); // 提交回读命令。
    let (send, receive) = std::sync::mpsc::channel(); // 测试等待通道。
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap();
        }); // GPU异步映射。
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .unwrap(); // 等待有硬超时，仅限测试。
    receive
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .unwrap(); // 不吞掉设备错误。
    f32::from_ne_bytes(buffer.slice(..).get_mapped_range()[..4].try_into().unwrap()) // 返回真实信号。
} // GPU信号读取结束。
