// 雨向捕获投影与真实主世界系统回归：不启动窗口、不分配GPU纹理，不依赖人工画面判断。
use super::{
    components::RainOcclusionCamera,
    constants, helpers,
    resources::{CaptureState, RainOcclusionMap},
    systems,
}; // 测试正式入口，不复制生命周期实现。
use crate::WeatherOutput; // 捕获与正式雨丝共用天气输出。
use bevy::prelude::*; // 无窗口ECS、投影与资产容器。
use bevy::time::TimeUpdateStrategy; // 手动固定真实时间，使暂停回归完全确定。
use feathrise_scene_core::view::PrimarySceneView; // 正式天气主相机标记。
use feathrise_scene_content::{ActiveDynamicScene, SceneId}; // 动态场景真实身份。
use feathrise_scene_core::state::ScenePhase; // 测试同枚举切换场景与离开玩法。
use std::time::Duration; // 确定性的帧时间控制。

fn fixture() -> (App, Entity) {
    // 只安装时间和主世界系统，避免任何GPU依赖。
    let mut app = App::new(); // 隔离每个测试的资源与实体。
    app.add_plugins(MinimalPlugins); // 使用正式 Time<Real>/Time<Virtual> 更新链。
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO)); // 默认不流逝真实时间，缓存重试由测试精确控制。
    app.init_resource::<Assets<Mesh>>(); // 提供真实源资产容器，不加载文件。
    app.init_resource::<Assets<StandardMaterial>>(); // 材质热更新消息与正式系统一致。
    app.init_resource::<Assets<Image>>(); // 只分配Image描述，不创建RenderApp。
    app.add_message::<AssetEvent<Mesh>>(); // 显式注册捕获系统消费的资产事件。
    app.add_message::<AssetEvent<StandardMaterial>>(); // 显式注册材质事件。
    app.init_resource::<RainOcclusionMap>(); // 默认无已捕获屋顶。
    app.init_resource::<CaptureState>(); // 固定大小捕获缓存。
    app.insert_resource(WeatherOutput {
        precipitation: 1.0,
        ..default()
    }); // 大雨触发唯一合法捕获需求。
    app.insert_resource(State::new(ScenePhase::Active)); // 验证同一个场景枚举中的场景身份。
    app.insert_resource(ActiveDynamicScene {
        scene_id: Some(SceneId("roof-test-a".into())),
    }); // 初始场景身份明确有效。
    app.add_systems(Startup, systems::initialize); // 正式初始化系统生成唯一禁用深度相机。
    app.add_systems(Update, systems::update); // 主世界捕获同步可脱离渲染执行。
    let anchor = app
        .world_mut()
        .spawn((
            // 真实捕获系统查询这些组件确定天气锚点。
            PrimarySceneView,      // 玩法相机而不是作者或辅助视图。
            Camera::default(), // 默认活动，排序由捕获系统使用。
            Projection::Perspective(PerspectiveProjection::default()), // 正式透视视图。
            GlobalTransform::IDENTITY, // 位于固定原点，稳定帧不改变投影。
        ))
        .id(); // 返回锚点用于删除与重建测试。
    (app, anchor) // 调用方拥有完整测试应用。
} // 所有资源都在测试结束后释放。

fn capture_is_active(app: &mut App) -> bool {
    // 读取正式相机是否提交本帧深度绘制。
    let world = app.world_mut(); // 单世界借用满足QueryState要求。
    let mut query = world.query_filtered::<&Camera, With<RainOcclusionCamera>>(); // 独立于玩法锚点查询。
    query.single(world).unwrap().is_active // 应只有一个长期保留的捕获相机。
} // 不将 map.active 和本帧重画混为一谈。

#[test] // F12关闭后源材质回到Deferred时，纯捕获相机也必须有可排队的Deferred phase。
fn capture_camera_supports_forward_and_deferred_sources_without_color_target() {
    // 验证正式初始化而非测试自己拼出的相机。
    use bevy::camera::RenderTarget; // 捕获视图不能因此变成普通颜色相机。
    use bevy::core_pipeline::prepass::{DeferredPrepass, DepthPrepass}; // 两种预通道共同承载世界源材质。
    let (mut app, _) = fixture(); // 使用真实initialize与update。
    app.update(); // 相机及required组件已经创建。
    let world = app.world_mut(); // 单次借用真实ECS世界。
    let mut query = world.query_filtered::<(
        Has<DepthPrepass>,
        Has<DeferredPrepass>,
        &RenderTarget,
        &Msaa,
    ), With<RainOcclusionCamera>>(); // 检查生产相机完整契约。
    let (depth, deferred, target, msaa) = query.single(world).unwrap(); // 捕获始终唯一。
    assert!(depth && deferred); // 普通和Deferred源都不会进入缺失的队列。
    assert!(matches!(target, RenderTarget::None { .. })); // 不执行额外主颜色绘制。
    assert_eq!(*msaa, Msaa::Off); // Deferred与数值深度保持单采样兼容。
} // 防止将来为省附件误删兼容phase而重新引入退出编辑器闪退。

#[test] // reverse-Z必须让雨向上游屋顶深度大于被遮住的下游雨丝。
fn vertical_and_slanted_rain_share_reverse_z_occlusion_geometry() {
    // 覆盖无风和明显斜风，而不是只测试矩阵可逆。
    let center = Vec3::new(20.0, 7.0, -45.0); // 非原点避免遗漏平移。
    for velocity in [Vec3::new(0.0, -30.0, 0.0), Vec3::new(12.0, -30.0, -8.0)] {
        // 同一算法处理两种实际雨向。
        let (view, matrix) = helpers::capture_view(center, velocity).unwrap(); // 使用正式捕获矩阵。
        let direction = velocity.normalize(); // 构造同一条降雨射线上的上下游点。
        let roof = matrix.project_point3(center - direction * 8.0); // 屋顶比采样点靠近降雨来源。
        let sheltered = matrix.project_point3(center + direction * 2.0); // 室内片元沿雨向处于屋顶下游。
        assert!(roof.z > sheltered.z && roof.z < 1.0 && sheltered.z > 0.0); // 保证比较方向与合法深度范围。
        assert!((roof.truncate() - sheltered.truncate()).length() < 0.0001); // 同一雨向射线必须落在同一深度像素。
        assert!(view.forward().dot(direction) > 0.9999); // 捕获相机必须沿实际风雨方向看。
        let far_outside = center + view.rotation * Vec3::X * constants::SPAN; // 从中心横向越过半窗口边界。
        let outside = matrix.project_point3(far_outside); // 消费者依据NDC窗口守卫排除。
        let uv = outside.truncate() * Vec2::new(0.5, -0.5) + Vec2::splat(0.5); // 与深度纹理的屏幕UV方向一致。
        assert!(uv.x > 1.0); // 窗外样本不能被clamp到边界屋顶而错误遮雨。
    } // 两种雨向都满足同一可见性约定。
} // 真实片元边界守卫另由Hanabi着色器测试覆盖。

#[test] // 损坏输入必须在发布矩阵前明确失效。
fn capture_rejects_nonfinite_and_zero_inputs() {
    // 避免NaN传播到相机与深度图。
    assert!(helpers::capture_view(Vec3::splat(f32::NAN), Vec3::NEG_Y).is_none()); // 损坏相机位置不能捕获。
    assert!(helpers::capture_view(Vec3::ZERO, Vec3::splat(f32::INFINITY)).is_none()); // 损坏雨速不能构造方向。
    assert!(helpers::capture_view(Vec3::ZERO, Vec3::ZERO).is_none()); // 零速度没有有效雨向。
    assert!(helpers::capture_view(Vec3::ZERO, Vec3::splat(f32::MAX)).is_none()); // 长度计算溢出也不能发布坏矩阵。
} // 不把失败当作有效单位矩阵。

#[test] // 大批祖先资格变更必须显式回退，不能发布局部完成的屋顶集合。
fn incomplete_caster_batch_disables_capture_until_atomic_restart() {
    // 使用实际主世界生产系统。
    let (mut app, _) = fixture(); // 正式雨向相机和需求资源。
    app.update(); // 建立第一份捕获身份。
    let previous = app.world().resource::<RainOcclusionMap>().image.clone(); // 记录旧目标。
    app.world_mut()
        .resource_mut::<CaptureState>()
        .casters_pending = true; // 模拟定向队列尚未处理完全部primitive。
    app.update(); // 不允许将部分资格集合提交GPU。
    assert!(!capture_is_active(&mut app)); // 中断当前捕获绘制。
    assert_eq!(
        app.world().resource::<RainOcclusionMap>().image,
        Handle::default()
    ); // 消费端显式无自动遮挡托底。
    app.world_mut()
        .resource_mut::<CaptureState>()
        .casters_pending = false; // 模拟最后一个源完成资格更新。
    app.update(); // 新完整集合重新生成目标。
    assert!(capture_is_active(&mut app)); // 恢复捕获。
    assert_ne!(app.world().resource::<RainOcclusionMap>().image, previous); // 禁止把旧图当成本轮部分结果。
} // 原子资格门回归结束。

#[test] // 锚点丢失、停雨与重新降雨必须归还默认图并创建新的完整捕获身份。
fn missing_anchor_and_dry_weather_invalidate_previous_capture() {
    // 验证实际系统而不是仅测试资源默认值。
    let (mut app, anchor) = fixture(); // 初始场景和相机全部有效。
    app.update(); // 初始化并发布第一次大雨捕获需求。
    let first = app.world().resource::<RainOcclusionMap>().image.clone(); // 记录不能跨失效期复用的目标身份。
    assert_ne!(first, Handle::<Image>::default()); // 应生成真实目标而不是默认图。
    assert!(app.world().resource::<RainOcclusionMap>().active); // 需求完整有效。
    app.world_mut().entity_mut(anchor).despawn(); // 模拟切换窗口或退出玩法时主相机先被删除。
    app.update(); // 当前帧必须停止使用旧屋顶。
    assert_eq!(
        app.world().resource::<RainOcclusionMap>().image,
        Handle::<Image>::default()
    ); // 消费者回到默认单像素图。
    assert!(!app.world().resource::<RainOcclusionMap>().active); // 不保留旧有效标志。
    assert!(!capture_is_active(&mut app)); // GPU捕获视图也立即停用。
    app.world_mut().spawn((
        // 恢复同场景的合法玩法锚点。
        PrimarySceneView,                                              // 天气所有者恢复。
        Camera::default(),                                         // 再次活动。
        Projection::Perspective(PerspectiveProjection::default()), // 透视资格恢复。
        GlobalTransform::IDENTITY,                                 // 与初始相机相同位置。
    )); // 恢复并不允许复用丢失前的像素。
    app.update(); // 新会话创建新目标。
    assert_ne!(app.world().resource::<RainOcclusionMap>().image, first); // 同位置同场景也拒绝前会话身份。
    app.world_mut()
        .resource_mut::<WeatherOutput>()
        .precipitation = 0.0; // 模拟正常天气转换到停雨。
    app.update(); // 停雨释放捕获需求。
    assert_eq!(
        app.world().resource::<RainOcclusionMap>().image,
        Handle::<Image>::default()
    ); // 不让下次降雨消费长时间陈旧屋顶。
} // 缺失输入期间显式使用安全无自动遮挡回退。

#[test] // DynamicSceneRoot枚举不变时场景ID仍必须使目标换代。
fn dynamic_scene_switch_replaces_target_before_new_capture() {
    // 防止上一场景屋顶投影污染新场景。
    let (mut app, _) = fixture(); // 建立第一个动态场景会话。
    app.update(); // 发布A场景需求。
    let first = app.world().resource::<RainOcclusionMap>().image.clone(); // 保留旧身份供比较。
    app.world_mut()
        .resource_mut::<ActiveDynamicScene>()
        .select(SceneId("roof-test-b".into())); // 同场景枚举选择另一场景。
    app.update(); // 新场景必须原子发布新的图像身份。
    let map = app.world().resource::<RainOcclusionMap>(); // 获取新完整捕获需求。
    assert!(map.active && map.image != first && map.image != Handle::default()); // 新场景有效且绝不沿用旧图。
    assert_eq!(
        app.world().resource::<CaptureState>().context,
        Some((
            ScenePhase::Active,
            Some(SceneId("roof-test-b".into()))
        ))
    ); // 缓存身份与发布目标一致。
    app.world_mut().resource_mut::<ActiveDynamicScene>().scene_id = None; // 场景尚未选择或正在撤销。
    app.update(); // 无身份不能保留B图。
    assert!(!app.world().resource::<RainOcclusionMap>().active); // 无完整上下文时撤销有效需求。
} // 无GPU执行也能验证跨场景身份边界。

#[test] // 缓存补捕获使用真实时间，F12暂停虚拟时间不会使迟到资源永久漏掉。
fn paused_virtual_time_still_allows_real_time_cache_refresh() {
    // 测试实际时间插件与捕获系统的组合。
    let (mut app, _) = fixture(); // 初始真实帧间隔为零。
    app.world_mut().resource_mut::<Time<Virtual>>().pause(); // 模拟作者编辑暂停玩法时间。
    for _ in 0..10 {
        app.update();
    } // 耗尽有限8帧启动重试，期间真实时间不前进。
    assert!(!capture_is_active(&mut app)); // 稳定缓存帧不持续绘制额外深度视图。
    let virtual_before = app.world().resource::<Time<Virtual>>().elapsed(); // 记录暂停时间。
    let real_before = app.world().resource::<Time<Real>>().elapsed(); // 记录稳定缓存的真实时刻。
    let image = app.world().resource::<RainOcclusionMap>().image.clone(); // 同会话刷新不需要换目标。
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        600,
    ))); // 超过正式0.5秒缓存刷新间隔。
    app.update(); // 正式Time插件只推进真实时间。
    assert_eq!(
        app.world().resource::<Time<Virtual>>().elapsed(),
        virtual_before
    ); // 虚拟时间确实停住。
    assert!(app.world().resource::<Time<Real>>().elapsed() > real_before); // 真实时间继续推进。
    assert!(capture_is_active(&mut app)); // 暂停期间仍重新捕获迟到纹理和屋顶。
    assert_eq!(app.world().resource::<RainOcclusionMap>().image, image); // 同会话刷新保持既有目标身份。
} // 不依赖睡眠、帧率或机器速度。
