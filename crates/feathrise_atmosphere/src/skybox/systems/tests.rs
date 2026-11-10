//! 动态天空 IBL 两件套与天空盒三件套回归：验证路径、格式、原子采用和失败恢复。

use bevy::asset::RenderAssetUsages; // 构造主世界可验证且渲染世界可采样的 Image。
use bevy::core_pipeline::Skybox; // 分别确认动态模式不写主 Cube、天空盒模式原子写入并支持精确回退。
use bevy::render::render_resource::{
    Extent3d, TextureFormat, TextureUsages, TextureViewDescriptor,
}; // 精确表达六面 Cube 元数据。

use super::*; // 测试正式路径、格式门和原子采用系统。
use crate::skybox::components::AuthoredSkyboxAssetPaths; // 构造 IBL 主身份与两张派生路径。
use crate::skybox::environment::AuthoredEnvironmentMapAssets; // 采用系统需要共享黑 Cube 与比例缓存。

/// 构造保留 CPU 像素的合法 RGBA16F 单 Mip 辐照 Cube。
fn diffuse_cube_image(edge: u32) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: edge,              // 每个 Cube 面使用相同边长。
            height: edge,             // Cube 面必须为正方形。
            depth_or_array_layers: 6, // 正负 XYZ 六面完整存在。
        },
        TextureDimension::D2, // Cube 底层仍为二维数组。
        vec![0; edge as usize * edge as usize * 6 * 8], // RGBA16F 每像素八字节。
        TextureFormat::Rgba16Float, // 独立补光比例派生要求半浮点格式。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 同时允许派生与 GPU 采样。
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube), // EnvironmentMapLight 要求 Cube 视图。
        ..default()
    });
    image // 返回可通过正式漫反射严格格式门的测试图。
}

/// 从合法辐照 Cube 构造具有完整粗糙度 Mip 链的镜面 Cube。
fn specular_cube_image(edge: u32) -> Image {
    let mut image = diffuse_cube_image(edge); // 复用相同格式、六面与采样用途。
    image.data = None; // 镜面采用不在 CPU 复制高分辨率像素。
    image.texture_descriptor.mip_level_count = edge.ilog2() + 1; // 从原尺寸完整二分到一像素。
    image
}

#[test]
fn request_derives_only_two_runtime_ibl_paths_and_rejects_invalid_inputs() {
    let request = AuthoredSkyboxRequest::new(
        Some("textures/test_skybox.ktx2".to_owned()), // 主路径在动态模式提供派生身份，天空盒模式还提供可见Cube。
        1_000.0,                                         // 天空盒切换用亮度保持可往返验证。
        300.0,                                           // 漫反射作者日间强度。
        1_000.0,                                         // 镜面作者日间强度。
        15.0,                                            // IBL 绕 Y 轴方向。
        1,                                               // 当前作者修订。
    );
    assert!(request.validate().is_ok()); // 合法主路径仍兼容旧场景清单。
    let paths = request.asset_paths().unwrap(); // 运行时只派生两张真正需要加载的资源。
    assert_eq!(paths.source, "textures/test_skybox.ktx2"); // 主路径同时是资源组身份与可选可见Cube路径。
    assert_eq!(paths.diffuse, "textures/test_skybox_diffuse.ktx2"); // 固定余弦卷积后缀。
    assert_eq!(paths.specular, "textures/test_skybox_specular.ktx2"); // 固定 GGX 后缀。
    let skybox_request = AuthoredSkyboxRequest::new_with_mode(
        AuthoredSkyMode::Skybox,    // 显式天空盒模式要求主路径同时作为可见 Cube。
        request.asset_path.clone(), // 复用同一合法主资源身份。
        1_000.0,                    // 天空盒日间可见亮度。
        300.0,                      // IBL 漫反射基准。
        1_000.0,                    // IBL 镜面基准。
        15.0,                       // 主 Cube 与 IBL 共享方向。
        5,                          // 独立作者修订。
    );
    assert!(skybox_request.validate().is_ok()); // 有主路径的天空盒请求可进入三件套加载。
    let missing_visible = AuthoredSkyboxRequest::new_with_mode(
        AuthoredSkyMode::Skybox, // 显式请求可见 Cube。
        None,                    // 缺少主资源时不得只加载派生图。
        1_000.0,
        300.0,
        1_000.0,
        0.0,
        6,
    );
    assert!(missing_visible.validate().is_err()); // 天空盒模式不能进入无主 Cube 的半套状态。

    let png = AuthoredSkyboxRequest::new(
        Some("textures/sky.png".to_owned()),
        1_000.0,
        300.0,
        1_000.0,
        0.0,
        2,
    ); // 普通二维图没有环境 Cube 派生协议。
    assert!(png.validate().is_err()); // 错误路径不得进入 AssetServer。
    let invalid_number = AuthoredSkyboxRequest::new(
        Some("sky.ktx2".to_owned()),
        f32::NAN,
        300.0,
        1_000.0,
        0.0,
        3,
    ); // 天空盒亮度即使在动态模式暂不生效也必须保持有限。
    assert!(invalid_number.validate().is_err()); // 损坏清单不能静默往返。
    let derived = AuthoredSkyboxRequest::new(
        Some("sky_specular.ktx2".to_owned()),
        1_000.0,
        300.0,
        1_000.0,
        0.0,
        4,
    ); // 派生图不能再次作为主身份。
    assert!(derived.validate().is_err()); // 防止重复后缀和永远缺失的资源。
}

#[test]
fn cube_validation_requires_six_square_filterable_render_layers() {
    let mut image = Image::new_uninit(
        Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST; // 模拟可采样加载资产。
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    assert!(validate_cube_image(&image).is_ok()); // 完整六面可过滤 Cube 通过通用门。
    image.texture_descriptor.size.depth_or_array_layers = 1; // 普通二维图不是环境 Cube。
    assert!(validate_cube_image(&image).is_err());
    image.texture_descriptor.size.depth_or_array_layers = 6;
    image.texture_descriptor.size.height = 8; // 非正方形不能成为 Cube。
    assert!(validate_cube_image(&image).is_err());
    image.texture_descriptor.size.height = 16;
    image.texture_descriptor.format = TextureFormat::Rgba8Uint; // 整数格式不能由过滤采样器读取。
    assert!(validate_cube_image(&image).is_err());
    image.texture_descriptor.format = TextureFormat::Rgba16Float;
    image.texture_descriptor.usage = TextureUsages::COPY_DST; // 缺失 TEXTURE_BINDING。
    assert!(validate_cube_image(&image).is_err());
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING;
    image.asset_usage = RenderAssetUsages::MAIN_WORLD; // 缺失渲染世界资格。
    assert!(validate_cube_image(&image).is_err());
}

#[test]
fn filtered_ibl_cubes_are_shared_between_two_cameras_without_visible_cube() {
    let mut app = App::new(); // 纯 ECS 测试原子采用，不启动 RenderApp。
    app.init_resource::<Assets<Image>>();
    app.init_resource::<AuthoredEnvironmentMapAssets>();
    app.add_systems(Update, apply_authored_skyboxes);
    let diffuse = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(diffuse_cube_image(16)); // 小型 CPU 辐照图允许编码三成比例。
    let specular = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(specular_cube_image(16)); // 完整五级 GGX 链保持原句柄。
    let original_skybox = Skybox {
        brightness: 77.0,
        ..default()
    }; // IBL 加载器必须完全忽略可见背景。
    let mut cameras = Vec::new();
    for _ in 0..2 {
        cameras.push(
            app.world_mut()
                .spawn((
                    original_skybox.clone(), // 动态天空系统将另行接管，当前加载器不得触碰。
                    AuthoredSkyboxRuntime {
                        original_skybox: Some(original_skybox.clone()), // 动态 IBL 采用后仍保持相机原可见背景。
                        original_sky_profile: None, // 测试相机没有外部天空昼夜标记。
                        original_environment_map: None,
                        original_generated_environment_map: None,
                        original_environment_profile: None,
                        active_source: None, // 首次采用前没有上一套作者环境。
                        active_mode: None,   // 首次采用前没有作者可见模式。
                        active_visible_handle: None, // 动态模式不持有主 Cube。
                        tracked: Some(TrackedAuthoredSkybox {
                            mode: AuthoredSkyMode::Dynamic, // 只采用 IBL 两件套。
                            paths: AuthoredSkyboxAssetPaths {
                                source: "textures/test_skybox.ktx2".to_owned(),
                                diffuse: "textures/test_skybox_diffuse.ktx2".to_owned(),
                                specular: "textures/test_skybox_specular.ktx2".to_owned(),
                            },
                            visible_handle: None, // 动态天空不得请求可见主 Cube。
                            diffuse_handle: diffuse.clone(),
                            specular_handle: specular.clone(),
                            revision: 7,
                            base_brightness: 1_000.0, // 动态模式只保留切换用亮度。
                            base_environment_diffuse_intensity: 300.0,
                            base_environment_specular_intensity: 1_000.0,
                            rotation: Quat::IDENTITY,
                        }),
                    },
                    AuthoredSkyboxStatus {
                        phase: AuthoredSkyboxPhase::Loading,
                        asset_path: Some("textures/test_skybox.ktx2".to_owned()),
                        revision: 7,
                        detail: String::new(),
                        visible_skybox_active: false, // 动态模式继续由大气和云提供背景。
                    },
                ))
                .id(),
        );
    }
    app.update(); // 两台相机在同一帧采用同一离线资源。
    let first = app.world().get::<EnvironmentMapLight>(cameras[0]).unwrap();
    let second = app.world().get::<EnvironmentMapLight>(cameras[1]).unwrap();
    assert_eq!(first.specular_map.id(), specular.id()); // 高分辨率镜面不复制。
    assert_eq!(second.specular_map.id(), specular.id()); // 两台相机复用 GGX 句柄。
    assert_ne!(first.diffuse_map.id(), diffuse.id()); // 三成补光编码到小型派生图。
    assert_eq!(second.diffuse_map.id(), first.diffuse_map.id()); // 两台相机共享同一比例派生图。
    assert_eq!(first.intensity, 1_000.0); // 公共 Uniform 承载完整镜面作者值。
    assert_eq!(
        app.world().get::<Skybox>(cameras[0]).unwrap().brightness,
        77.0
    ); // 加载 IBL 不写可见 Cube。
    assert_eq!(
        app.world()
            .get::<AuthoredSkyboxStatus>(cameras[0])
            .unwrap()
            .phase,
        AuthoredSkyboxPhase::Ready
    );
}

#[test]
fn skybox_mode_atomically_applies_visible_cube_and_both_ibl_maps() {
    let mut app = App::new(); // 纯 ECS 世界直接验证三件套采用边界。
    app.init_resource::<Assets<Image>>(); // 主 Cube 与两张 IBL 共用测试资产表。
    app.init_resource::<AuthoredEnvironmentMapAssets>(); // 漫反射独立强度需要共享派生缓存。
    app.add_systems(Update, apply_authored_skyboxes); // 只运行正式原子采用系统。
    let (visible, diffuse, specular) = {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>(); // 一次借用内登记三张合法 Cube。
        (
            images.add(diffuse_cube_image(16)), // 可见主 Cube 只要求通用采样契约。
            images.add(diffuse_cube_image(16)), // 辐照 Cube 保持单 Mip 与 CPU 半浮点像素。
            images.add(specular_cube_image(16)), // 镜面 Cube 提供完整五级 Mip 链。
        )
    };
    let rotation = Quat::from_rotation_y(0.45); // 使用非零方向验证主 Cube 与 IBL 同步。
    let entity = app
        .world_mut()
        .spawn((
            AuthoredSkyboxRuntime {
                original_skybox: None,                    // 测试相机初始使用动态天空。
                original_sky_profile: None,               // 初始没有外部可见亮度基准。
                original_environment_map: None,           // 初始没有外部 IBL。
                original_generated_environment_map: None, // 初始没有实时环境生成器。
                original_environment_profile: None,       // 初始没有外部 IBL 昼夜基准。
                active_source: None,                      // 首次候选之前没有旧作者结果。
                active_mode: None,                        // 首次候选之前没有旧模式。
                active_visible_handle: None,              // 首次候选之前没有旧主 Cube。
                tracked: Some(TrackedAuthoredSkybox {
                    mode: AuthoredSkyMode::Skybox, // 要求主 Cube 与两张 IBL 一起采用。
                    paths: AuthoredSkyboxAssetPaths {
                        source: "sky.ktx2".to_owned(),            // 可见主 Cube 身份。
                        diffuse: "sky_diffuse.ktx2".to_owned(),   // 辐照派生身份。
                        specular: "sky_specular.ktx2".to_owned(), // GGX 派生身份。
                    },
                    visible_handle: Some(visible.clone()), // 三件套第一张主 Cube。
                    diffuse_handle: diffuse.clone(),       // 三件套第二张辐照 Cube。
                    specular_handle: specular.clone(),     // 三件套第三张 GGX Cube。
                    revision: 12,                          // 状态与候选使用同一修订。
                    base_brightness: 2_400.0,              // 可见天空日间作者亮度。
                    base_environment_diffuse_intensity: 300.0, // 地物补光作者基准。
                    base_environment_specular_intensity: 1_000.0, // 水面反射作者基准。
                    rotation,                              // 三张图共享同一水平朝向。
                }),
            },
            AuthoredSkyboxStatus {
                phase: AuthoredSkyboxPhase::Loading, // 允许正式系统进入采用边界。
                asset_path: Some("sky.ktx2".to_owned()), // 状态主身份与候选一致。
                revision: 12,                        // 防止迟到状态采用错误候选。
                detail: String::new(),               // 采用后由正式系统写就绪说明。
                visible_skybox_active: false,        // 主 Cube 尚未采用时动态天空仍可回退。
            },
        ))
        .id(); // 保存相机实体读取最终三件套组件。

    app.update(); // 三张图全部合法时在同一帧完成采用。

    let skybox = app.world().get::<Skybox>(entity).unwrap(); // 读取最终可见 Cube。
    assert_eq!(skybox.image.as_ref().unwrap().id(), visible.id()); // 主 Cube 必须使用候选原句柄。
    assert_eq!(skybox.brightness, 2_400.0); // 世界光照运行前先写作者日间基准。
    assert_eq!(skybox.rotation, rotation); // 可见背景方向与请求一致。
    let sky_profile = app.world().get::<WorldSkyProfile>(entity).unwrap(); // 读取昼夜亮度基准。
    assert_eq!(sky_profile.base_brightness, 2_400.0); // 后续每帧从不变量重算亮度。
    let environment = app.world().get::<EnvironmentMapLight>(entity).unwrap(); // 读取同帧采用的 PBR IBL。
    assert_eq!(environment.specular_map.id(), specular.id()); // 镜面保留完整源句柄。
    assert_eq!(environment.rotation, rotation); // IBL 与可见 Cube 不得错位。
    let status = app.world().get::<AuthoredSkyboxStatus>(entity).unwrap(); // 读取公开采用结果。
    assert_eq!(status.phase, AuthoredSkyboxPhase::Ready); // 完整三件套进入就绪状态。
    assert!(status.visible_skybox_active); // 动态大气与云应据此让出，模拟天体不消费此状态。
}

#[test]
fn invalid_diffuse_restores_original_environment_but_leaves_skybox_untouched() {
    let mut app = App::new();
    app.init_resource::<Assets<Image>>();
    app.init_resource::<AuthoredEnvironmentMapAssets>();
    app.add_systems(Update, apply_authored_skyboxes);
    let (invalid_diffuse, valid_specular, original_diffuse, original_specular, original_generated) = {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let mut invalid = diffuse_cube_image(16);
        invalid.texture_descriptor.size.depth_or_array_layers = 1; // 故意破坏 Cube 六面契约。
        (
            images.add(invalid),
            images.add(specular_cube_image(16)),
            images.add(Image::default()),
            images.add(Image::default()),
            images.add(Image::default()),
        )
    };
    let rotation = Quat::from_rotation_y(0.35);
    let original_environment = EnvironmentMapLight {
        diffuse_map: original_diffuse.clone(),
        specular_map: original_specular.clone(),
        intensity: 654.0,
        rotation,
        affects_lightmapped_mesh_diffuse: false,
    };
    let original_generated_environment = GeneratedEnvironmentMapLight {
        environment_map: original_generated.clone(),
        intensity: 987.0,
        rotation,
        affects_lightmapped_mesh_diffuse: false,
    };
    let visible = Skybox {
        brightness: 432.0,
        rotation,
        ..default()
    }; // 动态天空所有权之外的组件不能被 IBL 失败路径改写。
    let entity = app
        .world_mut()
        .spawn((
            visible.clone(),
            EnvironmentMapLight::default(),
            WorldEnvironmentMapProfile::new(1.0, 1.0),
            AuthoredSkyboxRuntime {
                original_skybox: Some(visible.clone()), // IBL 失败时可见背景必须精确归还。
                original_sky_profile: None,             // 测试没有外部天空昼夜基准。
                original_environment_map: Some(original_environment.clone()),
                original_generated_environment_map: Some(original_generated_environment.clone()),
                original_environment_profile: Some(WorldEnvironmentMapProfile::new(333.0, 777.0)),
                active_source: None, // 首次候选失败，没有上一作者结果可保留。
                active_mode: None,   // 首次候选尚未采用模式。
                active_visible_handle: None, // 动态候选不含主 Cube。
                tracked: Some(TrackedAuthoredSkybox {
                    mode: AuthoredSkyMode::Dynamic, // 失败只涉及 IBL 两件套。
                    paths: AuthoredSkyboxAssetPaths {
                        source: "broken.ktx2".to_owned(),
                        diffuse: "broken_diffuse.ktx2".to_owned(),
                        specular: "broken_specular.ktx2".to_owned(),
                    },
                    visible_handle: None, // 动态模式没有主 Cube 候选。
                    diffuse_handle: invalid_diffuse,
                    specular_handle: valid_specular,
                    revision: 9,
                    base_brightness: 1_000.0, // 兼容亮度不参与本测试画面。
                    base_environment_diffuse_intensity: 300.0,
                    base_environment_specular_intensity: 1_000.0,
                    rotation: Quat::IDENTITY,
                }),
            },
            AuthoredSkyboxStatus {
                phase: AuthoredSkyboxPhase::Loading,
                asset_path: Some("broken.ktx2".to_owned()),
                revision: 9,
                detail: String::new(),
                visible_skybox_active: false, // 无作者可见 Cube。
            },
        ))
        .id();
    app.update(); // 无效漫反射必须撤销整套 IBL 候选。
    let skybox = app.world().get::<Skybox>(entity).unwrap();
    assert_eq!(skybox.brightness, visible.brightness); // IBL 失败不触碰可见背景。
    assert_eq!(skybox.rotation, visible.rotation);
    let environment = app.world().get::<EnvironmentMapLight>(entity).unwrap();
    assert_eq!(environment.diffuse_map.id(), original_diffuse.id());
    assert_eq!(environment.specular_map.id(), original_specular.id());
    assert_eq!(environment.intensity, original_environment.intensity);
    let generated = app
        .world()
        .get::<GeneratedEnvironmentMapLight>(entity)
        .unwrap();
    assert_eq!(generated.environment_map.id(), original_generated.id());
    assert_eq!(
        generated.intensity,
        original_generated_environment.intensity
    );
    let profile = app
        .world()
        .get::<WorldEnvironmentMapProfile>(entity)
        .unwrap();
    assert_eq!(profile.base_diffuse_intensity, 333.0);
    assert_eq!(profile.base_specular_intensity, 777.0);
    assert_eq!(
        app.world()
            .get::<AuthoredSkyboxStatus>(entity)
            .unwrap()
            .phase,
        AuthoredSkyboxPhase::Failed
    );
}

#[test]
fn specular_validation_rejects_single_mip_cube() {
    let mut image = diffuse_cube_image(16); // 通用 Cube 与漫反射格式均合法。
    image.data = None; // 镜面验证不依赖 CPU 像素。
    assert!(validate_cube_image(&image).is_ok()); // 同图满足通用环境 Cube 采样契约。
    assert!(validate_specular_cube_image(&image).is_err()); // 镜面必须包含十六至一像素完整五级链。
}
