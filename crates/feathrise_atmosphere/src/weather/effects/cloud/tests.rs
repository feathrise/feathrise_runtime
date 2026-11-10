use super::*; // 测试直接访问云幕私有逻辑、组件与同步系统。
use bevy::camera::visibility::RenderLayers; // 验证云层只进入统一天气专属渲染层。
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler}; // 验证程序噪声的重复线性采样器。
use bevy::prelude::*; // 最小App、时间、相机、投影、渲染资产与数学类型。
use bevy::render::render_resource::TextureFormat; // 噪声必须使用不经 sRGB 解码的线性 RGBA8。
use feathrise_scene_core::view::PrimarySceneView; // 构造正式玩法视觉相机。
use feathrise_scene_core::constants::render_layers::WEATHER_EFFECT_LAYER; // 断言云层不泄漏到默认相机层。
use feathrise_scene_core::state::ScenePhase; // 在玩法与菜单之间验证云层生命周期。
use std::time::Duration; // 构造带非零旧delta的暂停虚拟时钟。

use crate::{
    AuthoredSkyMode, AuthoredSkyboxPhase, AuthoredSkyboxRequest, AuthoredSkyboxStatus,
    WeatherAuthoringPreview, WeatherOutput, WeatherPreviewCamera, WorldCelestialLight,
    WorldCelestialLightKind, WorldClock,
}; // 构造连续天气、天空盒互斥状态和 F12 作者视口。

#[test] // 晴天隐藏云幕，云量增加时透明度和可见性必须连续增强。
fn cloud_cover_continuously_controls_opacity_and_visibility() {
    let clear = WeatherOutput::default(); // 核心默认晴天包含百分之五基础云量。
    let overcast = WeatherOutput {
        cloud_cover: 0.78,                            // 使用明显厚云覆盖率验证可见云幕。
        tint: LinearRgba::new(0.65, 0.70, 0.78, 1.0), // 使用阴天冷灰色调。
        ..clear                                       // 从同一输出只修改云量和色调形成阴天样本。
    };
    let clear_profile = cloud_curtain_profile(&clear, Some(Color::srgb(0.4, 0.7, 1.0)), true, None); // 无最终太阳灯时仍计算玩法日间晴天云幕参数。
    let overcast_sky = Color::linear_rgb(0.22, 0.30, 0.42); // 模拟世界时间完成阴天压暗后的最终天空色。
    let overcast_profile = cloud_curtain_profile(&overcast, Some(overcast_sky), true, None); // 计算玩法阴天云幕参数。
    assert!(!clear_profile.visible); // 晴天基础薄云不得生成可见几何云幕。
    assert_eq!(clear_profile.opacity, 0.0); // 晴天透明度必须准确落在零端点。
    assert!(overcast_profile.visible); // 阴天厚云必须显示云幕。
    assert!(overcast_profile.opacity > 0.5); // 明显厚云应形成高不透明云体，但云洞仍保持零 Alpha。
    assert!(overcast_profile.coverage > 0.7); // 阴天主要通过覆盖阈值形成连贯云层。
    assert!(overcast_profile.top_color.z > overcast_profile.top_color.x); // 最终阴天蓝灰色相应进入云顶。
    assert!(overcast_profile.top_color.max_element() > overcast_profile.bottom_color.max_element()); // 云顶与云底必须有明确明暗层次。
    assert!(!cloud_curtain_profile(&overcast, Some(overcast_sky), false, None).visible); // 非玩法场景必须无条件关闭云幕。
} // 结束云量与可见性映射测试。

#[test] // 风力必须连续提高滚动速度，暂停时保持偏移且长时间推进仍循环在零至一。
fn wind_scroll_is_continuous_paused_and_wrapped() {
    let calm_output = WeatherOutput {
        cloud_cover: 0.8, // 确保参数属于可见云幕状态。
        wind: 0.0,        // 使用最低风力获取基础缓慢漂移。
        ..default()       // 其余连续天气通道保持晴天默认值。
    }; // 构造具有可见云量的低风样本。
    let storm_output = WeatherOutput {
        wind: 1.0,     // 使用满风力验证更快滚动。
        ..calm_output  // 复用其余字段只提高风力。
    };
    let sky = Some(Color::srgb(0.3, 0.5, 0.8)); // 风力测试使用固定日景天空排除色调变量。
    let calm = cloud_curtain_profile(&calm_output, sky, true, None); // 计算低风滚动向量。
    let storm = cloud_curtain_profile(&storm_output, sky, true, None); // 计算满风滚动向量。
    assert!(storm.scroll_velocity.length() > calm.scroll_velocity.length()); // 大雨风力必须使云幕移动更快。
    assert!(storm.scroll_velocity.x > 0.0); // 默认风场沿+X推动云层。
    assert_eq!(storm.scroll_velocity.y, 0.0); // 不再叠加地点专属的第二方向偏移。
    let current = Vec2::new(0.95, 0.98); // 使用接近周期边界的起点验证回绕。
    assert_eq!(
        advance_cloud_scroll(current, storm.scroll_velocity, 0.0),
        current
    ); // 虚拟时间暂停不得移动云幕。
    let advanced = advance_cloud_scroll(current, storm.scroll_velocity, 10.0); // 推进足够时间跨越至少一个边界。
    assert!((0.0..1.0).contains(&advanced.x)); // 水平偏移必须循环在合法采样域。
    assert!((0.0..1.0).contains(&advanced.y)); // 垂直偏移必须循环在合法采样域。
} // 结束风力滚动与循环测试。

#[test] // 打开F1或菜单的当帧即使虚拟时间保留旧delta也不得再漂移一次云层。
fn paused_virtual_time_discards_existing_frame_delta() {
    let mut time = Time::<Virtual>::default(); // 构造可手动推进和暂停的虚拟时钟。
    time.advance_by(Duration::from_secs_f32(0.25)); // 模拟本帧开始时已经计算出的非零delta。
    assert!(active_virtual_delta_seconds(Some(&time)) > 0.0); // 正常运行时应读取该帧增量。
    time.pause(); // 模拟统一暂停系统在同一Update调度中打开F1、ESC或TAB。
    assert_eq!(active_virtual_delta_seconds(Some(&time)), 0.0); // 暂停状态必须覆盖仍保留的旧delta。
} // 结束云层暂停首帧回归测试。

#[test] // 动态云调色必须随最终天空从日间变暗到午夜，并保留夕阳暖色。
fn cloud_palette_tracks_day_night_and_twilight_sky() {
    let output = WeatherOutput {
        cloud_cover: 1.0,                             // 使用完整云量隔离时段色彩影响。
        tint: LinearRgba::new(0.48, 0.57, 0.70, 1.0), // 使用大雨冷灰天气色。
        ..default()
    };
    let day = cloud_curtain_profile(&output, Some(Color::srgb(0.32, 0.52, 0.82)), true, None); // 模拟白天阴雨天空。
    let dusk = cloud_curtain_profile(&output, Some(Color::srgb(0.42, 0.10, 0.03)), true, None); // 模拟夕阳暖色天空。
    let night = cloud_curtain_profile(&output, Some(Color::srgb(0.004, 0.008, 0.024)), true, None); // 模拟午夜深蓝黑天空。
    let day_peak = day.top_color.max_element(); // 读取日间云顶最亮通道。
    let night_peak = night.top_color.max_element(); // 读取午夜云顶最亮通道。
    assert!(night_peak < day_peak * 0.1); // 午夜云幕必须明显暗于日间，不能出现亮灰顶棚。
    assert!(night.top_color.z > night.top_color.x); // 午夜云幕保留冷蓝夜色。
    assert!(dusk.top_color.x > dusk.top_color.z); // 夕阳最终天空应把暖红色连续带入云层。
    assert!(night.top_color.max_element() > night.bottom_color.max_element()); // 夜间仍保留云顶与云底层次而非同色贴片。
} // 结束云幕昼夜与晨昏色调测试。

#[test] // 朝阳云边只读取正式太阳灯的最终方向、颜色与天气衰减后照度。
fn sun_rim_follows_final_daylight_and_never_uses_moonlight() {
    let mut authored = WorldCelestialLight::new(10_000.0, Color::WHITE, Color::WHITE); // 固定场景单灯可在日间贡献云边。
    let mut light = DirectionalLight {
        illuminance: 2_000.0,                // 模拟经过天气压暗的最终日间照度。
        color: Color::srgb(1.0, 0.55, 0.30), // 模拟夕阳最终暖光。
        ..default()                          // 其余光照字段不改变云边判定。
    };
    let low_sun = Transform::from_xyz(90.0, 20.0, 0.0); // 使用地平线附近的当帧最终光方向。
    let rim = CloudSunLight::from_final_light(&authored, &light, &low_sun, true).unwrap(); // 日间应生成真实朝向。
    assert!(rim.direction.x > 0.9 && rim.direction.y > 0.0); // 朝向太阳而非固定屏幕边。
    assert!(rim.color.x > rim.color.z && rim.rim > 0.0 && rim.rim < 1.0); // 暖色且强度克制。
    light.illuminance = 500.0; // 强降雨压暗最终方向光时云边同步变弱。
    let rainy = CloudSunLight::from_final_light(&authored, &light, &low_sun, true).unwrap(); // 只变化最终照度。
    assert!(rainy.rim < rim.rim); // 不使用未受天气影响的作者日光基准直接发光。
    assert!(CloudSunLight::from_final_light(&authored, &light, &low_sun, false).is_none()); // 夜间复用为月光的固定灯不得产生暖边。
    authored.kind = WorldCelestialLightKind::Moon; // 动态场景月亮灯也必须排除。
    assert!(CloudSunLight::from_final_light(&authored, &light, &low_sun, true).is_none()); // 即使照度大也不把月亮当太阳。
    authored.kind = WorldCelestialLightKind::Sun; // 恢复日间太阳身份用于正午测试。
    assert!(
        CloudSunLight::from_final_light(
            &authored,
            &light,
            &Transform::from_xyz(0.0, 100.0, 0.0),
            true
        )
        .is_none()
    ); // 高角度日光不叠加夕阳暖边。
    let night = cloud_curtain_profile(
        &WeatherOutput {
            cloud_cover: 0.8, // 午夜仍可存在天气云量。
            ..default()       // 其余天气字段沿用中性测试值。
        },
        Some(Color::linear_rgb(0.002, 0.004, 0.012)), // 最终深蓝夜色。
        true,                                         // 仍在玩法场景，不能因夜色隐藏实际厚云。
        None,                                         // 午夜实际没有太阳样本。
    ); // 午夜无光时云体Uniform必须归零。
    assert_eq!(night.sunlight, CloudSunLight::default()); // 午夜保持深蓝暗云，不会留下旧灯Uniform。
} // 完成同帧光与日夜隔离回归。

#[test] // 正式ECS同步必须同帧读取最终天体光，进入夜晚不能滞留旧暖边Uniform。
fn cloud_material_uses_current_sun_and_clears_rim_at_night() {
    let mut app = App::new(); // 最小世界只执行正式动态云同步系统。
    app.insert_resource(Assets::<Image>::default()); // 提供一次性程序噪声资产表。
    app.insert_resource(Assets::<Mesh>::default()); // 提供单个远景载体网格资产表。
    app.insert_resource(Assets::<DynamicCloudMaterial>::default()); // 提供云材质Uniform资产表。
    app.insert_resource(WeatherOutput {
        cloud_cover: 0.75, // 阴天确保云实体可见。
        ..default()        // 其余天气参数沿用中性值。
    });
    app.insert_resource(State::new(ScenePhase::Active)); // 玩法场景启用云幕。
    let mut clock = WorldClock::default(); // 世界时钟是固定场景日夜身份的权威来源。
    clock.set_hour(7.0); // 清晨低角度日光才允许有暖边。
    app.insert_resource(clock); // 系统读取与最终光照同源的时刻。
    app.world_mut().spawn((PrimarySceneView, Transform::default())); // 提供玩法主视图锚点。
    let sun = app
        .world_mut()
        .spawn((
            WorldCelestialLight::new(10_000.0, Color::WHITE, Color::WHITE), // 固定场景兼容太阳灯。
            DirectionalLight {
                illuminance: 2_000.0,               // 模拟经过天气修正的最终照度。
                color: Color::srgb(1.0, 0.55, 0.3), // 模拟最终晨光的暖色。
                ..default()                         // 阴影等字段使用Bevy安全默认值。
            }, // 完整方向光绑定本次作者身份。
            Transform::from_xyz(90.0, 20.0, 0.0), // 从世界指向太阳的实际灯位置。
        ))
        .id(); // 保存灯实体供天气强度变化测试。
    app.add_systems(Update, sync_weather_cloud_curtain); // 注册实际生产系统而非纯参数替身。
    app.update(); // 第一帧以最终太阳灯直接建立正确材质。
    let handle = {
        let world = app.world_mut(); // 先从云实体读取唯一材质句柄。
        let mut query = world
            .query_filtered::<&MeshMaterial3d<DynamicCloudMaterial>, With<WeatherCloudCurtain>>(); // 排除其他材质。
        query.single(world).unwrap().0.clone() // 克隆句柄后结束ECS世界借用。
    };
    let morning = app
        .world()
        .resource::<Assets<DynamicCloudMaterial>>()
        .get(&handle)
        .unwrap()
        .params; // 首帧实际Uniform。
    assert!(morning.sun_color_rim.w > 0.0 && morning.sun_direction.x > 0.9); // 朝东暖边已同帧安装。
    app.world_mut()
        .entity_mut(sun)
        .get_mut::<DirectionalLight>()
        .unwrap()
        .illuminance = 500.0; // 模拟暴雨时最终光照降低。
    app.update(); // 新照度必须当帧降低暖边而不重建云实体。
    let rainy = app
        .world()
        .resource::<Assets<DynamicCloudMaterial>>()
        .get(&handle)
        .unwrap()
        .params; // 读取复用的材质。
    assert!(rainy.sun_color_rim.w < morning.sun_color_rim.w); // 暴雨不保留晴天轮廓。
    app.world_mut().resource_mut::<WorldClock>().set_hour(22.0); // 月光时段的兼容灯可能仍保留非零照度。
    app.update(); // 同一云实体清除旧太阳方向与暖边。
    let night = app
        .world()
        .resource::<Assets<DynamicCloudMaterial>>()
        .get(&handle)
        .unwrap()
        .params; // 读取夜间Uniform。
    assert_eq!(night.sun_color_rim, Vec4::ZERO); // 午夜不再出现暖色云边。
    assert_eq!(night.sun_direction, Vec4::ZERO); // 旧太阳方向同帧失效。
} // 完成云材质同帧太阳采样及日夜转换测试。

#[test] // 周期值噪声在U、V边界必须严格接续，程序图像必须使用Repeat采样。
fn procedural_cloud_noise_is_tileable_and_repeating() {
    for sample in [0.0, 0.17, 0.53, 0.99] {
        assert_eq!(
            periodic_value_noise(0.0, sample, 7, CLOUD_NOISE_SEED), // 读取水平周期起点。
            periodic_value_noise(1.0, sample, 7, CLOUD_NOISE_SEED), // 读取水平周期终点的回绕值。
        ); // 水平边界必须无缝连续。
        assert_eq!(
            periodic_value_noise(sample, 0.0, 7, CLOUD_NOISE_SEED), // 读取垂直周期起点。
            periodic_value_noise(sample, 1.0, 7, CLOUD_NOISE_SEED), // 读取垂直周期终点的回绕值。
        ); // 垂直边界必须无缝连续。
    } // 结束多个边界采样点验证。
    let image = build_cloud_noise_image(); // 生成正式运行使用的程序云纹理。
    assert_eq!(
        image.data.as_ref().map(Vec::len), // 读取RGBA像素缓冲实际字节数。
        Some((CLOUD_NOISE_SIZE * CLOUD_NOISE_SIZE * 4) as usize), // 一百二十八方形RGBA8应有固定字节数。
    ); // 程序纹理数据必须完整。
    assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba8Unorm); // 噪声通道不得被 sRGB 曲线改变阈值。
    let data = image.data.as_ref().unwrap(); // 正式 MAIN_WORLD 程序图必须保留 CPU 像素。
    assert!(data.chunks_exact(4).any(|pixel| pixel[3] < 96)); // A 通道也是原始噪声，不得恢复固定 0.18 最低 Alpha 灰膜。
    assert!(data.chunks_exact(4).any(|pixel| pixel[3] > 160)); // 同一通道保留足够动态范围供第二层侵蚀。
    let ImageSampler::Descriptor(sampler) = image.sampler else {
        panic!("程序云纹理必须使用自定义重复采样器"); // 默认Clamp采样会在滚动边界形成明显接缝。
    }; // 结束自定义采样器存在性检查。
    assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat); // 水平方向必须重复。
    assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat); // 垂直方向必须重复。
    assert_eq!(sampler.mag_filter, ImageFilterMode::Linear); // 放大时必须平滑低分辨率噪声。
} // 结束程序纹理无缝与采样器测试。

#[test] // 缺少渲染资产容器时厚云必须安全降级，不创建半成品实体。
fn cloud_system_is_safe_without_render_assets() {
    let mut app = App::new(); // 不安装DefaultPlugins也不插入任何渲染资产资源。
    let output = WeatherOutput {
        cloud_cover: 1.0, // 请求满云量以进入云幕创建路径。
        ..default()       // 其余天气通道保持晴天默认值。
    }; // 从晴天输出构造厚云样本。
    app.insert_resource(output); // 提供云幕同步系统必需的连续天气输出。
    app.insert_resource(State::new(ScenePhase::Active)); // 动态场景玩法场景允许F1强制视觉天气。
    app.add_systems(Update, sync_weather_cloud_curtain); // 只注册被测云幕系统。
    app.world_mut().spawn((PrimarySceneView, Transform::default())); // 提供可定位玩法主相机但不提供Camera渲染组件。
    app.update(); // 缺少Image、Mesh和Material容器时应安全返回。
    let count = {
        let world = app.world_mut(); // 在单个可变世界借用中创建并执行标记查询。
        let mut query = world.query::<&WeatherCloudCurtain>(); // 查询可能错误生成的云幕实体。
        query.iter(world).count() // 统计实际实例数量并在块末释放借用。
    }; // 完成无渲染资源云幕计数。
    assert_eq!(count, 0); // 不得创建缺材质或缺网格的半成品天空实体。
} // 结束无渲染资产安全降级测试。

#[test] // 厚云只生成天气层单实例，晴天隐藏，进入菜单后彻底清理。
fn cloud_curtain_is_single_layered_hidden_when_clear_and_removed_in_menu() {
    let mut app = App::new(); // 使用最小ECS世界避免初始化真实GPU渲染器。
    app.insert_resource(Assets::<Image>::default()); // 提供程序噪声纹理资产容器。
    app.insert_resource(Assets::<Mesh>::default()); // 提供低模云穹网格资产容器。
    app.insert_resource(Assets::<DynamicCloudMaterial>::default()); // 提供世界视线动态云材质资产容器。
    let output = WeatherOutput {
        cloud_cover: 0.9, // 使用高云量触发云幕创建。
        wind: 0.8,        // 使用强风验证滚动参数路径不会影响实例去重。
        ..default()       // 其余天气通道保持晴天默认值。
    }; // 从有效晴天输出构造阴雨云量。
    app.insert_resource(output); // 提供被测云幕系统的连续输入。
    app.insert_resource(State::new(ScenePhase::Active)); // 动态场景玩法场景允许F1天气表现。
    app.add_systems(Update, sync_weather_cloud_curtain); // 注册唯一云幕同步系统。
    let gameplay_camera = app
        .world_mut()
        .spawn((
            PrimarySceneView,                       // 标记为项目玩法主相机。
            Transform::from_xyz(4.0, 6.0, 8.0), // 使用非零坐标验证相机跟随定位。
        ))
        .id(); // 保存相机实体以验证天空盒模式关闭动态云。
    app.update(); // 首帧创建程序纹理、材质、网格和唯一云幕。
    app.update(); // 第二帧验证系统复用而不会重复创建实例。
    let (entities, translation, layers, visibility) = {
        let world = app.world_mut(); // 单个可变世界借用中收集云幕轻量断言值。
        let mut query = world.query_filtered::<
                (Entity, &Transform, &RenderLayers, &Visibility), // 读取实例ID、定位、渲染层和可见性。
                With<WeatherCloudCurtain>, // 只匹配天气云幕实体。
            >(); // 完成云幕查询构造。
        let values = query
            .iter(world) // 遍历理论上唯一的云幕实例。
            .map(|(entity, transform, layers, visibility)| {
                (entity, transform.translation, layers.clone(), *visibility) // 复制所有断言所需轻量字段。
            }) // 结束单实例字段复制。
            .collect::<Vec<_>>(); // 收集后释放世界借用。
        (
            values.len(),        // 保存实例数量验证全局唯一。
            values[0].1,         // 保存相机跟随后的云幕世界位置。
            values[0].2.clone(), // 保存专属渲染层位集。
            values[0].3,         // 保存阴雨阶段可见性。
        ) // 返回测试断言元组。
    }; // 完成阴雨云幕实例读取。
    assert_eq!(entities, 1); // 连续同步必须只保留一个云幕实体。
    assert_eq!(translation, Vec3::new(4.0, 6.0, 8.0)); // 云穹中心与主相机完全重合，低仰角不会露出边缘。
    assert!(layers.intersects(&RenderLayers::layer(WEATHER_EFFECT_LAYER))); // 云幕必须进入天气专属层。
    assert!(!layers.intersects(&RenderLayers::layer(0))); // 云幕不得泄漏到默认小场景或辅助相机层。
    assert_eq!(visibility, Visibility::Visible); // 阴雨高云量必须显示云幕。
    app.world_mut().entity_mut(gameplay_camera).insert((
        AuthoredSkyboxRequest::new_with_mode(
            AuthoredSkyMode::Skybox,     // 场景明确选择离线可见背景。
            Some("sky.ktx2".to_owned()), // 模拟已经完整采用的主 Cube 身份。
            1_000.0,                     // 天空盒日间亮度。
            300.0,                       // IBL 漫反射强度。
            1_000.0,                     // IBL 镜面强度。
            0.0,                         // 天空与 IBL 默认方向。
            4,                           // 当前作者修订。
        ),
        AuthoredSkyboxStatus {
            phase: AuthoredSkyboxPhase::Ready,       // 三件套已经完整采用。
            asset_path: Some("sky.ktx2".to_owned()), // 状态与请求主身份一致。
            revision: 4,                             // 状态与请求修订一致。
            detail: String::new(),                   // 测试不依赖文案。
            visible_skybox_active: true,             // 动态云关闭所需的真实采用证据。
        },
    ));
    app.update(); // 天空盒内可能已有烘焙云，必须销毁独立动态云幕。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherCloudCurtain>>()
            .iter(app.world())
            .count(),
        0
    ); // 天空盒模式不保留隐藏透明实体或重复云层。
    app.world_mut()
        .entity_mut(gameplay_camera)
        .get_mut::<AuthoredSkyboxStatus>()
        .unwrap()
        .visible_skybox_active = false; // 模拟主 Cube 加载失败后回退动态天空。
    app.update(); // 同一阴天输出必须立即重新建立动态云。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherCloudCurtain>>()
            .iter(app.world())
            .count(),
        1
    ); // 失败回退不能把阴天云层永久关闭。
    app.world_mut().resource_mut::<WeatherOutput>().cloud_cover = 0.05; // 切回设计晴天基础薄云量。
    app.update(); // 同步晴天后保留缓存实体但隐藏透明批次。
    let hidden = {
        let world = app.world_mut(); // 创建查询读取晴天阶段唯一云幕可见性。
        let mut query = world.query_filtered::<&Visibility, With<WeatherCloudCurtain>>(); // 只匹配天气云幕。
        *query.single(world).unwrap() // 复制唯一实例可见性并释放借用。
    }; // 完成晴天隐藏状态读取。
    assert_eq!(hidden, Visibility::Hidden); // 晴天不得显示程序云幕。
    app.world_mut()
        .insert_resource(State::new(ScenePhase::Idle)); // 模拟从玩法场景返回主菜单。
    app.update(); // 菜单阶段必须销毁可能残留的隐藏云幕实体。
    let remaining = {
        let world = app.world_mut(); // 创建最终查询确认菜单隔离。
        let mut query = world.query::<&WeatherCloudCurtain>(); // 查询所有残留天气云幕。
        query.iter(world).count() // 统计并释放世界借用。
    }; // 完成菜单残留云幕计数。
    assert_eq!(remaining, 0); // 主菜单不得保留或渲染玩法天气云幕。
    app.world_mut().resource_mut::<WeatherOutput>().cloud_cover = 0.6; // 返回玩法前切换到与离场缓存不同的可见云量。
    app.world_mut()
        .insert_resource(State::new(ScenePhase::Active)); // 模拟重新进入动态场景玩法场景。
    let expected_opacity = cloud_curtain_profile(
        app.world().resource::<WeatherOutput>(), // 使用重新进入时的最新连续天气输出。
        None,                                    // 最小测试没有ClearColor资源。
        true,                                    // 动态场景属于允许天气表现的玩法场景。
        None,                                    // 最小ECS测试没有天体方向光。
    )
    .opacity; // 记录新实体首帧应写入的材质透明度。
    app.update(); // 复用缓存资产重新创建云幕实体。
    let recreated_material = {
        let world = app.world_mut(); // 查询新云幕绑定的实际材质句柄。
        let mut query = world.query_filtered::<
                &MeshMaterial3d<DynamicCloudMaterial>, // 只读取重新生成实体的动态云材质句柄。
                With<WeatherCloudCurtain>,         // 排除世界中的其他标准材质实体。
            >(); // 完成重进云幕材质查询构造。
        query.single(world).unwrap().0.clone() // 复制唯一句柄并释放查询借用。
    }; // 完成重建云幕材质句柄读取。
    let recreated_opacity = app
        .world()
        .resource::<Assets<DynamicCloudMaterial>>()
        .get(&recreated_material)
        .unwrap()
        .params
        .motion_cover
        .w; // 读取创建帧已经写入 GPU Uniform 的最新最大不透明度。
    assert!((recreated_opacity - expected_opacity).abs() < 0.000_001); // 重进首帧不得短暂沿用晴天零透明度或旧天气色调。
} // 结束云幕单例、图层、晴天隐藏和菜单清理测试。

#[test] // F12测绘视图不能留下固定七百五十米云穹硬边，返回透视后必须重新建立正式云幕。
fn authoring_orthographic_view_removes_cloud_curtain_and_perspective_restores_it() {
    let mut app = App::new(); // 最小ECS世界验证作者相机投影切换，不初始化真实GPU。
    app.insert_resource(Assets::<Image>::default());
    app.insert_resource(Assets::<Mesh>::default());
    app.insert_resource(Assets::<DynamicCloudMaterial>::default());
    app.insert_resource(WeatherOutput {
        cloud_cover: 0.9, // 使用明显厚云确保透视阶段确实生成可见云幕。
        ..default()
    });
    app.insert_resource(State::new(ScenePhase::Active)); // 使用动态场景验证作者视图与玩法天气切换。
    app.insert_resource(WeatherAuthoringPreview::new(true));
    app.add_systems(Update, sync_weather_cloud_curtain);
    let preview_camera = app
        .world_mut()
        .spawn((
            WeatherPreviewCamera,
            Transform::from_xyz(4.0, 6.0, 8.0),
            Projection::Perspective(PerspectiveProjection::default()),
        ))
        .id();

    app.update();
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherCloudCurtain>>()
            .iter(app.world())
            .count(),
        1
    ); // F12透视必须保留与玩法画面一致的正式云幕。

    app.world_mut()
        .entity_mut(preview_camera)
        .insert(Projection::Orthographic(
            OrthographicProjection::default_3d(),
        ));
    app.update();
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherCloudCurtain>>()
            .iter(app.world())
            .count(),
        0
    ); // 顶、前、侧测绘视图必须销毁相机局部云穹，而不是仅隐藏旧实例。

    app.world_mut()
        .entity_mut(preview_camera)
        .insert(Projection::Perspective(PerspectiveProjection::default()));
    app.update();
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherCloudCurtain>>()
            .iter(app.world())
            .count(),
        1
    ); // 切回透视后复用缓存资产自动恢复唯一云幕。
}
