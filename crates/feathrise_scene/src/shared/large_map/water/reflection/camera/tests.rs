//! 正式SSR相机资格链的无窗口回归，不建立RenderDevice或运行游戏。

use super::*; // 测试运行正式维护系统。
use bevy::core_pipeline::prepass::DepthPrepass; // 检查作者附件没有被本功能直接移除。
use feathrise_map_content::MapId; // 同枚举动态地图换代。

fn app() -> App {
    // 不安装整个渲染或设置插件。
    let mut app = App::new(); // 最小ECS宿主。
    app.insert_resource(State::new(ScenePhase::Active)); // 用户新建地图走动态地图运行时。
    app.insert_resource(ActiveDynamicMap {
        map_id: Some(MapId("test-a".into())),
        ..default()
    }); // 第一份真实地图身份。
    app.init_resource::<RenderSettings>(); // 默认高特效。
    app.init_resource::<GraphicsRuntimeLeaseRegistry>(); // 只检查本功能交出的最小声明。
    app.init_resource::<WaterReflectionCameraState>(); // 初始没有租约。
    app.add_systems(PostUpdate, maintain); // 实际生产生命周期。
    app // 返回无窗口夹具。
} // 完成宿主。

fn camera(app: &mut App) -> Entity {
    // 测试显式安装PBR在正式插件中提供的传输附件。
    app.world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            Projection::Perspective(PerspectiveProjection::default()),
            ScreenSpaceTransmission::default(),
            PrimarySceneView,
        ))
        .id() // 合法玩法透视相机。
} // 完成相机夹具。

#[test] // 中高极高只取得一份HDR/深度要求，稳定帧保持原票据。
fn reflection_camera_quality_and_stable_lease_do_not_touch_authored_components() {
    // 使用生产选择和发布链。
    let mut app = app(); // 当前动态地图。
    let entity = camera(&mut app); // 唯一玩法相机。
    app.world_mut().entity_mut(entity).insert(DepthPrepass); // 作者自己持有深度。
    app.update(); // 高档启用。
    assert!(app.world().get::<WaterReflectionView>(entity).is_some()); // 取得唯一捕获资格。
    let lease = app.world().resource::<WaterReflectionCameraState>().lease; // 记录稳定票据。
    let requirements = app
        .world()
        .resource::<GraphicsRuntimeLeaseRegistry>()
        .requirements_for_camera(entity)
        .unwrap(); // 检查实际声明。
    let mut expected = GraphicsRuntimeRequirements::new(entity); // 其余功能保持作者状态。
    expected.require_depth_prepass = true; // 几何只允许本帧深度要求。
    expected.require_hdr = true; // F12与玩法都需要在线性HDR中混合SSR颜色。
    assert_eq!(requirements, expected); // 不强制MSAA/官方SSR/Solari。
    app.update(); // 暂停稳定帧仍运行系统。
    assert_eq!(
        app.world().resource::<WaterReflectionCameraState>().lease,
        lease
    ); // 没有重复释放/申请。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .effects_quality = EffectsQuality::Low; // 降档立即关闭。
    app.update(); // 撤销唯一资格。
    assert!(app.world().get::<WaterReflectionView>(entity).is_none()); // RenderApp不再捕获该视图。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    ); // 无租约泄漏。
    assert!(app.world().get::<DepthPrepass>(entity).is_some()); // 作者附件不被直接删除。
    for quality in [
        EffectsQuality::Medium,
        EffectsQuality::High,
        EffectsQuality::Ultra,
    ] {
        // 三个启用档同一契约。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .effects_quality = quality; // 切换合法档。
        app.update(); // 同源维护。
        assert_eq!(
            app.world()
                .resource::<GraphicsRuntimeLeaseRegistry>()
                .active_lease_count(),
            1
        ); // 始终唯一租约。
    } // 完成四档切换。
} // 完成最小所有权回归。

#[test] // F12唯一接管，正交和steps=0必须安全关闭。
fn reflection_camera_f12_perspective_and_transmission_switches_are_exclusive() {
    // 不改变背景玩法相机配置。
    let mut app = app(); // 动态地图夹具。
    let gameplay = camera(&mut app); // 保留后台玩法相机。
    let editor = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera::default(),
            Projection::Perspective(PerspectiveProjection::default()),
            ScreenSpaceTransmission::default(),
            WeatherPreviewCamera,
        ))
        .id(); // 明确F12作者相机。
    app.update(); // 默认天气锚点为玩法。
    assert!(app.world().get::<WaterReflectionView>(gameplay).is_some()); // 只有玩法捕获。
    assert!(app.world().get::<WaterReflectionView>(editor).is_none()); // 未打开作者入口不抢占。
    app.insert_resource(WeatherAuthoringPreview::new(true)); // F12正式接管。
    app.update(); // 同帧唯一交接。
    assert!(app.world().get::<WaterReflectionView>(gameplay).is_none()); // 后台不复制第二份颜色。
    assert!(app.world().get::<WaterReflectionView>(editor).is_some()); // 作者透视启用。
    app.world_mut()
        .entity_mut(editor)
        .insert(Projection::Orthographic(
            OrthographicProjection::default_3d(),
        )); // F12切正交。
    app.update(); // 正交回退环境反射。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    ); // 不偷偷回到后台玩法视图。
    app.world_mut()
        .entity_mut(editor)
        .insert(Projection::Perspective(PerspectiveProjection::default())); // 恢复作者透视。
    app.world_mut()
        .get_mut::<ScreenSpaceTransmission>(editor)
        .unwrap()
        .steps = 0; // 用户显式关闭屏幕传输。
    app.update(); // 尊重关闭。
    assert!(app.world().get::<WaterReflectionView>(editor).is_none()); // 不启用SSR捕获。
    assert_eq!(
        app.world()
            .get::<ScreenSpaceTransmission>(editor)
            .unwrap()
            .steps,
        0
    ); // 不强改用户值。
} // 完成F12和传输门。

#[test] // 切图、相机卸载和菜单都撤销自己票据，同时保留其他渲染租约。
fn reflection_camera_map_epoch_and_other_lease_are_preserved() {
    // 同枚举动态切图也不能沿用旧捕获。
    let mut app = app(); // 活动地图a。
    let entity = camera(&mut app); // 允许相机实体跨图复用。
    let other = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::solari_warmup(entity)); // 模拟RTX持有完整附件。
    app.update(); // SSR加入第二份最小租约。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        2
    ); // 所有者相互独立。
    app.world_mut().resource_mut::<ActiveDynamicMap>().map_id = Some(MapId("test-b".into())); // 同场景切换真实地图。
    app.update(); // 空一帧撤销旧纹理资格。
    assert!(app.world().get::<WaterReflectionView>(entity).is_none()); // 不复用旧图捕获。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    ); // RTX票据仍存在。
    app.update(); // 新地图稳定后可重新建立输入。
    assert!(app.world().get::<WaterReflectionView>(entity).is_some()); // 正式恢复。
    app.world_mut().despawn(entity); // 相机卸载可能先于下一次维护。
    app.update(); // try_remove处理已删除实体。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    ); // 只释放本功能票据。
    app.world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .release(other); // 模拟RTX自身生命周期回收。
    let next = camera(&mut app); // 下一台合法相机。
    app.update(); // 新目标获得唯一票据。
    app.insert_resource(State::new(ScenePhase::Idle)); // 退出玩法。
    app.update(); // 菜单释放。
    assert!(app.world().get::<WaterReflectionView>(next).is_none()); // 菜单无旧标记。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    ); // 全部正式回收。
} // 完成共享租约和场景生命周期。

#[test] // 重复主相机和任意普通RTT都不得多份复制。
fn reflection_camera_duplicate_or_inactive_views_fail_closed() {
    // 无任意first猜测。
    let mut app = app(); // 活动动态地图。
    let first = camera(&mut app); // 第一台主相机。
    let second = camera(&mut app); // 重建交接期间出现第二台主相机。
    app.update(); // 多个有效候选明确关闭。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    ); // 没有双份复制。
    app.world_mut().get_mut::<Camera>(second).unwrap().is_active = false; // 旧相机先停用。
    app.update(); // 唯一活动相机可采用。
    assert!(app.world().get::<WaterReflectionView>(first).is_some()); // 正确选择唯一活动源。
    app.world_mut().entity_mut(first).remove::<PrimarySceneView>(); // 变为普通辅助相机。
    app.update(); // 不因Camera3d存在就授权。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    ); // 普通RTT无资格。
} // 完成唯一性和显式资格。
