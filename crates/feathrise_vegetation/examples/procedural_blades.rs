//! 无外部资产的公共 GPU 叶片示例。
//!
//! 叶片网格、综合色纹理、高度图和密度图均在启动时程序化生成。

use std::time::Duration; // 无窗口自动烟雾使用固定帧间隔推进RenderApp。

use bevy::app::{AppExit, ScheduleRunnerPlugin}; // 自动烟雾到时以成功状态退出，无窗口模式显式驱动帧循环。
use bevy::asset::RenderAssetUsages; // 程序资产同时保留在主世界并上传渲染世界。
use bevy::camera::{
    RenderTarget,           // 自动烟雾把唯一相机指向离屏Image，彻底避开隐藏交换链。
    primitives::Aabb,       // 叶片窗口使用完整候选范围参与视锥剔除。
    visibility::NoAutoAabb, // 禁止单片叶片的局部范围覆盖手工窗口包围盒。
};
use bevy::core_pipeline::prepass::{DeferredPrepass, DepthPrepass, MotionVectorPrepass}; // 烟雾覆盖真实历史位置与Solari同型四附件入口。
use bevy::mesh::Indices; // 程序叶片使用显式三角形索引。
use bevy::prelude::*; // 应用、场景、网格、材质、灯光和资源类型。
use bevy::render::{
    RenderPlugin, // 自动烟雾同步编译全部GPU管线，有限运行时间不落在异步等待窗口。
    render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
}; // 程序网格、三张输入纹理与离屏目标的底层格式。
use bevy::window::{ExitCondition, WindowPlugin}; // 自动烟雾不创建窗口，普通示例仍保持可观察。
use bevy::winit::WinitPlugin; // 无窗口自动烟雾禁用操作系统事件循环并改用固定帧调度器。
use feathrise_vegetation::blade::{
    GPU_GRASS_PATCH_AABB_HORIZONTAL_MARGIN, GpuBladeConfig, GpuBladeDensityMap, GpuBladeHeightMap,
    GpuBladePatch, GpuBladePlugin, GpuBladeProfile, GpuBladeProfileSlot, GpuBladeProfiles,
    GpuBladeRenderAssets, GpuBladeView,
}; // 示例只依赖叶片后端和无项目语义的配置API。

const PATCH_EXTENT: f32 = 64.0; // 单个静态窗口覆盖六十四米见方的程序地表。
const GRID_SIDE: u32 = 320; // 十万级候选足够展示GPU压缩与间接绘制，同时保持示例轻量。
const HEIGHT_MIN: f32 = -1.0; // 程序高度UNORM的米制解码下界。
const HEIGHT_MAX: f32 = 1.0; // 程序高度UNORM的米制解码上界。

fn main() {
    let smoke_seconds = std::env::var("VEGETATION_BLADE_PUBLIC_SMOKE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0); // 未设置或非法值保持正常交互示例。
    let automatic = smoke_seconds.is_some(); // 有限烟雾必须走真正无窗口的离屏路径，不能只隐藏交换链。
    let mut app = App::new();
    let mut plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: (!automatic).then(Window::default), // 交互模式保留主窗口，自动模式不创建交换链。
        exit_condition: if automatic {
            ExitCondition::DontExit // 无窗口会话只由墙钟计时器发送AppExit。
        } else {
            ExitCondition::OnPrimaryClosed // 手工窗口关闭后沿用Bevy标准退出语义。
        },
        ..default()
    });
    if automatic {
        plugins = plugins
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true, // 六秒验收必须覆盖已完成专用化的真实GPU管线。
                ..default()
            })
            .disable::<WinitPlugin>(); // 不创建窗口或呈现未写入的隐藏swapchain图像。
    }
    app.add_plugins((plugins, GpuBladePlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, exit_public_smoke);
    if let Some(seconds) = smoke_seconds {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))); // 无Winit时按约六十帧每秒推进主世界与RenderApp。
        app.insert_resource(PublicSmokeTimer(Timer::from_seconds(
            seconds,
            TimerMode::Once,
        ))); // CI可用有限秒数验证内嵌shader和程序资产管线能够实际启动。
    }
    app.run();
}

/// 可选公共烟雾的墙钟退出计时器。
#[derive(Resource)]
struct PublicSmokeTimer(Timer);

/// 到达公共烟雾时限后返回成功；严格GPU计数继续由项目validation包负责。
fn exit_public_smoke(
    timer: Option<ResMut<PublicSmokeTimer>>,
    time: Res<Time<Real>>,
    mut exits: MessageWriter<AppExit>,
) {
    let Some(mut timer) = timer else {
        return; // 普通clone后运行保持交互窗口，不自行退出。
    };
    if timer.0.tick(time.delta()).just_finished() {
        exits.write(AppExit::Success); // 程序资产和内嵌shader至少完成指定时长的真实应用运行。
    }
}

/// 创建完全程序化的叶片窗口、地表、相机与灯光。
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut render_assets: ResMut<GpuBladeRenderAssets>,
    mut profiles: ResMut<GpuBladeProfiles>,
    smoke_timer: Option<Res<PublicSmokeTimer>>, // 资源存在即与main保持一致，创建无窗口离屏目标。
) {
    let blade_mesh = meshes.add(create_blade_mesh()); // 原型只有一片竖直四边形，不依赖GLB。
    let blade_texture = images.add(create_blade_texture()); // 综合色与透明度由程序像素提供。
    let height_cells = images.add(create_flat_height_cells()); // 一个RGBA16像素覆盖整个窗口。
    let density_map = images.add(create_density_map()); // 八乘八棋盘概率图覆盖Compute纹理路径。

    render_assets.set_blade_base_color(Some(blade_texture)); // 消费者显式选择程序综合色纹理。
    profiles.set(
        GpuBladeProfileSlot::SLOT_0,
        GpuBladeProfile::new(
            Vec4::new(0.65, 1.05, 0.03, 0.06),   // 株高、根部下扎和静态倾斜。
            Vec4::new(0.75, 1.30, 0.55, 0.90),   // 宽窄、柔韧度和收尖。
            Vec4::new(0.36, 0.46, 0.95, 0.90),   // 风摆与局部交互响应。
            Vec4::new(1.0, 1.0, 0.0, 0.0),       // 使用整张程序综合色纹理。
            Vec4::new(0.012, 0.035, 0.006, 0.0), // 根部线性深绿色。
            Vec4::new(0.090, 0.220, 0.035, 0.0), // 叶尖线性亮绿色。
            Vec4::new(0.90, 1.02, 1.08, 0.0),    // 冷色世界扰动。
            Vec4::new(1.10, 1.00, 0.82, 0.0),    // 暖色世界扰动。
        ),
    ); // 公共库默认中性表由示例按零号槽显式覆盖。

    let half_extent = PATCH_EXTENT * 0.5; // 高度、密度和手工AABB共享同一世界范围。
    let blade_config = GpuBladeConfig::new(GpuBladeProfileSlot::SLOT_0, 1.05, 0.70, 0.80, 0.03); // 四槽ABI只记录槽号，内容含义完全由消费者决定。
    let patch = GpuBladePatch::square(Vec3::ZERO, PATCH_EXTENT, GRID_SIDE, 0x4752_4153)
        .with_visible_radius(30.0)
        .with_vegetation_config(blade_config)
        .with_height_field(
            Vec2::splat(-half_extent),
            Vec2::splat(half_extent),
            HEIGHT_MIN,
            HEIGHT_MAX,
        )
        .with_density_field(Vec2::splat(-half_extent), Vec2::splat(half_extent)); // CPU只提交一份固定窗口，逐候选筛选与压缩全部留在GPU。

    commands.spawn((
        Name::new("ProceduralGpuBlades"),
        Mesh3d(blade_mesh),
        patch,
        GpuBladeHeightMap::new(height_cells),
        GpuBladeDensityMap::new(density_map),
        Transform::default(),
        Aabb {
            center: Vec3A::new(0.0, 0.52, 0.0),
            half_extents: Vec3A::new(
                half_extent + GPU_GRASS_PATCH_AABB_HORIZONTAL_MARGIN,
                0.58,
                half_extent + GPU_GRASS_PATCH_AABB_HORIZONTAL_MARGIN,
            ),
        },
        NoAutoAabb,
    )); // 完成公共叶片后端的唯一窗口实体。

    commands.spawn((
        Name::new("ProceduralGround"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(PATCH_EXTENT, PATCH_EXTENT))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.055, 0.085, 0.035),
            perceptual_roughness: 0.95,
            ..default()
        })),
    )); // 普通PBR平面仅提供观察参照；地表材质不属于叶片后端。

    let camera_target = smoke_timer.is_some().then(|| {
        let handle = images.add(Image::new_target_texture(
            640,                           // 固定轻量宽度足以覆盖程序叶片管线。
            640,                           // 正方形离屏附件保持当前相机构图稳定。
            TextureFormat::Rgba8UnormSrgb, // 与普通窗口颜色输出使用相同标准颜色空间。
            None,                          // 单样本目标与相机Msaa::Off一致。
        )); // Image资产和相机组件共同保持强句柄，RenderApp实际执行完整颜色通道。
        RenderTarget::Image(handle.into()) // 自动模式只写离屏纹理，不执行swapchain present。
    });
    let mut camera = commands.spawn((
        Name::new("BladeCamera"),
        Camera3d::default(),
        GpuBladeView::legacy(),
        Transform::from_xyz(10.0, 8.0, 14.0).looking_at(Vec3::new(0.0, 0.4, 0.0), Vec3::Y),
        Msaa::Off,           // Deferred烟雾显式使用兼容的单采样附件组合。
        DepthPrepass,        // AlphaMask叶片把当前动态姿态写入深度预通道。
        MotionVectorPrepass, // 上一帧风场与交互姿态必须在真实GPU管线中完成校验。
        DeferredPrepass,     // 模拟Solari视图，在四附件路径写草叶GBuffer、深度与真实运动矢量。
    )); // 兼容视图使用窗口自己的单圈可见范围。
    if let Some(target) = camera_target {
        camera.insert(target); // 无窗口烟雾显式覆盖相机目标；交互模式继续使用PrimaryWindow。
    }
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 16_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, -0.8, -0.9, 0.0)),
    )); // 双面叶片通过方向光和实时阴影展示风摆轮廓。
}

/// 生成带根尖UV的双三角形竖直叶片原型。
fn create_blade_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.045, 0.0, 0.0],
            [0.045, 0.0, 0.0],
            [0.028, 1.0, 0.0],
            [-0.028, 1.0, 0.0],
        ],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

/// 生成四像素绿色综合色纹理，alpha保持完全不透明。
fn create_blade_texture() -> Image {
    Image::new(
        Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![
            32, 92, 18, 255, 44, 126, 24, 255, 56, 148, 30, 255, 72, 174, 38, 255,
        ],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// 生成四角同高的单像素RGBA16高度单元。
fn create_flat_height_cells() -> Image {
    let encoded = (0.5 * u16::MAX as f32).round() as u16; // 零米位于负一到正一解码区间中点。
    let mut pixels = Vec::with_capacity(8);
    for _ in 0..4 {
        pixels.extend_from_slice(&encoded.to_le_bytes());
    }
    Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba16Unorm,
        RenderAssetUsages::default(),
    )
}

/// 生成全程序化的八乘八密度图；边缘较疏、中心较密。
fn create_density_map() -> Image {
    let mut pixels = Vec::with_capacity(8 * 8 * 4);
    for z in 0_i32..8 {
        for x in 0_i32..8 {
            let edge = x.min(7 - x).min(z.min(7 - z));
            let density = if edge == 0 { 96 } else { 224 };
            pixels.extend_from_slice(&[density, density, density, 255]);
        }
    }
    Image::new(
        Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    )
}
