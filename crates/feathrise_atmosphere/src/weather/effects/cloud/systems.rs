//! 动态云 ECS 生命周期：选择唯一视觉相机、跟随位置并原子更新共享材质参数。

use bevy::camera::visibility::RenderLayers; // 天气专属层隔离小场景、菜单和辅助相机。
use bevy::ecs::system::SystemParam; // 收拢天气上下文、查询与可选资产资源。
use bevy::light::{NotShadowCaster, NotShadowReceiver}; // 远景云不参与场景阴影图或接受地物阴影。
use bevy::prelude::*; // Commands、Query、Assets 与基础组件类型。
use feathrise_scene_core::view::PrimarySceneView; // 正式玩法主相机是默认视觉锚点。
use feathrise_scene_core::constants::render_layers::WEATHER_EFFECT_LAYER; // 复用统一天气渲染层。
use feathrise_scene_core::state::ScenePhase; // 菜单和加载阶段销毁玩法云层。

use super::super::{is_gameplay_scene, supports_camera_local_weather}; // 云雨雾共享场景和正交隔离边界。
use super::dome_mesh::build_cloud_dome_mesh; // 一次性无 UV 二十面球远景载体。
use super::material::DynamicCloudMaterial; // 世界视线投影的透明自定义材质。
use super::noise::build_cloud_noise_image; // 一次性四通道周期噪声。
use super::profile::{
    CloudCurtainProfile, CloudSunLight, active_virtual_delta_seconds, advance_cloud_scroll,
    cloud_curtain_profile,
}; // 纯参数解析与暂停感知动画。
use crate::{
    AuthoredSkyboxRequest, AuthoredSkyboxStatus, WeatherAuthoringPreview, WeatherOutput,
    WeatherPreviewCamera, WorldCelestialLight, WorldClock,
    time::constants::{SUNRISE_HOUR, SUNSET_HOUR},
}; // 连续天气、天空盒采用状态与 F12 相机共同决定动态云资格。

/// 单实例动态天气云标记。
#[derive(Component, Debug)]
pub(crate) struct WeatherCloudCurtain; // 只承担查询、去重、调试与测试职责。

/// 当前周期风偏移保存在实体上，不污染全局天气资源。
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct CloudScrollOffset(pub(crate) Vec2); // 零至一循环偏移可在最小 ECS 测试中读取。

/// 系统本地持有一次性程序资产强句柄。
#[derive(Default)]
pub(crate) struct CloudAssetCache {
    texture: Option<Handle<Image>>, // 晴天隐藏期间仍保留四通道程序噪声。
    mesh: Option<Handle<Mesh>>,     // 菜单销毁实体后仍复用同一远景球。
    material: Option<Handle<DynamicCloudMaterial>>, // 所有天气阶段共享一份小 Uniform 材质。
}

/// 可成为动态云视觉锚点的相机数据。
type CloudCameraQueryData = (
    &'static Transform,                     // 云球中心逐帧复制当前相机位置。
    Option<&'static Camera>,                // 最小 ECS 测试允许省略真实 Camera。
    Has<WeatherPreviewCamera>,              // 区分 F12 作者相机与玩法主相机。
    Option<&'static Projection>,            // F12 正交测绘视图不显示局部远景云。
    Option<&'static AuthoredSkyboxRequest>, // 场景模式声明随相机请求发布。
    Option<&'static AuthoredSkyboxStatus>,  // 主 Cube 真正采用后关闭动态云。
);
/// 相机和云实体显式互斥，满足 Transform 借用约束。
type CloudCameraQueryFilter = (
    Or<(With<PrimarySceneView>, With<WeatherPreviewCamera>)>, // 普通辅助相机无天气资格。
    Without<WeatherCloudCurtain>,                         // 云实体绝不被误当视觉锚点。
);
/// 现有动态云更新所需组件。
type CloudCurtainQueryData = (
    Entity,
    &'static mut Transform,
    &'static mut Visibility,
    &'static mut CloudScrollOffset,
    &'static MeshMaterial3d<DynamicCloudMaterial>,
);
/// 云实体和两类相机显式互斥。
type CloudCurtainQueryFilter = (
    With<WeatherCloudCurtain>,
    Without<PrimarySceneView>,
    Without<WeatherPreviewCamera>,
);

/// 动态云每帧只读天气、天空、场景和虚拟时间上下文。
#[derive(SystemParam)]
pub(crate) struct CloudCurtainInputs<'w> {
    output: Res<'w, WeatherOutput>, // 云量、色调与风场来自唯一连续输出。
    scene: Option<Res<'w, State<ScenePhase>>>, // 正式游戏隔离菜单，最小测试可缺失。
    clear_color: Option<Res<'w, ClearColor>>, // 动态大气不可用时的回退色也用于云层调色。
    clock: Option<Res<'w, WorldClock>>, // 固定兼容灯夜间转为月光，按权威世界时间禁止暖边。
    virtual_time: Option<Res<'w, Time<Virtual>>>, // F1/F12 暂停同步冻结滚动。
    preview: Option<Res<'w, WeatherAuthoringPreview>>, // F12 开启时切换唯一视觉锚点。
}

/// 动态云相机与实体查询集合。
#[derive(SystemParam)]
pub(crate) struct CloudCurtainQueries<'w, 's> {
    cameras: Query<'w, 's, CloudCameraQueryData, CloudCameraQueryFilter>,
    curtains: Query<'w, 's, CloudCurtainQueryData, CloudCurtainQueryFilter>,
    sun_lights: Query<
        'w,
        's,
        (
            &'static WorldCelestialLight,
            &'static DirectionalLight,
            &'static Transform,
        ),
        Without<WeatherCloudCurtain>,
    >, // 只读取已有的最终天体灯，不创建第二套太阳轨迹。
}

/// 延迟创建动态云所需的可选渲染资产容器。
#[derive(SystemParam)]
pub(crate) struct CloudCurtainAssets<'w> {
    images: Option<ResMut<'w, Assets<Image>>>, // 四通道程序纹理资产表。
    meshes: Option<ResMut<'w, Assets<Mesh>>>,  // 无 UV 远景球资产表。
    materials: Option<ResMut<'w, Assets<DynamicCloudMaterial>>>, // 自定义透明材质资产表。
}

/// 同步玩法或 F12 透视相机周围唯一的世界视线动态云层。
pub(crate) fn sync_weather_cloud_curtain(
    mut commands: Commands,            // 创建、去重并在失去资格时销毁云实体。
    inputs: CloudCurtainInputs,        // 读取当帧最终天气、天空与暂停状态。
    mut queries: CloudCurtainQueries,  // 选择视觉锚点并更新现有单实例。
    mut assets: CloudCurtainAssets,    // 渲染资源缺失时安全降级到动态天空或 ClearColor。
    mut cache: Local<CloudAssetCache>, // 资产只在首次需要时创建并跨场景复用。
) {
    let gameplay_scene = is_gameplay_scene(inputs.scene.as_deref()); // 所有效果使用相同玩法场景边界。
    let final_sky_color = inputs.clear_color.as_deref().map(|color| color.0); // 最小测试允许没有清屏资源。
    let daylight = inputs.clock.as_deref().is_some_and(|clock| {
        (SUNRISE_HOUR..=SUNSET_HOUR).contains(&clock.hour_wrapped()) // 日夜资格与正式世界时钟一致。
    });
    let sunlight = queries
        .sun_lights
        .iter()
        .find_map(|(authored, light, transform)| {
            CloudSunLight::from_final_light(authored, light, transform, daylight) // 不从任意辅助灯或月亮读取朝阳方向。
        });
    let profile = cloud_curtain_profile(&inputs.output, final_sky_color, gameplay_scene, sunlight); // 一次解析完整连续参数。
    let preview_active = inputs
        .preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 缺失作者资源时选择正式玩法相机。
    let camera_position = queries
        .cameras
        .iter()
        .find(|(_, camera, is_preview, projection, _, _)| {
            *is_preview == preview_active // F12 开关决定当前唯一云层锚点。
                && camera.is_none_or(|camera| camera.is_active) // 排除切换场景期间已停用旧相机。
                && supports_camera_local_weather(*is_preview, *projection) // 正交视图没有局部云穹。
        })
        .and_then(|(transform, _, _, _, request, status)| {
            let skybox_active = request
                .zip(status)
                .is_some_and(|(request, status)| status.uses_visible_skybox(request)); // 只有真实采用主 Cube 才关闭动态云，加载失败仍显示天气回退。
            (!skybox_active).then_some(transform.translation) // 天空盒模式不生成或保留独立云层，避免烘焙云重复。
        }); // 复制轻量定位值后释放查询借用。
    let active_position = gameplay_scene.then_some(camera_position).flatten(); // 非玩法场景无条件视为无相机。
    let delta_seconds = active_virtual_delta_seconds(inputs.virtual_time.as_deref()); // 暂停首帧也严格为零。
    let mut kept = false; // 意外重复时只保留首个合法实例。
    for (entity, mut transform, mut visibility, mut scroll, material_handle) in
        &mut queries.curtains
    {
        let Some(position) = active_position else {
            commands.entity(entity).despawn(); // 菜单、无相机或 F12 正交时彻底清理远景云。
            continue;
        };
        if kept {
            commands.entity(entity).despawn(); // 异常重复实例不得叠加透明度。
            continue;
        }
        kept = true; // 当前实体成为本帧唯一保留云层。
        if transform.translation != position {
            transform.translation = position; // 只在相机真实移动时改写载体位置。
        }
        if profile.visible {
            scroll.0 = advance_cloud_scroll(scroll.0, profile.scroll_velocity, delta_seconds); // 可见云按统一风滚动。
        }
        let material_valid = assets
            .materials
            .as_deref_mut()
            .and_then(|materials| materials.get_mut(&material_handle.0))
            .is_some_and(|mut material| {
                material.write_profile(profile, scroll.0); // 同帧原子更新云量、色彩和偏移。
                true
            });
        *visibility = if profile.visible && material_valid {
            Visibility::Visible // 厚云且材质存在时进入透明主通道。
        } else {
            Visibility::Hidden // 晴天或资源失效时没有固定 Alpha 灰膜。
        };
    }
    if kept || !profile.visible {
        return; // 已有单例或晴天均无需新建资产与实体。
    }
    let Some(position) = active_position else {
        return; // 没有有效透视天气相机时不创建远景载体。
    };
    spawn_cloud_curtain(&mut commands, position, profile, &mut assets, &mut cache); // 首帧直接使用当前天气外观。
}

/// 在三个渲染资产容器齐全时延迟创建唯一动态云实体。
fn spawn_cloud_curtain(
    commands: &mut Commands,         // 只在完整资产就绪后生成实体。
    position: Vec3,                  // 当前视觉相机世界位置。
    profile: CloudCurtainProfile,    // 当前可见天气完整参数。
    assets: &mut CloudCurtainAssets, // 三个可选资产表必须同时存在。
    cache: &mut CloudAssetCache,     // 跨菜单与晴雨切换复用强句柄。
) {
    let (Some(images), Some(meshes), Some(materials)) = (
        assets.images.as_deref_mut(),
        assets.meshes.as_deref_mut(),
        assets.materials.as_deref_mut(),
    ) else {
        return; // 纯 ECS 或渲染初始化窗口安全保持只有天空回退。
    };
    let texture = cache
        .texture
        .get_or_insert_with(|| images.add(build_cloud_noise_image()))
        .clone(); // 四通道 64 KiB 噪声只创建一次。
    let mesh = cache
        .mesh
        .get_or_insert_with(|| meshes.add(build_cloud_dome_mesh()))
        .clone(); // 无 UV 二十面球只创建一次。
    let material = cache
        .material
        .get_or_insert_with(|| {
            materials.add(DynamicCloudMaterial::new(
                texture.clone(), // 材质持有程序纹理强句柄。
                profile,         // 创建帧直接采用当前阴雨参数。
                Vec2::ZERO,      // 新实例从稳定周期原点开始。
            ))
        })
        .clone(); // 缓存和实体共同保持材质存活。
    let Some(mut material_asset) = materials.get_mut(&material) else {
        cache.material = None; // 外部移除缓存材质时下一帧重新创建。
        return;
    };
    material_asset.write_profile(profile, Vec2::ZERO); // 重进场景时覆盖离场前天气，避免闪一帧旧状态。
    commands.spawn((
        Name::new("WeatherDynamicClouds"), // 层级诊断使用稳定 ASCII 名称。
        WeatherCloudCurtain,               // 单实例生命周期标记。
        CloudScrollOffset::default(),      // 独立保存周期滚动状态。
        Mesh3d(mesh),                      // 只向片元阶段提供平滑世界位置。
        MeshMaterial3d(material),          // 绑定世界视线动态云材质。
        Transform::from_translation(position), // 首帧即以相机为中心，不产生平移视差。
        Visibility::Visible,               // 仅从 profile.visible 分支进入创建。
        RenderLayers::layer(WEATHER_EFFECT_LAYER), // 菜单、小场景与角色预览不绘制。
        NotShadowCaster,                   // 不进入方向光阴影图。
        NotShadowReceiver,                 // 远景云不接受场景几何阴影。
    ));
}
