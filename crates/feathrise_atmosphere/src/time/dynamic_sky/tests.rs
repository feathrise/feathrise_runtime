//! 动态天空最小 ECS 回归：不启动窗口或 RenderApp 即可验证相机所有权与大气单例。

use bevy::camera::Hdr; // 验证 AtmosphereSettings required HDR 的创建与清理。
use bevy::core_pipeline::Skybox; // 构造接管前可见 Cube 并验证离场恢复。
use bevy::light::{Atmosphere, atmosphere::ScatteringMedium}; // 统计唯一地球大气实体。
use bevy::pbr::AtmosphereSettings; // 检查玩法与 F12 相机资格。
use bevy::prelude::*; // App、State、Projection 与基础 ECS 类型。
use feathrise_scene_core::view::PrimarySceneView; // 正式玩法主相机标记。
use feathrise_scene_core::state::ScenePhase; // 切换玩法与菜单场景。

use super::components::DynamicSkyCameraRuntime; // 验证所有权快照在离场后释放。
use super::systems::sync_dynamic_sky; // 直接注册正式同步系统。
use crate::{
    AuthoredSkyMode, AuthoredSkyboxPhase, AuthoredSkyboxRequest, AuthoredSkyboxStatus,
    WeatherAuthoringPreview, WeatherPreviewCamera,
}; // 构造 F12 作者视图与已采用天空盒资格。

/// 构造具有散射介质资产表的最小动态天空应用。
fn sky_app(scene: ScenePhase) -> App {
    let mut app = App::new(); // 不安装真实渲染器，只验证主世界组件生命周期。
    app.insert_resource(Assets::<ScatteringMedium>::default()); // 允许系统延迟创建 Earth 介质。
    app.insert_resource(State::new(scene)); // 当前场景决定相机是否取得动态天空。
    app.insert_resource(WeatherAuthoringPreview::new(false)); // 默认 F12 关闭。
    app.add_systems(Update, sync_dynamic_sky); // 注册唯一动态天空同步入口。
    app
}

#[test]
fn gameplay_camera_retires_visible_skybox_and_restores_it_in_menu() {
    let mut app = sky_app(ScenePhase::Active); // HN-01 正式玩法场景必须启用动态天空。
    let original = Skybox {
        brightness: 321.0,                     // 使用可辨识作者亮度验证完整恢复。
        rotation: Quat::from_rotation_y(0.42), // 使用非零方向验证完整恢复。
        ..default()
    };
    let camera = app.world_mut().spawn((PrimarySceneView, original.clone())).id(); // 最小测试省略真实 Camera，系统仍按玩法标记接管。
    app.update(); // 首帧安装原生大气设置、HDR 并移除可见 Cube。
    assert!(app.world().get::<AtmosphereSettings>(camera).is_some()); // 玩法相机必须取得动态大气。
    assert!(app.world().get::<Hdr>(camera).is_some()); // 原生大气 required 关系必须建立 HDR。
    assert!(app.world().get::<Skybox>(camera).is_none()); // 烘焙天空和其中静态云不得继续显示。
    assert!(app.world().get::<DynamicSkyCameraRuntime>(camera).is_some()); // 原状态仍由运行时快照持有。
    let atmosphere_count = app
        .world_mut()
        .query::<&Atmosphere>()
        .iter(app.world())
        .count(); // 首次玩法相机只生成一个地球大气。
    assert_eq!(atmosphere_count, 1); // 动态天空全局实体必须唯一。

    app.world_mut()
        .insert_resource(State::new(ScenePhase::Idle)); // 模拟返回主菜单但相机尚未销毁。
    app.update(); // 失去玩法资格后精确归还作者状态。
    let restored = app.world().get::<Skybox>(camera).unwrap(); // 原可见 Cube 应重新存在。
    assert_eq!(restored.brightness, original.brightness); // 作者亮度保持原值。
    assert_eq!(restored.rotation, original.rotation); // 作者方向保持原值。
    assert!(app.world().get::<AtmosphereSettings>(camera).is_none()); // 本领域新增的大气设置已移除。
    assert!(app.world().get::<Hdr>(camera).is_none()); // 本领域新增的 HDR 已移除。
    assert!(app.world().get::<DynamicSkyCameraRuntime>(camera).is_none()); // 离场后不残留所有权。
}

#[test]
fn ready_authored_skybox_suspends_atmosphere_without_overwriting_cube() {
    let mut app = sky_app(ScenePhase::Active); // 动态场景允许两种互斥可见天空来源。
    let authored = Skybox {
        brightness: 1_234.0, // 使用可辨识亮度确认动态系统没有恢复旧快照覆盖它。
        rotation: Quat::from_rotation_y(0.31), // 非零旋转确认完整主 Cube 保留。
        ..default()
    };
    let camera = app
        .world_mut()
        .spawn((
            PrimarySceneView,                  // 正式玩法相机本来具有动态大气资格。
            AtmosphereSettings::default(), // 模拟上一帧动态天空已经安装原生大气。
            authored.clone(),              // 模拟三件套加载器刚刚原子采用主 Cube。
            AuthoredSkyboxRequest::new_with_mode(
                AuthoredSkyMode::Skybox,     // 显式选择天空盒可见背景。
                Some("sky.ktx2".to_owned()), // 主 Cube 与 IBL 的共同身份。
                authored.brightness,         // 可见作者亮度。
                300.0,                       // IBL 漫反射基准。
                1_000.0,                     // IBL 镜面基准。
                0.0,                         // 本测试不依赖请求旋转。
                3,                           // 当前作者修订。
            ),
            AuthoredSkyboxStatus {
                phase: AuthoredSkyboxPhase::Ready,       // 三件套已经完整采用。
                asset_path: Some("sky.ktx2".to_owned()), // 公开状态保留主身份。
                revision: 3,                             // 与请求修订一致。
                detail: String::new(),                   // 测试不依赖诊断文案。
                visible_skybox_active: true,             // 动态表现让权的真实采用证据。
            },
        ))
        .id(); // 保存相机读取互斥组件结果。

    app.update(); // 动态系统必须只移除大气资格，不能触碰作者 Cube。

    assert!(app.world().get::<AtmosphereSettings>(camera).is_none()); // 天空盒模式关闭 Bevy 原生大气。
    let retained = app.world().get::<Skybox>(camera).unwrap(); // 作者主 Cube 仍存在。
    assert_eq!(retained.brightness, authored.brightness); // 动态快照不得覆盖加载器刚写入的亮度。
    assert_eq!(retained.rotation, authored.rotation); // 动态快照不得覆盖加载器刚写入的方向。
    assert!(app.world().get::<DynamicSkyCameraRuntime>(camera).is_none()); // 首次即为天空盒时不错误冻结作者 Cube 为外部原状态。
}

#[test]
fn f12_reopen_preserves_skybox_adopted_after_dynamic_fallback() {
    // 同时覆盖空背景回退和旧大气快照：关闭编辑器不得恢复过期的可见天空。
    for had_original_atmosphere in [false, true] {
        let mut app = sky_app(ScenePhase::Active);
        app.insert_resource(WeatherAuthoringPreview::new(true));
        let camera = app
            .world_mut()
            .spawn((
                WeatherPreviewCamera,
                Camera::default(),
                Projection::Perspective(PerspectiveProjection::default()),
            ))
            .id();
        if had_original_atmosphere {
            app.world_mut()
                .entity_mut(camera)
                .insert(AtmosphereSettings::default());
        }
        app.update(); // 作者资源未就绪时先取得动态天空所有权。
        assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
        assert!(app.world().get::<DynamicSkyCameraRuntime>(camera).is_some());

        let mut images = Assets::<Image>::default();
        let authored = Skybox {
            image: Some(images.add(Image::default())), // 使用真实资产句柄，验证重新打开后资源身份不变。
            brightness: 450.0,
            rotation: Quat::from_rotation_y(0.73),
        };
        app.world_mut().entity_mut(camera).insert((
            authored.clone(),
            AuthoredSkyboxRequest::new_with_mode(
                AuthoredSkyMode::Skybox,
                Some("sky.ktx2".to_owned()),
                authored.brightness,
                380.0,
                600.0,
                0.73,
                7,
            ),
            AuthoredSkyboxStatus {
                phase: AuthoredSkyboxPhase::Ready,
                asset_path: Some("sky.ktx2".to_owned()),
                revision: 7,
                detail: String::new(),
                visible_skybox_active: true,
            },
        )); // 模拟异步天空盒加载器在动态回退之后采用完整资源。
        app.update();
        assert!(app.world().get::<AtmosphereSettings>(camera).is_none());

        // 反复关闭和打开同一离屏相机；期间不修改任何天空参数或重新提交加载请求。
        for active in [false, true, false, true] {
            app.insert_resource(WeatherAuthoringPreview::new(active));
            app.world_mut().get_mut::<Camera>(camera).unwrap().is_active = active;
            app.update();
            let retained = app.world().get::<Skybox>(camera).unwrap();
            assert_eq!(retained.image, authored.image);
            assert_eq!(retained.brightness, authored.brightness);
            assert_eq!(retained.rotation, authored.rotation);
            assert!(app.world().get::<AtmosphereSettings>(camera).is_none());
            assert!(app.world().get::<DynamicSkyCameraRuntime>(camera).is_none());
        }
    }
}

#[test]
fn f12_orthographic_view_never_receives_dynamic_atmosphere() {
    let mut app = sky_app(ScenePhase::Active); // F12 只在正式动态玩法场景预览。
    app.insert_resource(WeatherAuthoringPreview::new(true)); // 模拟 F12 已打开。
    let preview = app
        .world_mut()
        .spawn((
            WeatherPreviewCamera, // 明确作者天气相机身份。
            Projection::Orthographic(OrthographicProjection::default_3d()), // 顶、前或侧测绘投影。
        ))
        .id();
    app.update(); // 正交视图必须保持测绘背景，不接管动态大气。
    assert!(app.world().get::<AtmosphereSettings>(preview).is_none()); // 正交作者相机没有大气 LUT 成本。
    assert!(
        app.world()
            .get::<DynamicSkyCameraRuntime>(preview)
            .is_none()
    ); // 从未取得所有权。

    app.world_mut()
        .entity_mut(preview)
        .insert(Projection::Perspective(PerspectiveProjection::default())); // 切回 F12 透视。
    app.update(); // 同一正式系统立即建立动态天空。
    assert!(app.world().get::<AtmosphereSettings>(preview).is_some()); // 透视作者视图与玩法画面一致。

    app.world_mut()
        .entity_mut(preview)
        .insert(Projection::Orthographic(
            OrthographicProjection::default_3d(),
        )); // 再次切回测绘视图。
    app.update(); // 已接管相机必须恢复原状态。
    assert!(app.world().get::<AtmosphereSettings>(preview).is_none()); // 不保留正交大气残留。
    assert!(app.world().get::<Hdr>(preview).is_none()); // 仅由大气引入的 HDR 同步释放。
}

#[test]
fn f12_viewport_owned_hdr_survives_projection_and_preview_changes() {
    let mut app = sky_app(ScenePhase::Active);
    app.insert_resource(WeatherAuthoringPreview::new(true));
    let camera = app
        .world_mut()
        .spawn((
            WeatherPreviewCamera,
            Camera::default(),
            Projection::Perspective(PerspectiveProjection::default()),
            Hdr, // 工作台在创建时已明确拥有HDR，动态天空不能在正交或关闭预览时移除。
        ))
        .id();
    app.update();
    assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
    for _ in 0..3 {
        app.world_mut()
            .entity_mut(camera)
            .insert(Projection::Orthographic(OrthographicProjection::default_3d()));
        app.update();
        assert!(app.world().get::<Hdr>(camera).is_some());
        assert!(app.world().get::<AtmosphereSettings>(camera).is_none()); // 仍遵守正交测绘不显示动态大气的边界。
        app.world_mut()
            .entity_mut(camera)
            .insert(Projection::Perspective(PerspectiveProjection::default()));
        app.update();
        assert!(app.world().get::<Hdr>(camera).is_some());
        assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
    }
    for active in [false, true, false] {
        app.insert_resource(WeatherAuthoringPreview::new(active));
        app.world_mut()
            .get_mut::<Camera>(camera)
            .unwrap()
            .is_active = active;
        app.update();
        assert!(app.world().get::<Hdr>(camera).is_some());
        assert_eq!(
            app.world().get::<AtmosphereSettings>(camera).is_some(),
            active
        ); // 开关大气不再触发HDR/LDR颜色格式换代。
    }
}
