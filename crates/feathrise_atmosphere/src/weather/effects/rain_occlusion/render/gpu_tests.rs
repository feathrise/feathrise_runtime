//! 显式无游戏窗口 GPU 烟雾：运行真实 Bevy 深度预通道、自动 caster 观察器及生产发布 Shader。
use super::super::{casters, components::RainOcclusionCamera, resources::RainOcclusionMap};
use bevy::asset::RenderAssetUsages; // 小型目标仅由GPU写入。
use bevy::camera::{RenderTarget, ScalingMode, visibility::RenderLayers}; // 独立正交相机及生产遮雨层。
use bevy::core_pipeline::prepass::{DeferredPrepass, DepthPrepass}; // 同时建立Forward与Deferred材质真实需要的预通道。
use bevy::material::OpaqueRendererMethod; // 回归真实F12关闭时Forward与Deferred材质策略切换。
use bevy::prelude::*; // 最小无窗口渲染App与测试几何。
use bevy::render::{
    RenderPlugin,
    extract_component::ExtractComponentPlugin,
    extract_resource::ExtractResourcePlugin,
    gpu_readback::{Readback, ReadbackComplete},
    pipelined_rendering::PipelinedRenderingPlugin,
    render_resource::{Extent3d, PollType, TextureDimension, TextureFormat, TextureUsages},
    renderer::RenderDevice,
}; // 官方headless与异步回读API。
use bevy::window::ExitCondition; // 无窗口不会自动退出。
use bevy::winit::WinitPlugin; // 禁止测试创建系统窗口或事件循环。
use feathrise_scene_core::constants::render_layers::RAIN_OCCLUSION_LAYER; // 与生产casters同层。
use std::time::{Duration, Instant}; // 测试等待有总时限，运行时不引入回读或同步等待。 // 使用真实资格登记与提取资源。

const SIZE: u32 = 128; // 测试目标只有129KiB，行跨度1024字节天然满足GPU回读对齐。
const SPAN: f32 = 16.0; // 16米窗口包含环形屋顶和2米天井。

#[derive(Resource, Default)] // 回调只收集最新完整GPU图像。
struct Captured {
    // 测试专用小图，不进入正式运行时。
    bytes: Vec<u8>, // RGBA16F原始通道数据。
    serial: u64,    // 用于拒绝删除屋顶前的迟到回读。
} // 回读状态结束。

#[derive(Component)] // 测试可整批删除屋顶验证深度不会残留。
struct TestRoof; // 不影响生产caster资格。

#[derive(Resource)] // 精确切换测试屋顶材质，不改透明天井样本。
struct RoofMaterials([Handle<StandardMaterial>; 2]); // 同时覆盖Opaque和AlphaMask两个预通道队列。

fn setup(
    // 构造真实摄像机、标准材质以及带天井的四块屋顶。
    mut commands: Commands,                          // 同次创建全部测试实体。
    mut images: ResMut<Assets<Image>>,               // 固定小图输出目标。
    mut meshes: ResMut<Assets<Mesh>>,                // 使用真正GPU网格上传。
    mut materials: ResMut<Assets<StandardMaterial>>, // 使用真正PBR预通道与透明规则。
) {
    // 测试场景创建开始。
    let mut image = Image::new_uninit(
        Extent3d {
            width: SIZE,
            height: SIZE + 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    ); // 与生产输出格式、metadata布局相同。
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC; // COPY_SRC仅用于本测试异步读回。
    let target = images.add(image); // 唯一完整输出身份。
    let transform = Transform::from_xyz(0.0, 16.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z); // 竖直向下，真实雨向相机约定。
    let matrix =
        Mat4::orthographic_rh(-8.0, 8.0, -8.0, 8.0, 32.0, 0.0) * transform.to_matrix().inverse(); // 与实际相机同一反向正交深度。
    commands.insert_resource(RainOcclusionMap {
        image: target.clone(),
        world_to_rain: matrix,
        active: true,
    }); // 生产转换系统读取真实资源。
    commands.spawn((
        // 独立视锥完全不依赖主相机。
        Camera3d {
            depth_texture_usages: (TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING)
                .into(),
            ..default()
        }, // GPU转换可以采样预通道结果。
        Camera {
            order: -1,
            ..default()
        }, // 先完成深度生产。
        RenderTarget::None {
            size: UVec2::splat(SIZE),
        }, // 无任何屏幕或颜色输出。
        Projection::Orthographic(OrthographicProjection {
            near: 0.0,
            far: 32.0,
            scaling_mode: ScalingMode::Fixed {
                width: SPAN,
                height: SPAN,
            },
            area: Rect::from_center_size(Vec2::ZERO, Vec2::splat(SPAN)),
            ..OrthographicProjection::default_3d()
        }), // 深度米制范围与metadata一致。
        DepthPrepass,        // 使用真实StandardMaterial alpha规则。
        DeferredPrepass,     // 与生产相机一致，F12关闭切到Deferred时Opaque/AlphaMask阶段均存在。
        Msaa::Off,           // 与生产转换texture_depth_2d类型一致。
        RainOcclusionCamera, // 仅该标记进入真实producer。
        RenderLayers::layer(RAIN_OCCLUSION_LAYER), // 只有自动登记成功的几何才可写图。
        transform,           // 相机与metadata完整配对。
    )); // 深度相机创建结束。
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        opaque_render_method: OpaqueRendererMethod::Forward,
        ..default()
    }); // 首先建立Forward普通实心屋顶。
    let masked = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Mask(0.5),
        opaque_render_method: OpaqueRendererMethod::Forward,
        ..default()
    }); // 实际alpha=1的裁切屋顶必须进入AlphaMask队列。
    commands.insert_resource(RoofMaterials([material.clone(), masked.clone()])); // 保存两个真实材质的身份用于F12切换回归。
    for (position, size) in [
        (Vec3::new(-3.0, 4.0, 0.0), Vec3::new(4.0, 0.2, 10.0)),
        (Vec3::new(3.0, 4.0, 0.0), Vec3::new(4.0, 0.2, 10.0)),
        (Vec3::new(0.0, 4.0, -3.0), Vec3::new(2.0, 0.2, 4.0)),
        (Vec3::new(0.0, 4.0, 3.0), Vec3::new(2.0, 0.2, 4.0)),
    ] {
        // 中央两米天井保持实际几何开口。
        let roof_material = if position.x > 0.0 {
            masked.clone()
        } else {
            material.clone()
        }; // 右屋顶为AlphaMask，左屋顶为Opaque，分别可从GPU采样证明。
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::from_size(size))),
            MeshMaterial3d(roof_material),
            Transform::from_translation(position),
            TestRoof,
        )); // 不手工赋雨层，必须由生产casters自动登记。
    } // 环形屋顶结束。
    let transparent = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.2),
        alpha_mode: AlphaMode::Blend,
        ..default()
    }); // 连续透明面不应伪造实心屋顶。
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 0.2, 1.0))),
        MeshMaterial3d(transparent),
        Transform::from_xyz(0.0, 6.0, 0.0),
    )); // 位于天井中，验证透明对象不会遮雨。
    commands.spawn(Readback::texture(target)).observe(
        |event: On<ReadbackComplete>, mut result: ResMut<Captured>| {
            // 官方异步回读回调。
            result.bytes.clone_from(&event.data); // 测试小图允许保留完整样本。
            result.serial += 1; // 每次GPU完整回读递增身份。
        },
    ); // 回读实体在整个测试中固定唯一。
} // 场景创建结束。

fn channel(bytes: &[u8], x: u32, y: u32, component: usize) -> f32 {
    // RGBA16F按纹理行直接访问。
    let offset = ((y * SIZE + x) as usize * 4 + component) * 2; // SIZE*8已满足256字节行对齐。
    half::f16::from_bits(u16::from_le_bytes([bytes[offset], bytes[offset + 1]])).to_f32() // 使用真实half解码。
} // 通道读取结束。

fn decode(bytes: &[u8], x: u32, y: u32) -> f32 {
    // 与生产消费者相同11/11/10位契约。
    f32::from_bits(
        channel(bytes, x, y, 0) as u32
            | ((channel(bytes, x, y, 1) as u32) << 11)
            | ((channel(bytes, x, y, 2) as u32) << 22),
    ) // 不进行有损half深度计算。
} // 数值还原结束。

fn depth_at(bytes: &[u8], matrix: Mat4, point: Vec3) -> f32 {
    // 世界坐标投影到真实捕获纹理。
    let ndc = matrix.project_point3(point); // 直接使用发布矩阵，拒绝固定样本像素猜测。
    let uv = ndc.truncate() * Vec2::new(0.5, -0.5) + Vec2::splat(0.5); // Bevy clip到纹理方向。
    decode(
        bytes,
        (uv.x * SIZE as f32) as u32,
        (uv.y * SIZE as f32) as u32 + 1,
    ) // 跳过metadata首行。
} // 深度读取结束。

fn step(app: &mut App) {
    // 仅测试进程允许等待GPU，不修改正式帧路径。
    app.update(); // 运行真正提取、预通道、转换和异步回读。
    app.world()
        .resource::<RenderDevice>()
        .wgpu_device()
        .poll(PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .expect("GPU测试提交必须在十秒内完成"); // 官方headless例子的显式测试同步点。
} // 单次驱动结束。

fn renderer_transition(app: &mut App, matrix: Mat4, method: OpaqueRendererMethod, height: f32) {
    // 模拟缓存禁用相机后F12关闭切换材质，再重新激活捕获。
    let camera = app
        .world_mut()
        .query_filtered::<Entity, With<RainOcclusionCamera>>()
        .single(app.world())
        .unwrap(); // 唯一真实生产相机。
    app.world_mut().get_mut::<Camera>(camera).unwrap().is_active = false; // 缓存帧会撤销对应视图的提取阶段。
    for _ in 0..3 {
        step(app);
    } // 确实让RenderApp处理相机禁用，不在同帧改回掩盖问题。
    let handles = app.world().resource::<RoofMaterials>().0.clone(); // 固定两个共享材质，不复制网格。
    for handle in handles {
        // 同时触发Opaque和AlphaMask实际资产变更。
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(&handle)
            .unwrap()
            .opaque_render_method = method; // 与游戏材质策略切换一致。
    } // 材质切换结束。
    let mut roofs = app
        .world_mut()
        .query_filtered::<&mut Transform, With<TestRoof>>(); // 移动所有测试屋顶，要求GPU生成新深度。
    for mut transform in roofs.iter_mut(app.world_mut()) {
        transform.translation.y = height;
    } // 防止旧Forward纹理使Deferred回归假通过。
    step(app); // 禁用相机时先完成材质提取，覆盖缓存/F12时序。
    app.world_mut().get_mut::<Camera>(camera).unwrap().is_active = true; // 恢复捕获必须重新建立正确的两个预通道。
    let deadline = Instant::now() + Duration::from_secs(30); // GPU编译有界等待，仅限显式测试。
    loop {
        // 异步材质上传、预通道和Readback允许跨帧完成。
        step(app); // DepthPrepass-only旧相机将在Deferred queue阶段真实panic。
        let capture = app.world().resource::<Captured>(); // 使用已完成GPU结果而非CPU预期数据。
        let expected = matrix.project_point3(Vec3::new(0.0, height + 0.1, 0.0)).z; // 当前屋顶Cuboid上表面必须跟随移动。
        let opaque = depth_at(&capture.bytes, matrix, Vec3::new(-3.0, 0.0, 0.0)); // 实际Opaque队列样本。
        let masked = depth_at(&capture.bytes, matrix, Vec3::new(3.0, 0.0, 0.0)); // 实际AlphaMask队列样本。
        if (opaque - expected).abs() < 0.001 && (masked - expected).abs() < 0.001 {
            // 两条实际GPU管线都就绪才能通过。
            assert_eq!(
                depth_at(&capture.bytes, matrix, Vec3::ZERO),
                0.0,
                "切换渲染策略不能填满天井"
            ); // 开口和透明排除语义不能回退。
            break; // 本次切换完整通过。
        } // 旧结果继续等待，不能直接视为成功。
        assert!(
            Instant::now() < deadline,
            "{method:?}相机重启后屋顶深度错误：opaque={opaque},mask={masked},expected={expected}"
        ); // 失败报告精确路径。
    } // 当前策略GPU验证结束。
} // 覆盖真实策略和缓存生命周期切换。

#[test] // 正常cargo test不要求GPU，开发者显式运行本烟雾。
#[ignore = "需要真实GPU；无游戏窗口，显式验证Bevy深度预通道和生产遮雨发布"]
fn real_gpu_roof_depth_keeps_courtyard_open_and_clears_removed_roof() {
    // 不创建主相机，证明遮挡独立于玩家视野。
    let mut app = App::new(); // 独立测试世界，不加载游戏或用户地图。
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    ); // 官方headless模式，测试自己驱动更新。
    app.init_resource::<RainOcclusionMap>()
        .init_resource::<Captured>(); // 生产资源及只读测试样本。
    app.add_plugins((
        ExtractResourcePlugin::<RainOcclusionMap>::default(),
        ExtractComponentPlugin::<RainOcclusionCamera>::default(),
    )); // 与生产相同的提取入口。
    casters::install(&mut app); // 使用真实生命周期观察器，不手工加雨层。
    super::install(&mut app); // 使用真实GPU发布管线与内置Shader。
    app.add_systems(Startup, setup); // 单次构造128²测试场景。
    app.finish(); // 外部驱动必须显式完成插件。
    app.cleanup(); // 不调用app.run，不启动窗口事件循环。
    let deadline = Instant::now() + Duration::from_secs(45); // 编译/设备问题明确超时，不无限等待。
    let matrix = loop {
        // 异步资产与Readback允许跨帧就绪。
        step(&mut app); // CPU等待仅存在于显式GPU测试。
        let capture = app.world().resource::<Captured>(); // 只查看已完成的GPU快照。
        let matrix = app.world().resource::<RainOcclusionMap>().world_to_rain; // 期待元数据来自同一生产需求。
        if capture.bytes.len() >= (SIZE * (SIZE + 1) * 8) as usize
            && channel(&capture.bytes, 16, 0, 0) == 1.0
            && depth_at(&capture.bytes, matrix, Vec3::new(-3.0, 0.0, 0.0)) > 0.5
        {
            break matrix;
        } // 不能把未渲染空纹理误判为成功。
        assert!(
            Instant::now() < deadline,
            "真实GPU未产生有效屋顶深度，最后回读序号{}",
            capture.serial
        ); // 给出可定位的失败。
    }; // 已确认GPU真实生产屋顶深度。
    let capture = app.world().resource::<Captured>(); // 成功帧必须原子包含metadata和深度。
    for (index, expected) in matrix.to_cols_array().into_iter().enumerate() {
        // 覆盖全部16个投影元素。
        assert_eq!(
            decode(&capture.bytes, index as u32, 0).to_bits(),
            expected.to_bits(),
            "GPU矩阵metadata与深度需求不一致"
        ); // 精确验证位编码，不只看valid标记。
    } // metadata完整性验证结束。
    assert_eq!(
        depth_at(&capture.bytes, matrix, Vec3::ZERO),
        0.0,
        "天井/透明面下不应该产生屋顶遮挡"
    ); // 深度预通道必须保留真实几何开口。
    let roof_depth = depth_at(&capture.bytes, matrix, Vec3::new(-3.0, 0.0, 0.0)); // 屋顶中心测试样本。
    let expected_depth = matrix.project_point3(Vec3::new(-3.0, 4.1, 0.0)).z; // Cuboid上表面真实y=4.1。
    assert!(
        (roof_depth - expected_depth).abs() < 0.001,
        "屋顶深度与真实几何不符：{roof_depth} vs {expected_depth}"
    ); // 不能由测试标记或错误平面凑出非零值。
    renderer_transition(&mut app, matrix, OpaqueRendererMethod::Deferred, 5.0); // F12关闭恢复Deferred，同时验证禁用相机重新登记阶段。
    renderer_transition(&mut app, matrix, OpaqueRendererMethod::Forward, 4.0); // 再回Forward也必须恢复两个材质队列，不遗留旧深度。
    let before_removal = app.world().resource::<Captured>().serial; // 删除前最后一个已完成样本身份。
    let roofs = app
        .world_mut()
        .query_filtered::<Entity, With<TestRoof>>()
        .iter(app.world())
        .collect::<Vec<_>>(); // 测试仅四个固定实体。
    assert_eq!(roofs.len(), 4); // 场景完整，不能漏创建屋顶。
    for roof in roofs {
        app.world_mut().despawn(roof);
    } // 真实移除网格，生产预通道应清空。
    loop {
        // 异步Readback可能先交付删除前已经在途的旧帧。
        step(&mut app); // 继续驱动真实GPU，而非直接清目标纹理。
        let capture = app.world().resource::<Captured>(); // 每次只读取完整回调。
        if capture.serial > before_removal
            && depth_at(&capture.bytes, matrix, Vec3::new(-3.0, 0.0, 0.0)) == 0.0
        {
            break;
        } // 删除后的空深度证明捕获不残留屋顶。
        assert!(Instant::now() < deadline, "删除屋顶后GPU缓存仍残留遮挡"); // 失败不能以旧有效metadata掩盖。
    } // 动态移除验证结束。
} // 真实GPU烟雾结束，无文件输出或游戏窗口。
