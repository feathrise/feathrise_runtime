//! 使用程序网格或仓库自有中性GLB的 Mesh 风渲染烟雾示例。
//!
//! 程序网格和贴图同时覆盖详细/低质量风、Opaque输入兼容、AlphaMask、方向光阴影、深度、延迟和运动向量预通道。

use std::{path::PathBuf, time::Duration}; // 离屏截图保存路径与无窗口帧循环间隔只服务自动烟雾。

use bevy::app::{AppExit, ScheduleRunnerPlugin}; // 自动烟雾到时以成功状态退出，无窗口模式显式驱动帧循环。
use bevy::asset::{AssetPlugin, RenderAssetUsages}; // 可选加载中性树形夹具GLB，程序网格与纹理同时保留主世界身份并上传RenderApp。
use bevy::camera::{
    CameraMainTextureUsages,
    RenderTarget, // 可选Solari存储写入与自动离屏Image目标使用官方相机组件。
    primitives::{Aabb, MeshAabb}, // 程序边界与夹具GLB网格边界共同决定完整树高参考。
    visibility::{NoAutoAabb, VisibilityRange}, // 手工动态扩边与生产同型GPU距离LOD变体同时接受像素验证。
}; // 完成相机剔除类型导入。
use bevy::core_pipeline::prepass::{
    DeferredPrepass, DeferredPrepassDoubleBuffer, DepthPrepass, DepthPrepassDoubleBuffer,
    MotionVectorPrepass,
}; // 一次触发Solari同型深度、GBuffer双缓冲、历史位置和延迟变体。
use bevy::material::OpaqueRendererMethod; // 两份叶簇材质分别强制前向和延迟主路径。
use bevy::mesh::Indices; // 交叉叶簇程序网格使用显式三角形索引。
use bevy::prelude::*; // 应用、场景、材质、灯光、时间与资源类型。
use bevy::render::{
    RenderPlugin, // 离屏烟雾同步编译全部管线，截图帧不会落在异步等待窗口。
    render_resource::{
        Extent3d, PrimitiveTopology, TextureDimension, TextureFormat, TextureUsages,
    }, // 程序资产底层格式与离屏截图复制能力。
    view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk}, // 可选把真实颜色附件保存为像素证据。
}; // 完成离屏渲染与截图类型导入。
use bevy::window::{ExitCondition, WindowPlugin}; // 自动烟雾不创建窗口，手工运行仍显示场景。
use bevy::winit::WinitPlugin; // 无窗口自动烟雾禁用操作系统事件循环并改用固定帧调度器。
use feathrise_environment::mesh_wind::{
    MeshWindLodLevel,                // 明确覆盖高档详细和低档宏观分支。
    MeshWindPart,                    // 两份程序叶簇都使用Foliage身份。
    MeshWindPlugin,                  // 安装自有噪声、Shader和具体扩展材质。
    TreeWindMaterial,                // 保存并逐帧更新三份共享风材质资产。
    TreeWindNoiseTexture,            // 等待Startup完整发布程序风噪声后再创建材质。
    combine_tree_part_aabbs,         // 夹具树干与树冠使用生产相同联合树高边界。
    create_tree_wind_material,       // 从程序StandardMaterial构造自有扩展材质。
    expanded_tree_wind_culling_aabb, // 手工边界覆盖最大GPU位移。
    update_tree_wind_material,       // 每帧推进当前与上一帧风参数。
}; // 完成公开Mesh风API导入。
use bevy_solari::prelude::{SolariLighting, SolariPlugins}; // 可选复现场景Solari节点与SkipDeferredLighting合成顺序。

const SPECIMEN_HEIGHT: f32 = 4.0; // 两份程序叶簇共用四米完整高度。
const SPECIMEN_HALF_WIDTH: f32 = 1.2; // 交叉卡片在X/Z方向各延伸一米二。

fn main() {
    // 设置正秒数时进入隐藏窗口自动烟雾；未设置时保持可交互观察。
    let smoke_seconds = std::env::var("MESH_WIND_SMOKE_SECONDS")
        .ok() // 环境变量缺失时继续普通示例。
        .and_then(|value| value.parse::<f32>().ok()) // 非数字输入不启用自动退出。
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0); // 只接受有限正时长。
    let capture_path = std::env::var_os("MESH_WIND_SMOKE_CAPTURE").map(PathBuf::from); // 设置路径时保存离屏颜色附件并在完成后退出。
    let real_tree = std::env::var_os("MESH_WIND_SMOKE_REAL_TREE").is_some(); // 保留原启动开关，现只加载本仓中性双部件GLB。
    let real_tree_instances = std::env::var("MESH_WIND_SMOKE_INSTANCES")
        .ok() // 环境变量缺失时保留单树像素基线。
        .and_then(|value| value.parse::<usize>().ok()) // 非正整数字符串不改变默认实例数。
        .filter(|count| (1..=32).contains(count)) // 限制正式高模批处理烟雾的GPU工作量。
        .unwrap_or(1); // 默认单实例继续隔离材质、网格和Solari静态组合。
    let solari = std::env::var_os("MESH_WIND_SMOKE_SOLARI").is_some(); // 显式设置时安装官方Solari并把测试相机接入实时光照。
    let automatic = smoke_seconds.is_some() || capture_path.is_some(); // 有时限或截图请求都必须走无窗口确定性路径。
    let mut plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: (!automatic).then(|| Window {
            title: "Mesh Wind Smoke".into(), // 手工运行时明确当前验证场景。
            ..default()                      // 尺寸和呈现模式沿用Bevy稳定默认值。
        }), // 自动模式不创建交换链，手工模式保留唯一主窗口。
        exit_condition: if automatic {
            ExitCondition::DontExit // 无窗口会话由截图或墙钟计时器发送AppExit。
        } else {
            ExitCondition::OnPrimaryClosed // 手工窗口关闭后沿用Bevy标准退出语义。
        }, // 完成有窗与无窗退出条件选择。
        ..default() // 不创建额外窗口。
    }); // 完成默认插件组配置。
    if automatic {
        plugins = plugins
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true, // 离屏像素必须来自全部专用管线已完成的帧。
                ..default() // 适配器、设备与其余渲染选项沿用项目当前后端。
            }) // 完成自动烟雾同步管线配置。
            .disable::<WinitPlugin>(); // 无窗口时不安装操作系统窗口事件循环。
    } // 手工模式继续使用默认异步编译与Winit事件循环。
    if real_tree {
        plugins = plugins.set(AssetPlugin {
            file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/examples/fixtures").into(), // 独立SDK检出即可找到自有夹具，不依赖运行目录或原游戏。
            ..default()                       // 处理器、观察器与元数据行为沿用项目当前Bevy配置。
        }); // 让GLB夹具烟雾使用当前SDK仓库自有的GLB与内嵌纹理。
    } // 程序烟雾保持无外部资产依赖。
    let mut app = App::new(); // 显式构造便于按可选时长插入资源。
    app.add_plugins((plugins, MeshWindPlugin)) // 先安装RenderApp与自有风材质管线。
        .add_systems(
            Startup,
            (setup_static_scene, request_real_tree_assets), // 静态场景与可选夹具GLB请求都不依赖风噪声资源。
        ) // 完成Startup场景和源资产请求注册。
        .add_systems(
            Update,
            (
                setup_wind_specimens,     // 噪声资源就绪后一次创建叶簇与茎干程序样本。
                setup_real_tree_specimen, // 可选等待夹具GLB后创建高档树干与树冠。
                animate_wind_specimens,   // 后续每帧推进三份共享材质历史。
                capture_automatic_smoke,  // 可选在完整渲染预热后保存离屏颜色附件。
                exit_automatic_smoke,     // 可选计时完成后返回成功。
            )
                .chain(), // 固定创建、更新、退出顺序便于烟雾复现。
        ); // 完成程序烟雾系统注册。
    if solari {
        app.add_plugins(SolariPlugins); // 安装与公共场景相同的官方实时光照和光追场景插件组。
    } // 普通烟雾不承担实验光追资源与管线成本。
    if automatic {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))); // 无Winit会话仍按约六十帧每秒推进RenderApp。
    } // 手工窗口继续由Winit拥有应用Runner。
    if let Some(seconds) = smoke_seconds {
        app.insert_resource(SmokeTimer(Timer::from_seconds(seconds, TimerMode::Once))); // GPU后端验证使用有限墙钟时长。
    } // 普通示例不插入计时器，因此不会自行退出。
    if let Some(path) = capture_path {
        app.insert_resource(SmokeCaptureRequest {
            path,             // 调用方决定PNG输出位置，示例不覆盖固定项目文件。
            requested: false, // 等待至少两个真实秒后只提交一次截图请求。
        }); // 原子安装本次离屏截图会话。
    } // 未设置路径时自动烟雾只验证GPU运行与零退出。
    if real_tree {
        app.insert_resource(RealTreeSmoke); // 启用夹具高档树干与树冠的隔离像素场景。
        app.insert_resource(RealTreeInstanceCount(real_tree_instances)); // 可选多实例模式复现主世界PBR合批与实例矩阵索引。
    } // 默认继续使用三张程序交叉卡片。
    if solari {
        app.insert_resource(SolariSmoke); // Startup据此只给本次离屏相机安装Solari必需附件。
    } // 默认相机继续使用标准Deferred光照节点。
    app.run(); // 启动真实窗口、RenderApp和选定WGPU后端。
} // 完成程序Mesh风烟雾入口。

/// 自动烟雾的墙钟退出计时器。
#[derive(Resource)] // 只在设置环境变量时存在。
struct SmokeTimer(Timer); // 使用Bevy真实时间推进，避免暂停虚拟时间影响退出。

/// 自动烟雾可选的离屏截图请求。
#[derive(Resource)] // 主世界持有路径与一次性提交门，不进入RenderApp。
struct SmokeCaptureRequest {
    path: PathBuf,   // 保存当前平台真实颜色附件的PNG路径。
    requested: bool, // 截图实体提交后置真，防止GPU回读期间重复请求。
} // 完成离屏截图请求资源。

/// 自动烟雾相机使用的离屏颜色目标。
#[derive(Resource)] // 截图系统通过强句柄保持Image存活并精确指定回读目标。
struct SmokeRenderTarget(Handle<Image>); // 单个六百四十乘六百四十RGBA8目标足够识别三份样本。

/// 使用自有GLB树干与叶卡替代程序卡片的显式测试模式。
#[derive(Resource)] // 仅作为有限分支标记，不保存或复制任何大资产数据。
struct RealTreeSmoke; // 环境变量存在时由main一次插入。

/// GLB夹具烟雾生成的有限实例数。
#[derive(Resource)] // 只保存一至三十二的小整数，不随帧或资产加载变化。
struct RealTreeInstanceCount(usize); // 多实例模式用于验证PBR合批后的实例索引与世界矩阵。

/// 为离屏测试相机启用官方Solari节点的显式模式。
#[derive(Resource)] // 只作为Startup固定分支，不复制Solari内部状态。
struct SolariSmoke; // 环境变量存在时由main一次插入。

/// GLB夹具烟雾需要保持存活的四个异步源资产句柄。
#[derive(Resource)] // 强句柄保证等待期间GLB子资产及其递归纹理不会被回收。
struct RealTreeSourceAssets {
    trunk_mesh: Handle<Mesh>,                  // 中性树形夹具高档Mesh0树干primitive。
    canopy_mesh: Handle<Mesh>,                 // 中性树形夹具高档Mesh1树冠primitive。
    trunk_material: Handle<StandardMaterial>,  // Material0/std程序树皮PBR源。
    canopy_material: Handle<StandardMaterial>, // Material1/std程序叶簇PBR源。
} // 完成GLB夹具源资产会话。

/// 三份共享风材质句柄；每帧只更新它们而不遍历程序实体。
#[derive(Resource)] // 资源存在同时表示一次性程序场景已经完成创建。
struct SmokeWindMaterials {
    detailed: Handle<TreeWindMaterial>, // 高档叶簇启用微摆、S曲线和上下起伏。
    low: Handle<TreeWindMaterial>,      // 低档叶簇只保留宏观树高弯曲。
    stem: Handle<TreeWindMaterial>,     // Opaque输入茎干覆盖Bevy 0.19.1原生预通道材质绑定路径。
} // 完成烟雾共享材质资源。

/// 创建不依赖风噪声的地面、相机和方向光。
#[allow(clippy::too_many_arguments)] // 独立烟雾示例显式注入各资产表与可选运行模式，便于逐项验证渲染资源。
fn setup_static_scene(
    mut commands: Commands,                          // 登记静态烟雾实体。
    mut meshes: ResMut<Assets<Mesh>>,                // 程序地面平面加入网格资产表。
    mut images: ResMut<Assets<Image>>,               // 自动模式创建可复制的离屏颜色目标。
    mut materials: ResMut<Assets<StandardMaterial>>, // 地面继续使用普通PBR材质。
    timer: Option<Res<SmokeTimer>>,                  // 有限时长烟雾必须避开隐藏窗口交换链。
    capture: Option<Res<SmokeCaptureRequest>>,       // 显式截图请求同样需要离屏相机目标。
    real_tree: Option<Res<RealTreeSmoke>>, // 夹具局部高31米，缩放0.42后世界高13.02米。
    solari: Option<Res<SolariSmoke>>,      // 可选给唯一相机安装官方Solari入口与存储用途。
) {
    let real_tree = real_tree.is_some(); // 把可选资源收敛为本次Startup固定场景分支。
    let ground_size = if real_tree { 160.0 } else { 14.0 }; // 固定大地面完整承接夹具风动阴影。
    commands.spawn((
        Name::new("MeshWindSmokeGround"), // 日志与世界检查器中的稳定身份。
        Mesh3d(meshes.add(Plane3d::default().mesh().size(ground_size, ground_size))), // 当前尺度地面接收动态树影。
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.07, 0.09, 0.055), // 深色粗糙地面突出叶簇轮廓。
            perceptual_roughness: 0.95,                 // 避免镜面高光干扰阴影观察。
            ..default()                                 // 其余标准PBR参数保持默认。
        })), // 完成地面材质登记。
    )); // 完成程序地面实体。
    let automatic = timer.is_some() || capture.is_some(); // 与main中的无窗口插件选择保持同一有限条件。
    let camera_target = automatic.then(|| {
        let mut target = Image::new_target_texture(
            640,                           // 固定宽度便于跨后端像素比较。
            640,                           // 固定高度保持三份四米样本完整入镜。
            TextureFormat::Rgba8UnormSrgb, // 截图直接保存标准颜色空间PNG。
            None,                          // 单样本离屏附件与相机Msaa::Off一致。
        ); // 创建供相机写入的GPU目标。
        target.texture_descriptor.usage |= TextureUsages::COPY_SRC; // Screenshot需要从颜色附件复制到回读缓冲。
        let handle = images.add(target); // 资产系统负责把Image准备到RenderApp。
        commands.insert_resource(SmokeRenderTarget(handle.clone())); // 保持强句柄并发布给截图系统。
        RenderTarget::Image(handle.into()) // 返回可直接作为Camera目标组件的类型。
    }); // 手工模式继续隐式使用PrimaryWindow。
    let camera_transform = if real_tree {
        Transform::from_xyz(24.0, 17.0, 30.0).looking_at(Vec3::new(0.0, 6.5, 0.0), Vec3::Y) // 对准缩放后13.02米夹具的中部，保留地面阴影。
    } else {
        Transform::from_xyz(8.0, 5.5, 10.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y) // 同时观察三份程序风材质样本。
    }; // 完成程序或GLB夹具相机选择。
    let mut camera = commands.spawn((
        Name::new("MeshWindSmokeCamera"), // 明确这是烟雾验证相机。
        Camera3d::default(),              // 使用标准三维相机主视图。
        camera_transform,                 // 使用当前场景尺度对应的固定观察姿态。
        Msaa::Off,                        // 延迟预通道和跨后端烟雾使用最兼容配置。
        DepthPrepass,                     // AlphaMask和当前帧风姿态进入深度预通道。
        DepthPrepassDoubleBuffer,         // 复现Solari相机保留上一帧深度附件的必需组件。
        MotionVectorPrepass,              // 上一帧风参数与上一帧世界变换必须被Shader读取。
        DeferredPrepass,                  // 低档材质强制走延迟顶点入口。
        DeferredPrepassDoubleBuffer,      // 复现Solari相机当前与上一帧GBuffer附件轮换。
    )); // 完成覆盖前向、预通道和延迟的相机实体。
    if let Some(target) = camera_target {
        camera.insert(target); // 自动模式只写离屏Image，不创建或呈现交换链。
    } // 手工模式不插入目标组件并沿用PrimaryWindow。
    if solari.is_some() {
        camera.insert((
            CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING), // Solari计算节点必须存储写入主颜色纹理。
            SolariLighting::default(), // required组件补齐HDR与同型预通道，并让RenderApp跳过标准DeferredLighting。
        )); // 完成唯一烟雾相机Solari接入。
    } // 普通模式保留标准光栅Deferred最终照明。
    commands.spawn((
        Name::new("MeshWindSmokeSun"), // 稳定方向光身份便于日志定位。
        DirectionalLight {
            illuminance: 16_000.0,     // 足够亮度展示StandardMaterial完整PBR响应。
            shadow_maps_enabled: true, // 强制创建并编译风动阴影管线。
            ..default()                // 颜色和阴影软化沿用Bevy默认值。
        }, // 完成方向光参数。
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, -0.7, -0.9, 0.0)), // 斜向光把风动轮廓投射到地面。
    )); // 完成带动态阴影的方向光。
} // 完成烟雾静态场景创建。

/// GLB夹具模式一次请求仓库自有的高档网格与两份PBR源材质。
fn request_real_tree_assets(
    mut commands: Commands,         // 四个强句柄完整建立后原子发布等待资源。
    asset_server: Res<AssetServer>, // 使用AssetPlugin配置的示例自有资产根。
    real_tree: Option<Res<RealTreeSmoke>>, // 默认程序模式不触发中性树形夹具GLB加载。
) {
    if real_tree.is_none() {
        return; // 无显式模式标记时保持烟雾完全无外部资产依赖。
    } // 完成GLB夹具加载门。
    commands.insert_resource(RealTreeSourceAssets {
        trunk_mesh: asset_server.load("wind_tree.glb#Mesh0/Primitive0"), // 自有夹具第一个网格为树干。
        canopy_mesh: asset_server.load("wind_tree.glb#Mesh1/Primitive0"), // 自有夹具第二个网格为叶卡。
        trunk_material: asset_server.load("wind_tree.glb#Material0/std"), // 使用程序树皮与Bevy标准材质子标签。
        canopy_material: asset_server.load("wind_tree.glb#Material1/std"), // 使用程序叶卡与Bevy标准材质子标签。
    }); // 完成GLB夹具四项强句柄发布。
} // 完成GLB夹具源资产请求。

/// 风噪声Startup资源就绪后一次创建高档前向与低档延迟叶簇。
fn setup_wind_specimens(
    mut commands: Commands, // 创建叶簇与茎干程序样本并原子发布共享材质句柄。
    noise: Option<Res<TreeWindNoiseTexture>>, // Startup首帧可能尚未应用资源命令。
    ready: Option<Res<SmokeWindMaterials>>, // 资源存在时立即跳过，禁止重复创建。
    mut meshes: ResMut<Assets<Mesh>>, // 交叉叶簇程序网格只登记一份并共享。
    mut images: ResMut<Assets<Image>>, // AlphaMask程序纹理只登记一份并共享。
    mut materials: ResMut<Assets<TreeWindMaterial>>, // 三份具体扩展材质加入同一资产表。
    real_tree: Option<Res<RealTreeSmoke>>, // GLB夹具模式由相邻系统创建真实高档部件。
) {
    if ready.is_some() || real_tree.is_some() {
        return; // 地图式一次性门确保Update不会重复分配程序资产。
    } // 完成重复创建防护。
    let Some(noise) = noise else {
        return; // 仅非阻塞等待Startup原子发布完整噪声句柄。
    }; // 取得项目自有共享风噪声资源。
    let alpha_texture = images.add(create_alpha_mask_texture()); // 创建带透明边缘的程序叶色纹理。
    let mesh = meshes.add(create_cross_foliage_mesh()); // 一份交叉卡片网格供三份风材质共享。
    let source_aabb = Aabb::from_min_max(
        Vec3::new(-SPECIMEN_HALF_WIDTH, 0.0, -SPECIMEN_HALF_WIDTH), // 地表根部和水平最小边界。
        Vec3::new(SPECIMEN_HALF_WIDTH, SPECIMEN_HEIGHT, SPECIMEN_HALF_WIDTH), // 四米叶簇顶部与水平最大边界。
    ); // 完成三份材质共享的完整树高参考。
    let culling_aabb = expanded_tree_wind_culling_aabb(source_aabb); // 手工视锥边界覆盖满风最大位移。
    let detailed = materials.add(create_tree_wind_material(
        foliage_base_material(alpha_texture.clone(), OpaqueRendererMethod::Forward), // 高档强制走主前向顶点入口。
        source_aabb,            // 单部件程序叶簇自身就是完整树高。
        &noise,                 // 复用Startup唯一四通道周期风噪声。
        MeshWindLodLevel::High, // 启用详细叶簇专用化定义。
        MeshWindPart::Foliage,  // 覆盖微摆、S曲线和上下起伏。
    )); // 完成高档前向AlphaMask材质。
    let low = materials.add(create_tree_wind_material(
        foliage_base_material(alpha_texture, OpaqueRendererMethod::Deferred), // 低档强制编译延迟顶点入口。
        source_aabb,           // 与高档共用相同树高，宏观轮廓可直接比较。
        &noise,                // 复用同一风场纹理和采样器。
        MeshWindLodLevel::Low, // 专用化关闭全部详细叶簇计算。
        MeshWindPart::Foliage, // 低档叶簇只保留宏观弯曲。
    )); // 完成低档延迟AlphaMask材质。
    let stem = materials.add(create_tree_wind_material(
        StandardMaterial {
            base_color: Color::srgb(0.22, 0.09, 0.035), // 无纹理不透明棕色明确模拟树干输入。
            perceptual_roughness: 0.9,                  // 粗糙木质表面继续走标准PBR。
            double_sided: true,                         // 与正式占位树干的双面校准一致。
            ..default() // 默认Opaque在Bevy 0.19.1自定义预通道中保留扩展材质绑定。
        }, // 完成原始不透明StandardMaterial输入。
        source_aabb,            // 程序卡片仍覆盖完整四米树高。
        &noise,                 // 复用Startup唯一风噪声资源。
        MeshWindLodLevel::High, // 茎干身份只启用宏观弯曲，不编译局部细摆。
        MeshWindPart::Stem,     // 精确覆盖夹具树干的材质与预通道组合。
    )); // 完成Bevy 0.19.1不透明输入预通道原生材质。
    commands.spawn((
        Name::new("MeshWindDetailedForward"), // 高档前向测试实体身份。
        Mesh3d(mesh.clone()),                 // 共享同一程序网格资产。
        MeshMaterial3d(detailed.clone()),     // 同档实体只引用共享材质句柄。
        Transform::from_xyz(-2.0, 0.0, 0.0),  // 左侧展示详细风分支。
        culling_aabb,                         // 覆盖宏观与局部最大顶点位移。
        NoAutoAabb,                           // 禁止静止网格边界覆盖手工风动边界。
    )); // 完成高档前向叶簇实体。
    commands.spawn((
        Name::new("MeshWindLowDeferred"),   // 低档延迟测试实体身份。
        Mesh3d(mesh.clone()),               // 继续共享同一程序网格资产。
        MeshMaterial3d(low.clone()),        // 同档实体只引用共享材质句柄。
        Transform::from_xyz(2.0, 0.0, 0.0), // 右侧展示仅宏观分支。
        culling_aabb,                       // 使用相同保守扩边便于比较。
        NoAutoAabb,                         // 禁止自动边界丢失风动余量。
    )); // 完成低档延迟叶簇实体。
    commands.spawn((
        Name::new("MeshWindOpaqueStem"), // 精确覆盖Bevy 0.19.1纯深度Opaque原生绑定路径。
        Mesh3d(mesh),                    // 复用程序网格即可验证材质绑定与顶点位移。
        MeshMaterial3d(stem.clone()),    // CPU/GPU都保持Opaque的共享风材质。
        Transform::from_xyz(0.0, 0.0, 0.5), // 放在两份叶簇之间便于手工模式观察。
        culling_aabb,                    // 同样覆盖满风位移。
        NoAutoAabb,                      // 保留项目手工风动边界。
    )); // 完成不透明输入茎干烟雾实体。
    commands.insert_resource(SmokeWindMaterials {
        detailed,
        low,
        stem,
    }); // 三份材质完整创建后原子发布逐帧更新资源。
} // 完成程序风动叶簇创建。

/// 夹具GLB四项源与风噪声全部就绪后创建一棵高档TreeWind树。
#[allow(clippy::too_many_arguments)] // 加载门禁、实例规模、四类资产和共享材质必须在同一原子生成系统中汇合。
fn setup_real_tree_specimen(
    mut commands: Commands, // 树干、树冠与三份共享风材质完整后原子发布就绪资源。
    real_tree: Option<Res<RealTreeSmoke>>, // 只有显式GLB夹具模式承担资产查询成本。
    instance_count: Option<Res<RealTreeInstanceCount>>, // 单树基线或有限多实例PBR合批规模。
    source: Option<Res<RealTreeSourceAssets>>, // Startup已请求的四个强句柄。
    noise: Option<Res<TreeWindNoiseTexture>>, // MeshWindPlugin生成的唯一程序风噪声。
    ready: Option<Res<SmokeWindMaterials>>, // 资源存在时禁止重复创建实体或材质。
    meshes: Res<Assets<Mesh>>, // 读取高档两份网格并计算真实联合树高边界。
    standard_materials: Res<Assets<StandardMaterial>>, // 克隆夹具的树皮与叶卡PBR源。
    mut wind_materials: ResMut<Assets<TreeWindMaterial>>, // 创建三份具体扩展材质供更新与截图。
) {
    if real_tree.is_none() || ready.is_some() {
        return; // 默认模式或已完成会话不触碰夹具资产表。
    } // 完成模式与一次性门。
    let (Some(source), Some(noise)) = (source, noise) else {
        return; // 非阻塞等待Startup命令与程序噪声资源应用。
    }; // 取得夹具源句柄与公共噪声。
    let (Some(trunk_mesh), Some(canopy_mesh)) = (
        meshes.get(&source.trunk_mesh), // 等待高档树干primitive完成CPU资产准备。
        meshes.get(&source.canopy_mesh), // 等待高档树冠primitive完成CPU资产准备。
    ) else {
        return; // 中性树形夹具GLB尚未完成时留给下一帧重试。
    }; // 取得两份夹具高档网格。
    let (Some(trunk_aabb), Some(canopy_aabb)) =
        (trunk_mesh.compute_aabb(), canopy_mesh.compute_aabb())
    else {
        return; // 缺位置属性的异常源不能伪造可见树高。
    }; // 从实际顶点计算两部件的局部边界。
    let (Some(trunk_base), Some(canopy_base)) = (
        standard_materials.get(&source.trunk_material), // 等待树皮材质及递归纹理完成。
        standard_materials.get(&source.canopy_material), // 等待叶簇材质及递归纹理完成。
    ) else {
        return; // 任一PBR源尚未登记时保持整棵树未生成。
    }; // 取得两份正式标准材质。
    let combined_aabb = combine_tree_part_aabbs(trunk_aabb, canopy_aabb); // 树干与树冠共享完整三十一米树高。
    let culling_aabb = expanded_tree_wind_culling_aabb(combined_aabb); // 手工边界覆盖正式最大GPU风位移。
    let mut trunk_base = trunk_base.clone(); // 克隆后只校准本次烟雾，不修改GLB源资产。
    trunk_base.double_sided = true; // 与GLB夹具树干校准保持一致。
    trunk_base.cull_mode = None; // 任意相机方向都保留树皮外侧与异常面朝向兼容。
    trunk_base.opaque_render_method = OpaqueRendererMethod::Deferred; // 复现Solari活动期兼容桥对正式TreeWind资产的渲染方式。
    let mut canopy_base = canopy_base.clone(); // 克隆后保留全部纹理与PBR参数。
    canopy_base.alpha_mode = AlphaMode::Mask(0.45); // 与GLB夹具树冠裁切阈值一致。
    canopy_base.double_sided = true; // 薄叶簇正反面都可见。
    canopy_base.cull_mode = None; // 禁用背面剔除以保留完整树冠轮廓。
    canopy_base.opaque_render_method = OpaqueRendererMethod::Deferred; // 覆盖活动期GBuffer兼容桥的同类渲染方式。
    let detailed = wind_materials.add(create_tree_wind_material(
        canopy_base.clone(),    // 夹具树冠PBR与AlphaMask输入。
        combined_aabb,          // 与树干共享完整树高。
        &noise,                 // 使用生产相同程序噪声绑定。
        MeshWindLodLevel::High, // 高档启用完整叶簇细摆。
        MeshWindPart::Foliage,  // 选择叶簇详细位移分支。
    )); // 完成夹具高档树冠扩展材质。
    let low = wind_materials.add(create_tree_wind_material(
        canopy_base,           // 未生成的低档副本只维持现有三句柄更新夹具。
        combined_aabb,         // 保持同一夹具树高参数。
        &noise,                // 保持同一风噪声绑定。
        MeshWindLodLevel::Low, // 不编译详细叶簇分支。
        MeshWindPart::Foliage, // 仍使用叶簇宏观参数。
    )); // 完成未渲染低档更新句柄。
    let stem = wind_materials.add(create_tree_wind_material(
        trunk_base,             // 夹具树皮PBR输入。
        combined_aabb,          // 与树冠共享完整树高。
        &noise,                 // 使用生产相同程序噪声绑定。
        MeshWindLodLevel::High, // 高档茎干仍只启用宏观弯曲。
        MeshWindPart::Stem,     // 选择树干根部固定语义。
    )); // 完成夹具高档树干扩展材质。
    let instance_count = instance_count.map_or(1, |count| count.0); // 缺资源的防御路径仍生成单树基线。
    for instance in 0..instance_count {
        let centered = instance as f32 - (instance_count.saturating_sub(1) as f32 * 0.5); // 围绕世界原点对称排布，固定相机无需随实例数改变。
        let transform = Transform::from_xyz(centered * 7.0, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_y(centered * 0.17))
            .with_scale(Vec3::splat(0.42)); // 局部31米乘0.42，得到固定13.02米高的测试样本。
        commands.spawn((
            Name::new(format!("MeshWindRealTreeTrunk_{instance}")), // 图形捕获中的夹具树干稳定实例身份。
            Mesh3d(source.trunk_mesh.clone()), // 全部实例复用夹具Mesh0强句柄以形成同材质批次。
            MeshMaterial3d(stem.clone()),      // 全部实例绑定同一夹具树干TreeWind材质。
            transform,                         // 树干与同编号树冠共享完全相同世界姿态。
            VisibilityRange {
                start_margin: 0.0..0.0, // 与生产高档相同的近端范围，同时显式启用VISIBILITY_RANGE_DITHER变体。
                end_margin: 1_000.0..1_100.0, // 隔离测试相机始终位于完整可见区，禁止正常远端淡出污染结论。
                use_aabb: false,              // 与双部件夹具相同地按实例根变换测距。
            }, // 完成夹具树干距离LOD输入。
            culling_aabb,                      // 覆盖全部风姿态的联合边界。
        )); // 完成当前夹具树干实例。
        commands.spawn((
            Name::new(format!("MeshWindRealTreeCanopy_{instance}")), // 图形捕获中的夹具树冠稳定实例身份。
            Mesh3d(source.canopy_mesh.clone()), // 全部实例复用夹具Mesh1强句柄以形成同材质批次。
            MeshMaterial3d(detailed.clone()),   // 全部实例绑定同一夹具高档详细叶簇材质。
            transform,                          // 树冠与同编号树干严格同位。
            VisibilityRange {
                start_margin: 0.0..0.0,       // 树冠与树干共享完全相同近端宽域。
                end_margin: 1_000.0..1_100.0, // 当前相机距离内严格保持完整可见。
                use_aabb: false,              // 与夹具树形统一使用根位置而非各部件AABB中心。
            }, // 完成夹具树冠距离LOD输入。
            culling_aabb,                       // 与树干共享联合风动边界。
        )); // 完成当前夹具树冠实例。
    } // 完成有限多实例PBR合批烟雾场景。
    commands.insert_resource(SmokeWindMaterials {
        detailed, // 每帧推进夹具高档树冠历史。
        low,      // 保持低质量Shader变体仍参与烟雾编译检查。
        stem,     // 每帧推进夹具高档树干历史。
    }); // 两个可见部件与三份更新材质完整后原子发布截图就绪条件。
} // 完成夹具高档树烟雾创建。

/// 每帧只推进三份共享材质，不查询或遍历固定程序实体。
fn animate_wind_specimens(
    handles: Option<Res<SmokeWindMaterials>>, // 创建完成前无事可做。
    mut materials: ResMut<Assets<TreeWindMaterial>>, // 原地推进当前和上一帧Uniform。
    time: Res<Time>,                          // 世界风向缓慢旋转并由Shader时间驱动噪声滚动。
) {
    let Some(handles) = handles else {
        return; // 非阻塞等待一次性程序材质发布。
    }; // 取得三份共享材质句柄。
    let angle = time.elapsed_secs() * 0.18; // 缓慢转向同时验证实例局部风向转换。
    let direction = Vec2::new(angle.cos(), angle.sin()); // 构造始终有限的单位世界X/Z风向。
    if let Some(mut material) = materials.get_mut(&handles.detailed) {
        update_tree_wind_material(
            &mut material,          // 更新一份高档共享材质。
            MeshWindLodLevel::High, // 保持创建时详细质量档。
            MeshWindPart::Foliage,  // 保持叶簇局部细摆语义。
            direction,              // 使用当前统一世界风向。
            0.78,                   // 非端点强风便于观察全部位移项。
            1.1,                    // 使用与正式默认接近的动画速度。
        ); // 完成高档当前/历史推进。
    } // 句柄暂未准备时留给下一帧重试。
    if let Some(mut material) = materials.get_mut(&handles.low) {
        update_tree_wind_material(
            &mut material,         // 更新一份低档共享材质。
            MeshWindLodLevel::Low, // 保持创建时宏观质量档。
            MeshWindPart::Foliage, // 保持叶簇身份但不启用详细分支。
            direction,             // 与高档共享完全相同的世界风向。
            0.78,                  // 宏观强度与高档严格一致。
            1.1,                   // 噪声滚动速度与高档严格一致。
        ); // 完成低档当前/历史推进。
    } // 句柄暂未准备时留给下一帧重试。
    if let Some(mut material) = materials.get_mut(&handles.stem) {
        update_tree_wind_material(
            &mut material,          // 更新不透明输入茎干共享材质。
            MeshWindLodLevel::High, // 与创建时质量档保持一致。
            MeshWindPart::Stem,     // 只执行根部固定的宏观弯曲。
            direction,              // 与两份叶簇共享世界风向。
            0.78,                   // 使用相同非端点强风。
            1.1,                    // 使用相同噪声滚动速度。
        ); // 完成茎干当前/历史推进。
    } // Opaque输入兼容材质未准备时留给下一帧重试。
} // 完成三份共享Mesh风材质逐帧推进。

/// 预热后只提交一次离屏颜色附件截图，并在GPU回读完成时成功退出。
fn capture_automatic_smoke(
    mut commands: Commands, // 创建Bevy标准Screenshot回读实体并附加保存/退出观察器。
    mut request: Option<ResMut<SmokeCaptureRequest>>, // 未设置输出路径时稳定帧零成本返回。
    target: Option<Res<SmokeRenderTarget>>, // Startup已发布的唯一离屏Image强句柄。
    ready: Option<Res<SmokeWindMaterials>>, // 真实或程序样本完整生成后才允许截取颜色附件。
    time: Res<Time<Real>>,  // 使用真实时间确保异步资产与RenderApp获得足够预热帧。
    mut ready_frames: Local<u8>, // 样本生成后额外等待固定八帧完成提取、准备与绘制。
) {
    let (Some(request), Some(target), Some(_ready)) = (request.as_mut(), target, ready) else {
        return; // 普通手工或仅计时烟雾不产生GPU回读与磁盘文件。
    }; // 同时取得一次性请求与离屏目标。
    *ready_frames = ready_frames.saturating_add(1); // 就绪后逐帧累计且在八帧处自然饱和。
    if request.requested || time.elapsed_secs() < 2.0 || *ready_frames < 8 {
        return; // 前两秒只允许材质、网格和全部同步专用管线稳定进入渲染阶段。
    } // 完成一次性与预热门。
    request.requested = true; // 先关闭重复入口，再把截图实体交给异步GPU回读。
    commands
        .spawn(Screenshot::image(target.0.clone())) // 精确回读相机正在写入的离屏颜色Image。
        .observe(save_to_disk(request.path.clone())) // 使用调用方路径同步保存RGB PNG。
        .observe(exit_after_smoke_capture); // 捕获事件完成后再结束无窗口应用。
} // 完成自动离屏截图提交。

/// 截图已经回到主世界后结束自动烟雾。
fn exit_after_smoke_capture(
    _capture: On<ScreenshotCaptured>, // 事件本身证明GPU回读完成，像素由相邻保存观察器消费。
    mut exits: MessageWriter<AppExit>, // 使用Bevy标准退出消息让RenderApp完整清理。
) {
    exits.write(AppExit::Success); // 截图完成代表本次像素烟雾正常结束。
} // 完成截图后退出观察器。

/// 到达自动烟雾时限后返回成功。
fn exit_automatic_smoke(
    timer: Option<ResMut<SmokeTimer>>, // 普通交互运行没有该资源。
    time: Res<Time<Real>>,             // 墙钟时间不受游戏虚拟时间缩放影响。
    mut exits: MessageWriter<AppExit>, // 通过Bevy标准退出消息结束窗口和RenderApp。
) {
    let Some(mut timer) = timer else {
        return; // 手工观察模式保持运行直到用户关闭窗口。
    }; // 取得自动烟雾计时器。
    if timer.0.tick(time.delta()).just_finished() {
        exits.write(AppExit::Success); // 运行足够时长后以零退出码结束后端烟雾。
    } // 未到时继续让RenderApp编译并绘制所有管线变体。
} // 完成自动烟雾退出控制。

/// 构造保留StandardMaterial PBR、双面与AlphaMask行为的程序叶簇基础材质。
fn foliage_base_material(
    texture: Handle<Image>,         // 共享程序综合色与透明度纹理。
    renderer: OpaqueRendererMethod, // 分别选择前向或延迟主路径。
) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,          // 不额外着色程序纹理的绿色通道。
        base_color_texture: Some(texture), // AlphaMask片元继续由StandardMaterial读取。
        perceptual_roughness: 0.82,        // 粗糙叶面保留可辨识方向光响应。
        alpha_mode: AlphaMode::Mask(0.5),  // 透明边缘参与主画面、预通道和动态阴影裁剪。
        double_sided: true,                // 交叉卡片从任意相机和阴影方向都可见。
        opaque_render_method: renderer,    // 烟雾明确覆盖前向与延迟两条主路径。
        ..default()                        // 金属度、反射率等沿用标准PBR默认值。
    } // 返回供自有扩展包装的完整基础材质。
} // 完成程序叶簇基础PBR材质构造。

/// 生成两张互相垂直的四米叶簇卡片。
fn create_cross_foliage_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    ); // 主世界保留且上传GPU的三角网格。
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION, // 八个顶点组成X/Y和Z/Y两个竖直平面。
        vec![
            [-SPECIMEN_HALF_WIDTH, 0.0, 0.0],             // 第一平面左根。
            [SPECIMEN_HALF_WIDTH, 0.0, 0.0],              // 第一平面右根。
            [SPECIMEN_HALF_WIDTH, SPECIMEN_HEIGHT, 0.0],  // 第一平面右梢。
            [-SPECIMEN_HALF_WIDTH, SPECIMEN_HEIGHT, 0.0], // 第一平面左梢。
            [0.0, 0.0, -SPECIMEN_HALF_WIDTH],             // 第二平面后根。
            [0.0, 0.0, SPECIMEN_HALF_WIDTH],              // 第二平面前根。
            [0.0, SPECIMEN_HEIGHT, SPECIMEN_HALF_WIDTH],  // 第二平面前梢。
            [0.0, SPECIMEN_HEIGHT, -SPECIMEN_HALF_WIDTH], // 第二平面后梢。
        ], // 完成程序顶点位置。
    ); // 登记位置属性。
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL, // 快速法线路径直接使用两张卡片的导入式常量法线。
        vec![
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        ], // 完成两张卡片法线。
    ); // 登记法线属性。
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0, // 两张卡片共享完整零至一AlphaMask纹理区域。
        vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0],
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0],
        ], // 完成两张卡片UV。
    ); // 登记基础颜色UV属性。
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7])); // 每张卡片各使用两个三角形。
    mesh // 返回可直接登记并供三份材质共享的程序网格。
} // 完成交叉叶簇网格生成。

/// 生成带透明边缘和不规则孔洞的八乘八叶色纹理。
fn create_alpha_mask_texture() -> Image {
    let mut pixels = Vec::with_capacity(8 * 8 * 4); // RGBA8固定容量避免循环扩容。
    for y in 0_i32..8 {
        // 每行根据边缘和固定孔洞生成AlphaMask。
        for x in 0_i32..8 {
            // 中心绿色不透明，外圈和两处孔洞完全透明。
            let border = x == 0 || x == 7 || y == 0 || y == 7; // 外圈裁成叶簇轮廓。
            let hole = (x == 2 && y == 4) || (x == 5 && y == 2); // 固定孔洞验证阴影Alpha裁剪。
            let alpha = if border || hole { 0 } else { 255 }; // 阈值两侧使用精确端点避免歧义。
            let green = 105 + y as u8 * 10; // 从根部到梢部逐步提亮绿色。
            pixels.extend_from_slice(&[38, green, 28, alpha]); // 写入线性可视颜色和AlphaMask通道。
        } // 完成当前纹理行。
    } // 完成八乘八全部程序像素。
    Image::new(
        Extent3d {
            width: 8,                 // 固定八像素宽度足以展示裁剪轮廓。
            height: 8,                // 固定八像素高度保持烟雾轻量。
            depth_or_array_layers: 1, // 只使用一张二维纹理层。
        }, // 完成图片尺寸描述。
        TextureDimension::D2, // StandardMaterial按二维基础颜色纹理采样。
        pixels,               // 传入完整程序绿色和透明度字节。
        TextureFormat::Rgba8UnormSrgb, // 颜色按sRGB解释，Alpha仍保持线性阈值。
        RenderAssetUsages::default(), // 主世界保留并上传RenderApp。
    ) // 返回可直接登记到Assets<Image>的程序AlphaMask纹理。
} // 完成程序叶色纹理生成。
