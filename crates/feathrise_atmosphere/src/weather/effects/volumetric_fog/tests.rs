// 天气体积雾 — 档位、隔离、作者恢复、新实体继承与稳定帧回归测试。

use super::*; // 访问私有纯参数构造、组件比较和正式同步系统。
use bevy::window::WindowRef; // 构造主窗口渲染目标以验证玩法相机过滤。

/// 建立只运行正式体积雾同步系统的轻量 ECS 应用。
fn test_app() -> App {
    let mut app = App::new(); // 参数与组件所有权测试不初始化真实 GPU 渲染器。
    app.init_resource::<RenderSettings>(); // 默认保持体积雾关闭基线。
    app.insert_resource(WeatherOutput::default()); // 提供完整晴天连续天气输出。
    app.insert_resource(ClearColor(Color::srgb(0.30, 0.45, 0.65))); // 模拟世界时间最终日间天空色。
    app.add_systems(Update, sync_weather_volumetric_fog); // 直接注册被测正式系统并让命令在 Update 末应用。
    app // 返回可逐帧修改设置和实体的测试应用。
}

/// 生成带有限世界姿态的窗口目标玩法主相机。
fn spawn_gameplay_camera(app: &mut App) -> Entity {
    let transform = Transform::from_xyz(19.0, 11.0, -47.0); // 使用非网格位置验证体积分格中心。
    app.world_mut()
        .spawn((
            Camera3d::default(),                      // 建立真实 3D 相机身份。
            PrimarySceneView,                             // 标记为项目唯一玩法主相机。
            RenderTarget::Window(WindowRef::Primary), // 输出主窗口，允许天气体积雾接管。
            transform,                                // 保存相机本地姿态供真实场景使用。
            GlobalTransform::from(transform), // 最小测试没有传播系统，显式提供同值世界姿态。
        ))
        .id() // 返回相机实体供测试逐档断言。
}

/// 生成带世界时间标记的玩法天体方向光。
fn spawn_celestial_light(app: &mut App, authored_volumetric: bool) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            DirectionalLight {
                shadow_maps_enabled: true, // 天体光保留生成遮挡光束所需标准阴影资格。
                ..default()
            }, // 完成方向光作者配置。
            WorldCelestialLight::new(
                20_000.0,                      // 提供正式标记所需日间照度基线。
                Color::WHITE,                  // 使用中性日光色简化测试。
                Color::srgb(0.30, 0.45, 0.65), // 使用与测试天空相同的日景色并保留固定场景旧月光行为。
            ), // 明确该方向光属于玩法天体光。
        ))
        .id(); // 保存实体供可选作者组件和断言使用。
    if authored_volumetric {
        app.world_mut().entity_mut(entity).insert(VolumetricLight); // 模拟作者原本已经启用光束资格。
    }
    entity // 返回方向光实体。
}

#[test]
fn low_high_and_off_restore_authored_camera_and_light() {
    let mut app = test_app(); // 使用完整作者恢复路径。
    let camera = spawn_gameplay_camera(&mut app); // 创建可接管玩法窗口相机。
    let authored_camera_fog = VolumetricFog {
        ambient_color: Color::srgb(0.8, 0.7, 0.6), // 模拟作者暖色环境雾。
        ambient_intensity: 0.12,                   // 模拟作者自定义环境光强度。
        jitter: 0.25,                              // 模拟作者自定义随机起点。
        step_count: 20,                            // 模拟不属于全局低高档的作者预算。
    }; // 完成作者组件快照。
    app.world_mut()
        .entity_mut(camera)
        .insert(authored_camera_fog); // 在首次接管前安装作者体积雾组件。
    let light = spawn_celestial_light(&mut app, true); // 作者原本也启用天体光束资格。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::Low; // 选择三十二步低档。
    app.update(); // 首帧捕获作者状态并应用低档。

    assert_eq!(
        app.world().get::<VolumetricFog>(camera).unwrap().step_count,
        32
    ); // 低档必须映射三十二步。
    assert!(
        app.world()
            .get::<AuthoredGameplayVolumetricFog>(camera)
            .is_some()
    ); // 首次接管必须保存作者相机组件。
    assert!(app.world().get::<VolumetricLight>(light).is_some()); // 作者天体光束资格在接管期间保持存在。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherVolumetricFogVolume>>()
            .iter(app.world())
            .count(),
        1
    ); // 启用档只生成一个有限雾体积。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::High; // 切换到六十四步高档。
    app.update(); // 原地更新相机参数而不重建雾体积实体。
    assert_eq!(
        app.world().get::<VolumetricFog>(camera).unwrap().step_count,
        64
    ); // 高档必须映射六十四步。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::Off; // 关闭全局体积雾。
    app.update(); // 精确恢复作者相机、作者灯光并清理自有体积。
    let restored = app.world().get::<VolumetricFog>(camera).unwrap(); // 作者原本有组件，因此关闭后仍应存在。
    assert!(camera_fog_matches(restored, &authored_camera_fog)); // 全部作者字段必须无损恢复。
    assert!(
        app.world()
            .get::<AuthoredGameplayVolumetricFog>(camera)
            .is_none()
    ); // 恢复后立即归还相机控制权。
    assert!(app.world().get::<VolumetricLight>(light).is_some()); // 作者原有天体光束资格必须恢复保留。
    assert!(
        app.world()
            .get::<AuthoredCelestialVolumetricLight>(light)
            .is_none()
    ); // 方向光作者恢复后清理私有所有权。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherVolumetricFogVolume>>()
            .iter(app.world())
            .count(),
        0
    ); // 关闭档不得保留任何体积雾绘制实体。
}

#[test]
fn gameplay_window_camera_isolated_from_menu_and_texture_cameras() {
    let mut app = test_app(); // 使用正式 PrimarySceneView 与窗口目标双重过滤。
    let gameplay_camera = spawn_gameplay_camera(&mut app); // 唯一合法玩法窗口相机。
    let texture_author = VolumetricFog {
        step_count: 7, // 模拟小地图或角色预览的专用极低预算。
        ..default()
    }; // 完成纹理相机作者配置。
    let texture_camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),                                    // 建立纹理输出 3D 相机。
            PrimarySceneView, // 即使错误保留玩法标记，输出目标仍必须隔离。
            RenderTarget::Image(Handle::<Image>::default().into()), // 输出纹理而非窗口。
            Transform::default(), // 提供相机局部姿态。
            GlobalTransform::default(), // 提供有限世界姿态。
            texture_author, // 保留作者专用体积雾参数。
        ))
        .id(); // 保存纹理相机实体。
    let menu_author = VolumetricFog {
        step_count: 9, // 模拟主菜单作者专用参数。
        ..default()
    }; // 完成菜单相机作者配置。
    let menu_camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),                      // 建立主菜单窗口 3D 相机。
            RenderTarget::Window(WindowRef::Primary), // 虽然输出窗口，但没有 PrimarySceneView。
            Transform::default(),                     // 提供相机局部姿态。
            GlobalTransform::default(),               // 提供有限世界姿态。
            menu_author,                              // 保留主菜单作者体积雾参数。
        ))
        .id(); // 保存主菜单相机实体。
    let light = spawn_celestial_light(&mut app, false); // 作者原本没有天体光束资格。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::High; // 启用高档玩法体积雾。
    app.update(); // 应用正式过滤与所有权捕获。

    assert_eq!(
        app.world()
            .get::<VolumetricFog>(gameplay_camera)
            .unwrap()
            .step_count,
        64
    ); // 合法玩法窗口相机继承当前档位。
    assert_eq!(
        app.world()
            .get::<VolumetricFog>(texture_camera)
            .unwrap()
            .step_count,
        7
    ); // 纹理相机保持作者专用配置。
    assert!(
        app.world()
            .get::<AuthoredGameplayVolumetricFog>(texture_camera)
            .is_none()
    ); // 纹理相机不得附加天气作者备份。
    assert_eq!(
        app.world()
            .get::<VolumetricFog>(menu_camera)
            .unwrap()
            .step_count,
        9
    ); // 主菜单窗口相机没有玩法标记，保持作者配置。
    assert!(app.world().get::<VolumetricLight>(light).is_some()); // 合法玩法视图存在时接管天体光。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::Off; // 关闭并归还原本缺失的作者状态。
    app.update(); // 执行相机、灯光和体积清理。
    assert!(app.world().get::<VolumetricFog>(gameplay_camera).is_none()); // 作者原本无组件时完整移除天气体积雾。
    assert!(app.world().get::<VolumetricLight>(light).is_none()); // 作者原本无资格时完整移除天气光束标记。
}

#[test]
fn new_gameplay_camera_and_celestial_light_inherit_current_quality() {
    let mut app = test_app(); // 验证切图后新实体无需额外初始化钩子。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::High; // 在场景生成前先选择高档。
    app.update(); // 没有相机或灯光时系统必须安全跳过。
    let camera = spawn_gameplay_camera(&mut app); // 模拟切图后新生成玩法主相机。
    let light = spawn_celestial_light(&mut app, false); // 模拟切图后新生成天体方向光。
    app.update(); // 下一帧自动接管新实体并创建唯一雾体积。
    assert_eq!(
        app.world().get::<VolumetricFog>(camera).unwrap().step_count,
        64
    ); // 新相机自动继承高档步进数。
    assert!(app.world().get::<VolumetricLight>(light).is_some()); // 新天体方向光自动继承光束资格。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherVolumetricFogVolume>>()
            .iter(app.world())
            .count(),
        1
    ); // 新相机出现后自动建立唯一有限雾体积。
}

#[test]
fn weather_visibility_controls_density_and_taa_controls_jitter() {
    let clear = WeatherOutput::default(); // 晴天使用完整能见度与中性环境光。
    let mist = WeatherOutput {
        visibility: 0.35, // 使用正式薄雾最低能见度目标。
        ..clear           // 只改变影响体积密度的天气字段。
    }; // 完成薄雾连续输出样本。
    let sky = Color::srgb(0.30, 0.45, 0.65); // 使用相同最终天空色隔离能见度影响。
    let clear_volume = fog_volume_for_weather(weather_fog_profile(&clear, sky, true)); // 构造晴天体积参数。
    let mist_volume = fog_volume_for_weather(weather_fog_profile(&mist, sky, true)); // 构造薄雾体积参数。
    assert!(mist_volume.density_factor > clear_volume.density_factor); // 能见度越低体积密度必须连续增加。

    let mut settings = RenderSettings::default(); // 默认 MSAA 不允许随机起点抖动。
    let msaa = camera_fog_for_quality(64, Some(&settings), &clear, sky); // 构造默认高档相机参数。
    assert_eq!(msaa.jitter, 0.0); // 没有 TAA 时必须关闭体积雾随机抖动。
    settings.graphics.set_anti_aliasing(AntiAliasingMode::Taa); // 切到实验性跨帧累积抗锯齿。
    let taa = camera_fog_for_quality(64, Some(&settings), &clear, sky); // 重新构造同一步进预算参数。
    assert_eq!(taa.jitter, 0.5); // TAA 使用 Bevy 官方示例的体积雾抖动强度。
}

#[test]
fn stable_inputs_do_not_rewrite_camera_volume_transform_or_light() {
    let mut app = test_app(); // 使用相同设置、天气、天空与相机姿态连续运行。
    let camera = spawn_gameplay_camera(&mut app); // 创建合法玩法相机。
    let light = spawn_celestial_light(&mut app, false); // 创建需要首次接管的天体光。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::Low; // 固定低档三十二步。
    app.update(); // 首帧合法插入相机、灯光、作者备份和唯一体积。
    let volume = app
        .world_mut()
        .query_filtered::<Entity, With<WeatherVolumetricFogVolume>>()
        .single(app.world())
        .expect("启用档应生成唯一体积"); // 保存唯一体积实体供稳定帧变更检测。
    app.world_mut().clear_trackers(); // 清除首帧合法组件插入和参数写入标记。
    app.update(); // 相同输入运行第二帧，不应重复修改任何渲染组件。

    assert!(
        !app.world()
            .entity(camera)
            .get_ref::<VolumetricFog>()
            .unwrap()
            .is_changed()
    ); // 稳定档位不得重复写相机体积雾组件。
    assert!(
        !app.world()
            .entity(volume)
            .get_ref::<FogVolume>()
            .unwrap()
            .is_changed()
    ); // 稳定天气不得重复写雾体积 uniform 参数。
    assert!(
        !app.world()
            .entity(volume)
            .get_ref::<Transform>()
            .unwrap()
            .is_changed()
    ); // 相机未跨分格时不得重复写雾体积 Transform。
    assert!(
        !app.world()
            .entity(light)
            .get_ref::<VolumetricLight>()
            .unwrap()
            .is_changed()
    ); // 稳定帧不得重复插入天体光束资格。
}

#[test]
fn snapped_volume_center_updates_only_after_crossing_grid_boundary() {
    let first = snapped_volume_transform(Vec3::new(1.0, 3.0, 1.0)); // 位于原点网格中心附近。
    let same_cell = snapped_volume_transform(Vec3::new(15.0, 3.0, -15.0)); // 仍未越过最近水平网格中点。
    let next_cell = snapped_volume_transform(Vec3::new(17.0, 5.0, -17.0)); // 同时越过水平与垂直最近网格中点。
    assert_eq!(first, same_cell); // 同格移动保持完全相同 Transform，稳定帧不会触发写入。
    assert_ne!(first.translation, next_cell.translation); // 跨格后才更新体积中心以继续包围相机。
    assert_eq!(next_cell.scale, VOLUME_SCALE); // 任意跟随位置都保持固定有限世界尺寸。
}

#[test] // F12正交测绘视图必须归还相机体积雾并清理有限雾体积，返回透视后完整恢复。
fn authoring_orthographic_view_releases_volumetric_fog_and_perspective_restores_it() {
    let mut app = test_app();
    app.insert_resource(WeatherAuthoringPreview::new(true));
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .volumetric_fog_quality = VolumetricFogQuality::High;
    let preview_camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            WeatherPreviewCamera,
            RenderTarget::Image(Handle::<Image>::default().into()),
            Projection::Perspective(PerspectiveProjection::default()),
            Transform::from_xyz(4.0, 6.0, 8.0),
        ))
        .id();

    app.update();
    assert_eq!(
        app.world()
            .get::<VolumetricFog>(preview_camera)
            .unwrap()
            .step_count,
        64
    ); // F12透视采用当前高档正式体积雾。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherVolumetricFogVolume>>()
            .iter(app.world())
            .count(),
        1
    ); // 透视阶段只保留唯一相机局部雾体积。

    app.world_mut()
        .entity_mut(preview_camera)
        .insert(Projection::Orthographic(
            OrthographicProjection::default_3d(),
        ));
    app.update();
    assert!(app.world().get::<VolumetricFog>(preview_camera).is_none()); // 作者原本无组件，正交时必须完整归还为空。
    assert!(
        app.world()
            .get::<AuthoredGameplayVolumetricFog>(preview_camera)
            .is_none()
    ); // 归还完成不得残留天气系统所有权标记。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherVolumetricFogVolume>>()
            .iter(app.world())
            .count(),
        0
    ); // 正交视图不得残留有限雾体积遮住大地图。

    app.world_mut()
        .entity_mut(preview_camera)
        .insert(Projection::Perspective(PerspectiveProjection::default()));
    app.update();
    assert_eq!(
        app.world()
            .get::<VolumetricFog>(preview_camera)
            .unwrap()
            .step_count,
        64
    ); // 返回透视后重新取得同一正式质量档。
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherVolumetricFogVolume>>()
            .iter(app.world())
            .count(),
        1
    ); // 返回透视后重新建立唯一有限雾体积。
}
