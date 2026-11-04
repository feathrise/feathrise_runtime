//! 可见天体 ECS 系统：为动态地图同步相机远景太阳与月亮圆盘。

use bevy::{
    camera::{Projection, visibility::RenderLayers}, // 透视裁剪距离和专属环境层隔离小地图与菜单。
    ecs::system::SystemParam,                       // 收拢相机、光源、天体实体与可选渲染资产查询。
    light::{NotShadowCaster, NotShadowReceiver},    // 画面圆盘不进入方向光阴影图。
    prelude::*, // Commands、Assets、Mesh3d、StandardMaterial与ECS基础类型。
};
use feathrise_scene_core::view::PrimarySceneView; // 正式玩法主相机是F12关闭时唯一视觉锚点。
use feathrise_scene_core::constants::render_layers::{CELESTIAL_AUTHORING_LAYER, CELESTIAL_GAMEPLAY_LAYER}; // 玩法和F12作者视图各有独立天体层，避免同时活动时互相串画面。
use feathrise_scene_core::state::ScenePhase; // 菜单、加载和固定场景必须清理动态地图圆盘。

use super::{
    assets::{
        CelestialVisualAssetCache, SUN_HALO_DISTANCE_MULTIPLIER, SUN_HALO_RADIUS_MULTIPLIER,
        SUN_VISUAL_RADIUS_MULTIPLIER, build_sun_halo_image, celestial_core_material,
        moon_core_hdr_color, sun_core_hdr_color, sun_halo_material, sun_visual_tint,
    }, // 导入分离的程序纹理、材质、尺寸倍率与强句柄缓存。
    helpers::{
        celestial_distance, celestial_radius, moon_is_visible, sun_is_visible,
        visible_moon_direction, visible_sun_direction,
    }, // 导入可见天体的纯轨迹、显隐和角尺寸计算。
}; // 完成天体视觉资产与纯逻辑导入。
use crate::{
    WeatherAuthoringPreview, WeatherPreviewCamera, WorldCelestialLight, WorldCelestialLightKind,
    WorldClock,
    time::{
        components::{CelestialVisual, CelestialVisualKind}, // 可见核心、柔光和月亮实体身份。
        constants::MOON_ANGULAR_DIAMETER_RADIANS,           // 月亮视觉与月亮方向光共享固定角直径。
    },
}; // 动态作者光、世界时钟和F12相机选择共同决定当前两个圆盘。

/// 当前被选中的视觉相机轻量快照。
#[derive(Clone, Copy, Debug)] // 复制后释放相机查询借用，允许同一系统写天体Transform。
struct CelestialCameraSnapshot {
    position: Vec3,      // 世界位置用于让远景圆盘始终围绕当前观察者。
    distance: f32,       // 由透视远裁面推导，保证地图地形优先遮挡天体。
    render_layer: usize, // 当前目标相机唯一包含的天体层，玩法与F12之间不会串视图。
}

/// 天体系统读取的场景、时间与F12锚点资源。
#[derive(SystemParam)]
pub(crate) struct CelestialVisualInputs<'w> {
    clock: Res<'w, WorldClock>, // 当前日期和时刻决定两个圆盘方向与显隐。
    scene: Option<Res<'w, State<ScenePhase>>>, // 无状态最小测试按玩法处理，正式菜单和加载必须清理。
    preview: Option<Res<'w, WeatherAuthoringPreview>>, // F12打开时选择作者透视相机而不是玩法主相机。
}

/// 天体同步使用的渲染资产容器；最小ECS测试可全部缺失并安全降级。
#[derive(SystemParam)]
pub(crate) struct CelestialVisualAssets<'w> {
    meshes: Option<ResMut<'w, Assets<Mesh>>>, // 首次创建共享Circle网格。
    materials: Option<ResMut<'w, Assets<StandardMaterial>>>, // 首次创建太阳核心、柔光和月亮材质。
    images: Option<ResMut<'w, Assets<Image>>>, // 首次创建太阳径向柔光程序纹理。
}

/// 相机、动态地图天体作者值与三个运行时视觉图层查询。
#[derive(SystemParam)]
pub(crate) struct CelestialVisualQueries<'w, 's> {
    cameras: Query<
        'w,
        's,
        (
            &'static Transform,      // 两台目标相机都是世界根，可直接读取同帧本地世界姿态。
            Option<&'static Camera>, // 最小测试可省略真实Camera组件。
            Option<&'static Projection>, // 透视远裁面决定天体距离；F12正交视图不显示天体。
            Has<WeatherPreviewCamera>, // true表示F12作者相机。
        ),
        (
            Or<(With<PrimarySceneView>, With<WeatherPreviewCamera>)>, // 只接受玩法主相机或显式作者相机。
            Without<CelestialVisual>, // 与可写天体Transform查询形成静态互斥，避免Bevy B0001。
        ),
    >,
    profiles: Query<'w, 's, (&'static WorldCelestialLight, &'static DirectionalLight)>, // 动态太阳作者值与同帧最终方向光颜色共同驱动可见圆盘。
    visuals: Query<
        'w,
        's,
        (
            Entity,
            &'static CelestialVisual,
            &'static mut Transform,
            &'static mut Visibility,
            &'static mut RenderLayers,
        ),
        (
            Without<PrimarySceneView>,         // 与玩法相机Transform只读查询显式互斥。
            Without<WeatherPreviewCamera>, // 与F12相机Transform只读查询显式互斥。
        ),
    >,
}

/// 同步动态地图可见太阳核心、太阳柔光和月亮；固定场景、菜单及无透视相机时不保留图层。
pub(in crate::time) fn sync_celestial_visuals(
    mut commands: Commands,        // 创建、去重或清理最多三个运行时天体视觉实体。
    inputs: CelestialVisualInputs, // 读取世界时间、场景边界和F12视觉锚点。
    mut queries: CelestialVisualQueries, // 读取相机/作者配置并写太阳核心、柔光与月亮。
    mut assets: CelestialVisualAssets, // 缺少渲染资产容器时安全降级为只有现有光照。
    mut cache: Local<CelestialVisualAssetCache>, // 小网格、程序纹理和三份材质只创建一次并跨场景复用。
) {
    let gameplay_scene = inputs.scene.as_deref().is_none_or(|scene| {
        matches!(scene.get(), ScenePhase::Active) // 可见圆盘首版只迁移数据驱动动态地图。
    });
    let sun_snapshot = gameplay_scene
        .then(|| {
            queries
                .profiles
                .iter()
                .find(|(profile, _)| {
                    profile.uses_authored_day_night_curve
                        && profile.kind == WorldCelestialLightKind::Sun // 双方向光场景必须显式选择太阳，不能依赖实体遍历顺序误读月亮颜色。
                })
                .map(|(profile, light)| (profile.clone(), light.color)) // 光照系统已先写入低角度与天气合成后的最终太阳色。
        })
        .flatten();
    let preview_active = inputs
        .preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // F12关闭或资源缺失时选择玩法主相机。
    let camera = sun_snapshot.as_ref().and_then(|_| {
        queries
            .cameras
            .iter()
            .find(|(_, camera, projection, is_preview)| {
                *is_preview == preview_active // 作者视口与正式玩法只允许一个成为当前圆盘锚点。
                    && camera.is_none_or(|camera| camera.is_active) // 场景交接时排除已停用旧相机。
                    && celestial_distance(*projection).is_some() // F12正交测绘不显示球面天空天体。
            })
            .and_then(|(transform, _, projection, is_preview)| {
                // 天空模式只决定背景来源；可见日月始终由世界时钟模拟，切换天空盒不得撤销或重建。
                celestial_distance(projection).map(|distance| CelestialCameraSnapshot {
                    position: transform.translation, // 两类正式目标相机都是无父级世界根。
                    distance,                        // 保持在该相机远裁面内的稳定距离。
                    render_layer: if is_preview {
                        CELESTIAL_AUTHORING_LAYER // F12打开时圆盘只进入离屏作者相机。
                    } else {
                        CELESTIAL_GAMEPLAY_LAYER // F12关闭时圆盘只进入正式玩法相机。
                    },
                })
            })
    });
    let Some(((profile, final_sun_light_color), camera)) = sun_snapshot.zip(camera) else {
        for (entity, _, _, _, _) in &mut queries.visuals {
            commands.entity(entity).despawn(); // 离开动态地图、进入菜单或切到F12正交视图时彻底清理。
        }
        return;
    };

    let hour = inputs.clock.hour_wrapped(); // 显隐和方向在同一个规范化时刻采样，避免边界错帧。
    let sun_direction = visible_sun_direction(&inputs.clock, profile.azimuth_offset_radians); // 可见太阳永远只走真实白天轨迹。
    let moon_direction = visible_moon_direction(&inputs.clock, profile.azimuth_offset_radians); // 可见月亮永远不读取隐藏主光代理。
    let physical_sun_radius = celestial_radius(camera.distance, profile.angular_diameter_radians); // 作者角直径继续作为SunDisk、Solari和软阴影的物理口径。
    let sun_radius = physical_sun_radius * SUN_VISUAL_RADIUS_MULTIPLIER; // 视觉核心独立放大两倍，不反向改变物理光源范围。
    let sun_halo_camera = CelestialCameraSnapshot {
        distance: camera.distance * SUN_HALO_DISTANCE_MULTIPLIER, // 透明柔光中心固定置于不透明核心后方百分之一，避免两个三角网格共面争夺深度。
        ..camera // 相机位置和专属渲染层与核心保持完全一致。
    }; // 使用独立轻量快照让核心与柔光的深度分层在生成和后续更新中采用同一规则。
    let sun_halo_radius = celestial_radius(
        sun_halo_camera.distance,         // 按柔光自己的稍远距离重新计算世界半径。
        profile.angular_diameter_radians, // 仍以作者太阳物理角直径作为画面尺寸基础。
    ) * SUN_VISUAL_RADIUS_MULTIPLIER
        * SUN_HALO_RADIUS_MULTIPLIER; // 距离与半径同比增加后屏幕角尺寸不变，只消除共面闪烁。
    let moon_radius = celestial_radius(camera.distance, MOON_ANGULAR_DIAMETER_RADIANS); // 首版月亮使用接近地球的固定角直径。
    let sun_core_color = sun_core_hdr_color(final_sun_light_color); // 核心跟随低角度和天气后的最终方向光色相，只保留固定HDR峰值。
    let sun_halo_color = sun_visual_tint(final_sun_light_color); // 柔光使用同一线性色相，使太阳周围散射与水面直射高光一致。
    let moon_core_color = moon_core_hdr_color(); // 固定冷白月盘使用独立较低HDR峰值抵消默认物理曝光，避免夜间黑盘。
    sync_cached_sun_material_colors(
        assets.materials.as_deref_mut(), // 已有视觉实体的材质在时刻或天气改变时原位更新。
        &cache,                          // 缓存句柄定位核心与柔光两份共享材质。
        sun_core_color,                  // 传入本帧最终HDR核心颜色。
        sun_halo_color,                  // 传入本帧最终柔光乘色。
    ); // 在“三层均已存在”的快速返回前同步颜色，避免太阳生成后永远停留在首帧白色。
    let mut kept_sun = false; // 去重时只保留第一颗太阳。
    let mut kept_sun_halo = false; // 去重时只保留第一层太阳柔光。
    let mut kept_moon = false; // 去重时只保留第一颗月亮。
    for (entity, visual, mut transform, mut visibility, mut layers) in &mut queries.visuals {
        let kept = match visual.kind {
            CelestialVisualKind::Sun if !kept_sun => {
                kept_sun = true; // 当前实体成为唯一太阳。
                write_visual_transform(&mut transform, camera, sun_direction, sun_radius); // 同帧跟随当前相机与太阳方向。
                *visibility = if sun_is_visible(hour) {
                    Visibility::Visible // 六至十八点显示真实太阳圆盘。
                } else {
                    Visibility::Hidden // 夜间隐藏，不跟随主光代理迁移。
                };
                *layers = RenderLayers::layer(camera.render_layer); // F12开关同帧把太阳切到唯一目标视图。
                true
            }
            CelestialVisualKind::SunHalo if !kept_sun_halo => {
                kept_sun_halo = true; // 当前实体成为唯一太阳柔光层。
                write_visual_transform(
                    &mut transform,  // 原位更新已有柔光实体。
                    sun_halo_camera, // 柔光在核心后方保持稳定世界空间分层。
                    sun_direction,   // 柔光与核心仍使用完全相同的太阳方向。
                    sun_halo_radius, // 同比放大后的世界半径保持原有屏幕尺寸。
                ); // 取消共面后相机时域抖动不再显露圆盘三角划分。
                *visibility = if sun_is_visible(hour) {
                    Visibility::Visible // 六至十八点与物理太阳核心共同显示。
                } else {
                    Visibility::Hidden // 夜间同时隐藏，禁止残留固定亮斑。
                };
                *layers = RenderLayers::layer(camera.render_layer); // 柔光只进入与核心相同的当前目标视图。
                true
            }
            CelestialVisualKind::Moon if !kept_moon => {
                kept_moon = true; // 当前实体成为唯一月亮。
                write_visual_transform(&mut transform, camera, moon_direction, moon_radius); // 按独立夜间轨迹同步。
                *visibility = if moon_is_visible(hour) {
                    Visibility::Visible // 十八至次日六点显示首版固定圆月。
                } else {
                    Visibility::Hidden // 白天隐藏，暂不实现月相与日间淡月。
                };
                *layers = RenderLayers::layer(camera.render_layer); // 月亮与太阳共用当前相机隔离口径。
                true
            }
            _ => false, // 同类重复实例在本帧排队销毁。
        };
        if !kept {
            commands.entity(entity).despawn(); // 防御热重载或异常重复生成。
        }
    }
    if kept_sun && kept_sun_halo && kept_moon {
        return; // 三个实例都已存在，只需完成本帧Transform与显隐更新。
    }

    let (Some(meshes), Some(materials), Some(images)) = (
        assets.meshes.as_deref_mut(),
        assets.materials.as_deref_mut(),
        assets.images.as_deref_mut(),
    ) else {
        return; // 无渲染资产环境不创建半成品实体，方向光和天空仍照常工作。
    };
    let mesh = cache
        .mesh
        .get_or_insert_with(|| meshes.add(Mesh::from(Circle::new(1.0)))) // 单位圆盘用Transform保持角尺寸。
        .clone();
    let sun_material = cache
        .sun_material
        .get_or_insert_with(|| materials.add(celestial_core_material(sun_core_color))) // 首次动态地图按当前最终太阳色创建曝光校准的前向HDR核心材质。
        .clone();
    let sun_halo_texture = cache
        .sun_halo_texture
        .get_or_insert_with(|| images.add(build_sun_halo_image())) // 固定一百二十八像素程序渐变只生成一次。
        .clone();
    let sun_halo_material = cache
        .sun_halo_material
        .get_or_insert_with(|| materials.add(sun_halo_material(sun_halo_texture, sun_halo_color))) // 柔光材质引用中性纹理并乘入当前太阳色相。
        .clone();
    let moon_material = cache
        .moon_material
        .get_or_insert_with(|| materials.add(celestial_core_material(moon_core_color))) // 首次动态地图创建曝光校准但弱于太阳的冷白前向月盘材质。
        .clone();
    if !kept_sun {
        spawn_visual(
            &mut commands,
            CelestialVisualKind::Sun,
            mesh.clone(),
            sun_material,
            camera,
            sun_direction,
            sun_radius,
            sun_is_visible(hour),
        ); // 创建唯一太阳，首帧即采用当前时刻和角直径。
    }
    if !kept_sun_halo {
        spawn_visual(
            &mut commands,                // 生成运行时唯一太阳柔光实体。
            CelestialVisualKind::SunHalo, // 标记该层只负责透明散射光晕。
            mesh.clone(),                 // 继续复用单位圆盘网格。
            sun_halo_material,            // 使用动态太阳色相与径向透明纹理。
            sun_halo_camera,              // 使用位于不透明核心后方的独立距离快照。
            sun_direction,                // 保持与太阳核心完全相同的天空方向。
            sun_halo_radius,              // 距离同比放大后仍保持原屏幕角尺寸。
            sun_is_visible(hour),         // 与太阳核心使用同一日间显隐窗口。
        ); // 创建唯一柔光层，首帧即位于核心后方并保持相同方向与独立宽半径。
    }
    if !kept_moon {
        spawn_visual(
            &mut commands,
            CelestialVisualKind::Moon,
            mesh,
            moon_material,
            camera,
            moon_direction,
            moon_radius,
            moon_is_visible(hour),
        ); // 创建唯一月亮，其方向与独立月亮方向光共享同一夜间轨迹。
    }
}

/// 只在最终太阳色实际变化时原位更新缓存材质，避免静止时刻无条件触发资产重提取。
fn sync_cached_sun_material_colors(
    materials: Option<&mut Assets<StandardMaterial>>, // 最小ECS测试可缺少材质资产容器并安全跳过。
    cache: &CelestialVisualAssetCache,                // 强句柄缓存定位当前太阳核心与柔光材质。
    core_color: Color,                                // 本帧最终HDR太阳核心颜色。
    halo_color: Color,                                // 本帧最终归一太阳柔光色相。
) {
    let Some(materials) = materials else {
        return; // 没有渲染资产容器时仍允许Transform、显隐和方向光逻辑继续运行。
    };
    if let Some(handle) = cache.sun_material.as_ref() {
        if let Some(mut material) = materials.get_mut(handle) {
            if material.base_color != core_color {
                material.base_color = core_color; // 时刻高度或天气色变更后让核心同帧采用最终方向光色相。
            }
        }
    }
    if let Some(handle) = cache.sun_halo_material.as_ref() {
        if let Some(mut material) = materials.get_mut(handle) {
            if material.base_color != halo_color {
                material.base_color = halo_color; // 柔光与核心同步更新，禁止白色光晕包围橙色水面倒影。
            }
        }
    }
}

/// 生成一个只进入环境远景层且不参与阴影的天体视觉实体。
#[allow(clippy::too_many_arguments)] // 每项参数对应明确的资产、身份、轨迹或首帧画面状态。
fn spawn_visual(
    commands: &mut Commands,            // 生成运行时天体实体。
    kind: CelestialVisualKind,          // 区分太阳核心、太阳柔光和月亮的后续轨迹与显隐。
    mesh: Handle<Mesh>,                 // 三个实体共享单位Circle网格。
    material: Handle<StandardMaterial>, // 每层天体视觉使用自己的无光照材质。
    camera: CelestialCameraSnapshot,    // 当前视觉相机位置和远景距离。
    direction: Vec3,                    // 从相机指向天体的世界单位向量。
    radius: f32,                        // 与相机距离匹配的世界圆盘半径。
    visible: bool,                      // 当前时刻的首帧显隐状态。
) {
    let mut transform = Transform::default(); // 先构造身份Transform，再统一走与更新相同的写入函数。
    write_visual_transform(&mut transform, camera, direction, radius); // 首帧不允许出现在世界原点闪烁。
    let name = match kind {
        CelestialVisualKind::Sun => "DynamicCelestialSun", // ASCII稳定名用于层级诊断。
        CelestialVisualKind::SunHalo => "DynamicCelestialSunHalo", // 柔光独立命名便于检查透明层与核心尺寸。
        CelestialVisualKind::Moon => "DynamicCelestialMoon",       // 月亮与太阳视觉保持不同身份。
    };
    commands.spawn((
        Name::new(name),          // 调试层级可直接辨认两个运行时天体。
        CelestialVisual { kind }, // 后续同步按身份选择轨迹、角尺寸和显隐。
        Mesh3d(mesh),             // 普通轻量网格进入光栅主通道，不进入动态地图唯一地形Solari代理。
        MeshMaterial3d(material), // 无光照材质只提供核心、柔光或月亮外观。
        transform,                // 相机远景位置、朝向与角尺寸已经在生成前完成。
        if visible {
            Visibility::Visible // 当前处于对应天体可见时段。
        } else {
            Visibility::Hidden // 仍生成成对实体，跨日只切显隐不反复分配资产。
        },
        RenderLayers::layer(camera.render_layer), // 只进入当前玩法或F12目标视图，小地图、菜单和角色预览不可见。
        NotShadowCaster,                          // 天体圆盘不进入方向光阴影图。
        NotShadowReceiver,                        // 圆盘不接收地形或角色阴影。
    ));
}

/// 把天体放到当前相机远景并让单位Circle正面朝向相机。
fn write_visual_transform(
    transform: &mut Transform,       // 原位更新已有或新建圆盘Transform。
    camera: CelestialCameraSnapshot, // 当前相机位置与远景距离。
    direction: Vec3,                 // 从相机指向天体的世界方向。
    radius: f32,                     // 由角直径和距离计算的世界半径。
) {
    let direction = direction.normalize_or(Vec3::Y); // 损坏数学结果回退天顶，禁止写入NaN变换。
    transform.translation = camera.position + direction * camera.distance; // 相机平移不产生天体视差。
    transform.rotation = Quat::from_rotation_arc(Vec3::Z, -direction); // Circle法线+Z始终朝回相机。
    transform.scale = Vec3::splat(radius.max(0.001)); // 保持正尺寸并让角直径随远裁面同比缩放。
}

#[cfg(test)]
mod tests {
    use super::*; // 最小ECS测试验证动态地图限定、层隔离和昼夜显隐。
    use crate::{AuthoredSkyboxRequest, AuthoredSkyboxStatus, WorldTimeSnapshot}; // 构造天空模式切换、午夜和正午正式状态。

    fn test_app(scene: ScenePhase, hour: f32, authored: bool) -> App {
        let mut app = App::new(); // 不启动窗口或RenderApp，只验证主世界实体行为。
        app.insert_resource(Assets::<Mesh>::default()); // 提供Circle网格资产容器。
        app.insert_resource(Assets::<StandardMaterial>::default()); // 提供三份无光照材质容器。
        app.insert_resource(Assets::<Image>::default()); // 提供太阳柔光程序纹理资产容器。
        app.insert_resource(State::new(scene)); // 显式验证动态地图与固定场景边界。
        app.insert_resource(WorldClock::from_snapshot(WorldTimeSnapshot {
            month: 6,                              // 使用六月覆盖太阳与月亮各自的年度轨道倾斜。
            day: 15,                               // 月中日期与默认世界时钟一致。
            seconds_since_midnight: hour * 3600.0, // 指定被测时刻。
        }));
        app.add_systems(Update, sync_celestial_visuals); // 单独注册正式同步系统。
        app.world_mut().spawn((
            PrimarySceneView,                                              // F12关闭时选择唯一玩法相机。
            Camera::default(),                                         // 默认活动相机满足正式过滤。
            Projection::Perspective(PerspectiveProjection::default()), // 透视视图允许天空天体。
            Transform::default(),                                      // 世界根相机位于原点。
        ));
        let profile = if authored {
            WorldCelestialLight::new_authored(
                WorldCelestialLightKind::Sun, // 任一动态太阳或月亮配置都可提供可见天体作者快照。
                9_000.0,                      // 动态地图日间基准。
                Color::WHITE,                 // 测试不依赖光色。
                1_000.0,                      // 动态地图月亮午夜峰值。
                Color::WHITE,                 // 测试不依赖月光色。
                0.0,                          // 无方位偏移便于断言东西方向。
                0.533_f32.to_radians(),       // 使用默认太阳角直径。
                Color::BLACK,                 // 清屏色不参与可见圆盘同步。
            )
        } else {
            WorldCelestialLight::new(9_000.0, Color::WHITE, Color::BLACK) // HN固定场景旧构造。
        };
        app.world_mut().spawn((
            profile, // 场景中存在唯一玩法天体作者标记。
            DirectionalLight {
                color: Color::WHITE, // 最小测试不运行光照系统，显式提供可见太阳读取的同帧最终光色。
                ..default()          // 照度与阴影字段不参与本模块测试。
            },
        )); // 生产场景同样把作者标记与真实方向光绑定在同一实体。
        app
    }

    #[test]
    fn dynamic_map_spawns_three_layer_isolated_visuals_with_day_night_visibility() {
        let mut app = test_app(ScenePhase::Active, 12.0, true); // 正午应显示太阳、隐藏月亮。
        app.update(); // 首帧延迟创建太阳核心、柔光和月亮实体。
        let values = {
            let world = app.world_mut(); // 在同一世界借用中收集身份、显隐、渲染层、缩放和距离。
            let mut query = world
                .query_filtered::<
                    (&CelestialVisual, &Visibility, &RenderLayers, &Transform),
                    With<Mesh3d>,
                >(); // 三种视觉都必须携带普通网格和完整可见状态。
            query
                .iter(world)
                .map(|(visual, visibility, layers, transform)| {
                    (
                        visual.kind,                    // 保存太阳核心、柔光或月亮身份。
                        *visibility,                    // 保存当前正午显隐状态。
                        layers.clone(),                 // 保存玩法与作者隔离层。
                        transform.scale.x,              // 单位Circle的X缩放就是当前世界半径。
                        transform.translation.length(), // 测试相机位于原点，因此长度就是当前天体相机距离。
                    ) // 组合成离开World借用后仍可直接断言的轻量值。
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(values.len(), 3); // 动态地图只创建太阳核心、柔光和一颗月亮。
        let sun = values
            .iter()
            .find(|value| value.0 == CelestialVisualKind::Sun)
            .unwrap();
        let sun_halo = values
            .iter()
            .find(|value| value.0 == CelestialVisualKind::SunHalo)
            .unwrap(); // 柔光必须是可单独诊断和去重的独立视觉层。
        let moon = values
            .iter()
            .find(|value| value.0 == CelestialVisualKind::Moon)
            .unwrap();
        assert_eq!(sun.1, Visibility::Visible); // 正午显示太阳。
        assert_eq!(sun_halo.1, Visibility::Visible); // 正午柔光与白热核心同帧显示。
        assert_eq!(moon.1, Visibility::Hidden); // 正午隐藏首版圆月。
        let physical_sun_radius = celestial_radius(
            celestial_distance(Some(&Projection::Perspective(
                PerspectiveProjection::default(), // 测试相机使用与正式生成时完全相同的默认透视投影。
            )))
            .unwrap(), // 透视相机必须提供合法远景天体距离。
            0.533_f32.to_radians(), // 测试作者配置使用地球默认物理太阳角直径。
        ); // 重新计算未经过艺术放大的物理圆盘半径。
        assert!((sun.3 / physical_sun_radius - SUN_VISUAL_RADIUS_MULTIPLIER).abs() < 0.000_1); // 可见太阳固定放大两倍，但作者物理角直径保持原值。
        assert!(
            (sun_halo.3 / sun.3 - SUN_HALO_RADIUS_MULTIPLIER * SUN_HALO_DISTANCE_MULTIPLIER).abs()
                < 0.000_1
        ); // 世界半径同时包含画面放大和后移距离补偿。
        assert!((sun_halo.4 / sun.4 - SUN_HALO_DISTANCE_MULTIPLIER).abs() < 0.000_1); // 柔光必须明确位于核心后方而不再共享同一深度平面。
        assert!(
            (sun_halo.3 / sun_halo.4 - sun.3 / sun.4 * SUN_HALO_RADIUS_MULTIPLIER).abs() < 0.000_1
        ); // 距离与半径同比变化后柔光屏幕角尺寸保持原设计倍率。
        assert!(
            sun.2
                .intersects(&RenderLayers::layer(CELESTIAL_GAMEPLAY_LAYER))
        ); // F12关闭时太阳只进入玩法环境远景层。
        assert!(
            !sun.2
                .intersects(&RenderLayers::layer(CELESTIAL_AUTHORING_LAYER))
        ); // 玩法圆盘不得串入F12作者相机。
        assert!(!sun.2.intersects(&RenderLayers::layer(0))); // 小地图默认层看不到太阳。
        assert_eq!(sun_halo.2, sun.2); // 柔光与核心必须进入完全相同的隔离层。
        app.world_mut().resource_mut::<WorldClock>().set_hour(0.0); // 切到午夜但不重建任何视觉资产。
        app.update(); // 同步既有三个实体的方向和显隐。
        let visibility = {
            let world = app.world_mut();
            let mut query = world.query::<(&CelestialVisual, &Visibility)>();
            query
                .iter(world)
                .map(|(visual, visibility)| (visual.kind, *visibility))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            visibility
                .iter()
                .find(|value| value.0 == CelestialVisualKind::Sun)
                .unwrap()
                .1,
            Visibility::Hidden,
        ); // 午夜隐藏太阳，绝不随夜间主光代理出现。
        assert_eq!(
            visibility
                .iter()
                .find(|value| value.0 == CelestialVisualKind::SunHalo)
                .unwrap()
                .1,
            Visibility::Hidden,
        ); // 午夜柔光必须与太阳核心一起隐藏，不能留下一圈固定亮斑。
        assert_eq!(
            visibility
                .iter()
                .find(|value| value.0 == CelestialVisualKind::Moon)
                .unwrap()
                .1,
            Visibility::Visible,
        ); // 午夜显示独立月亮圆盘。
        app.insert_resource(WeatherAuthoringPreview::new(true)); // 模拟F12打开并接管唯一视觉锚点。
        app.world_mut().spawn((
            WeatherPreviewCamera,                                      // 标记作者离屏相机。
            Camera::default(),                                         // 活动相机可以成为当前目标。
            Projection::Perspective(PerspectiveProjection::default()), // 透视F12视图显示天体。
            Transform::from_xyz(100.0, 200.0, 300.0), // 非零位置用于覆盖玩法相机锚点。
        ));
        app.update(); // 同一组三层视觉应切层而不是复制出第二组。
        let switched_layers = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<&RenderLayers, With<CelestialVisual>>();
            query.iter(world).cloned().collect::<Vec<_>>()
        };
        assert_eq!(switched_layers.len(), 3); // F12接管不能重复生成任何天体视觉层。
        assert!(switched_layers.iter().all(|layers| {
            layers.intersects(&RenderLayers::layer(CELESTIAL_AUTHORING_LAYER))
                && !layers.intersects(&RenderLayers::layer(CELESTIAL_GAMEPLAY_LAYER))
        })); // F12打开后三层视觉都只进入作者层，背后玩法相机不再看见。
    }

    #[test]
    fn unauthored_light_never_spawns_dynamic_celestial_bodies() {
        let mut app = test_app(ScenePhase::Active, 0.0, false); // 未声明太阳身份的光不能驱动天体。
        app.update(); // 即使在玩法地图中也不能凭普通灯光猜测圆盘作者参数。
        let count = {
            let world = app.world_mut();
            let mut query = world.query::<&CelestialVisual>();
            query.iter(world).count()
        };
        assert_eq!(count, 0); // 固定场景不迁移到本次可见天体实现。
    }

    #[test]
    fn authored_skybox_keeps_dynamic_celestial_visuals_unchanged() {
        let mut app = test_app(ScenePhase::Active, 12.0, true); // 正午用于同时验证可见太阳与隐藏月亮状态。
        app.update(); // 先建立天空模式切换前的太阳核心、柔光和月亮实体。
        let before = {
            let world = app.world_mut(); // 在一个借用范围内记录三层原始身份和显隐。
            let mut query = world.query::<(Entity, &CelestialVisual, &Visibility)>(); // 身份稳定可证明切换没有销毁重建。
            query
                .iter(world) // 读取当前全部运行时天体视觉。
                .map(|(entity, visual, visibility)| (visual.kind, entity, *visibility)) // 保存类型、实体和昼夜显隐。
                .collect::<Vec<_>>() // 释放World借用后再修改相机天空状态。
        }; // 完成切换前快照。
        assert_eq!(before.len(), 3); // 正午同样常驻太阳核心、柔光和隐藏月亮三层。
        let camera = {
            let world = app.world_mut(); // 在单个世界借用中找到唯一玩法相机实体。
            let mut query = world.query_filtered::<Entity, With<PrimarySceneView>>(); // 只匹配正式玩法相机。
            query.single(world).unwrap() // 保存实体后释放查询借用。
        };
        app.world_mut().entity_mut(camera).insert((
            AuthoredSkyboxRequest::new_with_mode(
                crate::AuthoredSkyMode::Skybox, // 主 Cube 接管可见天空。
                Some("sky.ktx2".to_owned()),    // 三件套共同主身份。
                1_000.0,                        // 可见 Cube 作者亮度。
                300.0,                          // IBL 漫反射基准。
                1_000.0,                        // IBL 镜面基准。
                0.0,                            // 测试不依赖方向。
                9,                              // 当前作者修订。
            ),
            AuthoredSkyboxStatus {
                phase: crate::AuthoredSkyboxPhase::Ready, // 三件套已经完整采用。
                asset_path: Some("sky.ktx2".to_owned()),  // 状态保留主身份。
                revision: 9,                              // 与请求修订一致。
                detail: String::new(),                    // 测试不依赖状态文案。
                visible_skybox_active: true, // 主Cube已经真正采用，但不能改变模拟日月。
            },
        ));
        app.update(); // 天空盒只替换背景，三层天体继续由同一系统同步。
        let skybox = {
            let world = app.world_mut(); // 记录天空盒真正采用后的身份与显隐。
            let mut query = world.query::<(Entity, &CelestialVisual, &Visibility)>(); // 使用与切换前相同口径比较。
            query
                .iter(world) // 读取切换后的全部运行时天体视觉。
                .map(|(entity, visual, visibility)| (visual.kind, entity, *visibility)) // 保存类型、实体和昼夜显隐。
                .collect::<Vec<_>>() // 释放World借用后执行逐类型断言。
        }; // 完成天空盒状态快照。
        assert_eq!(skybox.len(), 3); // 天空盒模式仍常驻太阳核心、柔光和月亮三层。
        for (kind, entity, visibility) in &before {
            // 每种天体必须保留原实体和同一时刻显隐，避免切换瞬间消失或重建闪烁。
            assert!(skybox.contains(&(*kind, *entity, *visibility))); // 类型、实体身份和显隐必须逐项完全一致。
        } // 完成天空盒采用状态核对。
        app.world_mut()
            .entity_mut(camera)
            .get_mut::<AuthoredSkyboxStatus>()
            .unwrap()
            .visible_skybox_active = false; // 模拟天空盒失败后回退动态天空。
        app.update(); // 背景回退同样不得干扰持续存在的模拟日月。
        let fallback = {
            let world = app.world_mut(); // 记录回退动态天空后的身份与显隐。
            let mut query = world.query::<(Entity, &CelestialVisual, &Visibility)>(); // 沿用同一稳定性口径。
            query
                .iter(world) // 读取回退后的全部运行时天体视觉。
                .map(|(entity, visual, visibility)| (visual.kind, entity, *visibility)) // 保存类型、实体和昼夜显隐。
                .collect::<Vec<_>>() // 结束最终World借用。
        }; // 完成动态天空回退快照。
        assert_eq!(fallback.len(), 3); // 背景加载失败也不能增减天体视觉层。
        for (kind, entity, visibility) in &before {
            // 回退后仍必须是最初三实体，证明两种天空模式都不拥有日月生命周期。
            assert!(fallback.contains(&(*kind, *entity, *visibility))); // 类型、身份和显隐在完整往返后保持一致。
        } // 完成模式往返稳定性核对。
    }

    #[test]
    fn existing_sun_materials_follow_final_directional_light_color() {
        let mut app = test_app(ScenePhase::Active, 17.67, true); // 复现用户截图约十七点四十分的动态地图。
        app.update(); // 首帧先创建太阳核心、柔光以及共享材质资产。
        let sunset_color = Color::srgb(1.0, 0.36, 0.10); // 模拟光照系统合成后的低角度暖橙最终太阳色。
        {
            let world = app.world_mut(); // 单独借用世界并更新正式太阳方向光组件。
            let mut query = world.query::<(&WorldCelestialLight, &mut DirectionalLight)>(); // 定位双方向光中的太阳实体。
            let (_, mut light) = query
                .iter_mut(world)
                .find(|(profile, _)| profile.kind == WorldCelestialLightKind::Sun)
                .expect("动态地图测试必须存在太阳方向光"); // 测试场景缺失太阳时直接暴露构造回归。
            light.color = sunset_color; // 模拟同帧低角度与天气计算完成后的最终输出。
        }
        app.update(); // 可见天体系统必须在不重建实体的情况下原位更新两份材质。
        let (core_handle, halo_handle) = {
            let world = app.world_mut(); // 收集太阳核心与柔光使用的StandardMaterial句柄。
            let mut query = world.query::<(&CelestialVisual, &MeshMaterial3d<StandardMaterial>)>(); // 按视觉身份读取共享材质句柄。
            let mut core_handle = None; // 尚未找到太阳核心材质。
            let mut halo_handle = None; // 尚未找到太阳柔光材质。
            for (visual, material) in query.iter(world) {
                match visual.kind {
                    CelestialVisualKind::Sun => core_handle = Some(material.0.clone()), // 保存HDR核心材质句柄。
                    CelestialVisualKind::SunHalo => halo_handle = Some(material.0.clone()), // 保存透明柔光材质句柄。
                    CelestialVisualKind::Moon => {} // 月亮颜色不应跟随太阳方向光。
                }
            }
            (
                core_handle.expect("动态地图必须存在太阳核心材质"), // 缺失核心说明三层生成回归。
                halo_handle.expect("动态地图必须存在太阳柔光材质"), // 缺失柔光说明散射层生成回归。
            )
        };
        let materials = app.world().resource::<Assets<StandardMaterial>>(); // 读取更新后的正式材质资产。
        let core = materials
            .get(&core_handle)
            .expect("太阳核心材质必须仍在缓存"); // 强句柄缓存不得让资产被回收。
        let halo = materials
            .get(&halo_handle)
            .expect("太阳柔光材质必须仍在缓存"); // 柔光材质同样应保持有效。
        assert_eq!(core.base_color, sun_core_hdr_color(sunset_color)); // 核心应采用最终方向光色相与固定HDR峰值。
        assert_eq!(core.alpha_mode, AlphaMode::Blend); // 核心必须留在Solari延迟解析后的前向透明阶段且以Alpha一维持视觉不透明。
        assert_eq!(halo.base_color, sun_visual_tint(sunset_color)); // 柔光应采用和水面PBR相同的最终方向光色相。
    }
}
