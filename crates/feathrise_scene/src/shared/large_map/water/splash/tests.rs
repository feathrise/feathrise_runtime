//! 真实ECS调度验证互斥查询、暂停寿命和场景身份清池，不创建窗口或GPU。
use super::super::material::DynamicWaterMaterial;
use super::{components::SplashDrop, resources::SplashWaterTarget, systems::*}; // 测试生产系统。
use bevy::prelude::*; // 实际App和系统调度。
use feathrise_scene_core::view::PrimarySceneView; // 正式主相机身份。
use crate::{SceneRuntimeObserver, WaterInteractionSource}; // 显式提供观察位置和脚底水面互动资格。
use feathrise_environment::terrain_surface::{
    SurfacePuddleMask, SurfaceWeatherGpu, SurfaceWeatherUpload,
}; // 同一完整天气契约。
use feathrise_scene_core::state::ScenePhase; // 加载状态明确禁止发射。 // 实际水材质资源。

fn app() -> App {
    // 仅提供SpatialQuery所需现有树，不启动物理或渲染线程。
    let mut app = App::new(); // 最小真实调度器。
    app.init_resource::<Time>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<DynamicWaterMaterial>>(); // 固定资源集合。
    app.init_resource::<avian3d::collider_tree::ColliderTrees>()
        .init_resource::<SurfacePuddleMask>()
        .init_resource::<SplashWaterTarget>(); // SpatialQuery与目标接口。
    app.insert_resource(SurfaceWeatherGpu {
        weather: Vec4::new(1.0, 1.0, 1.0, 0.0),
        wind_quality: Vec4::new(0.0, 0.0, 0.0, 2.0),
        ..default()
    }); // 高档大雨但无未授权落点。
    app.insert_resource(State::new(ScenePhase::Active)); // 正式玩法场景。
    app.world_mut().spawn((
        SceneRuntimeObserver,
        WaterInteractionSource::default(), // 此夹具脚底即原点；只有观察锚点不能授权脚下水体。
        Transform::default(),
        GlobalTransform::default(),
    )); // 真玩家排除水滴写侧。
    app.world_mut().spawn((
        PrimarySceneView,
        Transform::default(),
        GlobalTransform::default(),
    )); // 真相机排除水滴写侧。
    app.add_systems(Startup, initialize_rain_splashes); // 建立真正48槽共享池。
    app.add_systems(
        PostUpdate,
        (
            super::super::systems::update_dynamic_water_surfaces,
            update_rain_splashes,
        )
            .chain()
            .after(bevy::transform::TransformSystems::Propagate)
            .after(SurfaceWeatherUpload)
            .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
            .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
    ); // 与生产一致的依赖顺序。
    app.update(); // 初始化所有生产Query，B0001会直接令测试失败。
    app // 返回已建立时钟基线的App。
} // 完成无窗口场景。

fn activate_one(app: &mut App) -> Entity {
    // 测试不需要随机命中即可验证真实池生命周期。
    let world = app.world_mut(); // 独占世界只在测试中操作。
    let entity = world
        .query_filtered::<Entity, With<SplashDrop>>()
        .iter(world)
        .next()
        .unwrap(); // 固定池一定存在。
    let mut drop = world.get_mut::<SplashDrop>(entity).unwrap(); // 修改真实生产组件。
    drop.origin = Vec3::ZERO;
    drop.velocity = Vec3::Y * 1.25;
    drop.age = 0.0;
    drop.active = true; // 建立一粒真实弹道。
    entity // 返回槽身份。
} // 完成人工落点授权。

#[test]
fn splash_quality_limits_real_emission_and_paused_downgrade() {
    // 高极高实际发射预算及暂停降档裁剪。
    for (quality, interval, count, capacity) in [(2.0, 0.1, 2, 24), (3.0, 0.05, 4, 48)] {
        // 正式四档契约的两个几何档。
        let mut app = app();
        install_water(&mut app); // 安装可靠真实水域。
        {
            let mut weather = app.world_mut().resource_mut::<SurfaceWeatherGpu>();
            weather.wind_quality.w = quality;
            weather.weather.w = interval * 0.49;
        } // 不足一次间隔。
        app.update(); // 正式系统不能提前发射。
        assert_eq!(
            app.world_mut()
                .query::<&SplashDrop>()
                .iter(app.world())
                .filter(|drop| drop.active)
                .count(),
            0
        ); // 检查节流确实作用于发射链。
        app.world_mut()
            .resource_mut::<SurfaceWeatherGpu>()
            .weather
            .w = interval;
        app.update(); // 恰好到一批间隔。
        assert_eq!(
            app.world_mut()
                .query::<&SplashDrop>()
                .iter(app.world())
                .filter(|drop| drop.active)
                .count(),
            count
        ); // 大雨可靠水域每候选均接纳。
        for mut drop in app
            .world_mut()
            .query::<&mut SplashDrop>()
            .iter_mut(app.world_mut())
        {
            drop.active = true;
            drop.age = 0.0;
        } // 压满物理池以验证容量硬门。
        app.update(); // 时间不动仍须裁剪档位容量。
        assert_eq!(
            app.world_mut()
                .query::<&SplashDrop>()
                .iter(app.world())
                .filter(|drop| drop.active)
                .count(),
            capacity
        ); // 高24极高48。
        app.world_mut()
            .resource_mut::<SurfaceWeatherGpu>()
            .wind_quality
            .w = 2.0;
        app.update(); // 暂停中由极高降高。
        assert_eq!(
            app.world_mut()
                .query::<&SplashDrop>()
                .iter(app.world())
                .filter(|drop| drop.active)
                .count(),
            24
        ); // 不等时间恢复即可裁剪。
    } // 完成两档实际发射回归。
} // 完成质量生命周期回归。

#[test]
fn splash_wind_is_bounded_and_existing_drops_do_not_turn() {
    // 真实新滴速度与已发射固定弹道。
    let mut app = app();
    install_water(&mut app); // 可靠水面确保有实际发射。
    {
        let mut weather = app.world_mut().resource_mut::<SurfaceWeatherGpu>();
        weather.wind_quality = Vec4::new(f32::MAX, f32::MAX, f32::MAX, 3.0);
        weather.weather.w = 0.05;
    } // 极大有限风值仍必须有限。
    app.update(); // 执行真实风初速注入链。
    let values: Vec<_> = app
        .world_mut()
        .query::<(Entity, &SplashDrop)>()
        .iter(app.world())
        .filter(|(_, drop)| drop.active)
        .map(|(entity, drop)| (entity, drop.velocity))
        .collect(); // 保存实际发射速度。
    assert_eq!(values.len(), 4); // 满雨四候选真实发射。
    assert!(values.iter().all(|(_, velocity)| velocity.is_finite()
        && Vec2::new(velocity.x, velocity.z).length() <= 0.521
        && (1.25..=1.5).contains(&velocity.y))); // 风不改变峰高且水平速度有硬上界。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .wind_quality
        .x = -1.0;
    app.update(); // 暂停中反向风。
    assert!(values.iter().all(|(entity, velocity)| {
        app.world().get::<SplashDrop>(*entity).unwrap().velocity == *velocity
    })); // 已发射水滴不会突然转向。
    assert_eq!(
        super::helpers::splash_wind_velocity(Vec4::new(f32::NAN, 0.0, 1.0, 3.0)),
        Vec3::ZERO
    ); // 非有限调试输入安全退回无风。
} // 完成风弹道回归。

#[test]
fn splash_quality_uses_actual_camera_distance() {
    // 距離按真实相机而非玩家附近就视为近景。
    let mut app = app();
    install_water(&mut app); // 同一个玩家和可靠水面。
    for mut camera in app
        .world_mut()
        .query_filtered::<&mut GlobalTransform, With<PrimarySceneView>>()
        .iter_mut(app.world_mut())
    {
        *camera = GlobalTransform::from_translation(Vec3::Y * 15.0);
    } // 相机离水面15米。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .weather
        .w = 0.1;
    app.update(); // 高档12米外应拒绝所有候选。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .all(|drop| !drop.active)
    ); // 玩家脚下有水仍不能越过相机距离预算。
    {
        let mut weather = app.world_mut().resource_mut::<SurfaceWeatherGpu>();
        weather.wind_quality.w = 3.0;
        weather.weather.w = 0.2;
    } // 极高允许20米近景。
    app.update(); // 同一真实发射链改用极高距离。
    assert_eq!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .filter(|drop| drop.active)
            .count(),
        4
    ); // 同样候选现在有真实可见水花。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .wind_quality
        .w = 2.0;
    app.update(); // 暂停中降高。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .all(|drop| !drop.active)
    ); // 距离收缩立即清除旧滴。
} // 完成相机距离实际链回归。

#[test]
fn splash_schedule_is_valid_and_pause_freezes_real_pool() {
    // 初始化互斥查询并冻结真实组件。
    let mut app = app(); // 实际生产系统至少执行一次。
    assert_eq!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .count(),
        48
    ); // 实体预算固定。
    let entity = activate_one(&mut app); // 激活固定槽。
    app.update(); // 相同共享时间等于暂停。
    let drop = app.world().get::<SplashDrop>(entity).unwrap(); // 读取同一槽。
    assert!(drop.active);
    assert_eq!(drop.age, 0.0); // 暂停不老化不补发。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .weather
        .w = 0.5; // 模拟低帧长间隔。
    app.update(); // 完整消费0.5秒。
    assert!(!app.world().get::<SplashDrop>(entity).unwrap().active); // 旧水滴不得延长寿命。
} // 完成暂停长帧回归。

#[test]
fn splash_loading_clears_pool_and_wrap_preserves_delta() {
    // 菜单加载不能保留原图特效。
    let mut app = app(); // 正式系统完成初始化。
    let entity = activate_one(&mut app); // 建立原图活滴。
    app.insert_resource(State::new(ScenePhase::Loading)); // 同位置进入加载状态。
    app.update(); // 当前身份立即撤销。
    assert!(!app.world().get::<SplashDrop>(entity).unwrap().active); // 不等待寿命自然结束。
    assert!((super::helpers::elapsed_delta(Some(4095.98), 0.02) - 0.04).abs() < 0.001); // 自然回绕只推进真实短间隔。
} // 完成场景与循环边界。

#[test]
fn splash_real_water_candidates_emit_and_weather_gates_clear() {
    // 实际足迹、目标选择、材质采样和池发射全链回归。
    let mut app = app(); // 生产系统和固定池已经初始化。
    let water = install_water(&mut app); // 复用真实水体安装路径。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .weather
        .w = 0.1; // 推进一批真实视觉时间。
    app.update(); // 生产链从脚下水体选取到固定池发射全部执行。
    assert_eq!(app.world().resource::<SplashWaterTarget>().0, Some(water)); // 确认真正脚下水体选择成功。
    let drops: Vec<_> = app
        .world_mut()
        .query::<&SplashDrop>()
        .iter(app.world())
        .filter(|drop| drop.active)
        .map(|drop| drop.origin)
        .collect(); // 读取实际发射结果。
    assert!(!drops.is_empty() && drops.len() <= 4); // 发射链不是永久空操作且满足四候选硬预算。
    assert!(drops.iter().all(|point| point.is_finite()
        && point.x.abs() < 6.0
        && point.z.abs() < 6.0
        && point.y.abs() < 1.0)); // 动态起点保持真实水域附近且有限。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .weather
        .z = 0.0; // 雨停必须清除旧滴。
    app.update(); // 无需等待寿命即可清池。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .all(|drop| !drop.active)
    ); // 晴天不保留旧水花。
    {
        let mut weather = app.world_mut().resource_mut::<SurfaceWeatherGpu>();
        weather.weather.z = 1.0;
        weather.weather.w = 0.2;
    } // 恢复大雨并推进下一批。
    app.update(); // 再次通过实际候选链发射。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .any(|drop| drop.active)
    ); // 晴雨切换确实复用池。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .wind_quality
        .w = 1.0; // 降到中档关闭几何水花。
    app.update(); // 与生产质量门完全一致。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .all(|drop| !drop.active)
    ); // 中档不残留高档水滴。
} // 完成实际发射与天气画质门回归。

#[test] // 无Collider屋顶必须真正阻止发射，并在暂停新增时清除旧水滴。
fn explicit_shelter_blocks_actual_splashes_and_paused_existing_drops() {
    // 完整候选链消费正式共享资源。
    use feathrise_environment::rain_shelter::{
        RainShelterBox, RainShelterState, RainShelterVolume,
    }; // 与作者桥同源代理。
    let mut app = app();
    install_water(&mut app); // 开放水面先产生真实水花。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .weather
        .w = 0.1;
    app.update(); // 运行一批发射。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .any(|drop| drop.active)
    ); // 排除永久关闭的假阳性。
    let mut state = RainShelterState::default(); // 完整代理批次。
    let volume = RainShelterVolume::from_box(
        RainShelterBox {
            center: Vec3::ZERO,
            half_extents: Vec3::new(10.0, 0.2, 10.0),
        },
        &GlobalTransform::from_translation(Vec3::Y * 4.0),
    )
    .unwrap(); // 屋顶无碰撞、覆盖全部候选。
    state.gpu.header = Vec4::new(1.0, volume.half_extents.w, 0.0, 0.0);
    state.gpu.shelters[0] = volume; // 原子发布当前屋顶证据。
    app.insert_resource(state);
    app.update(); // 相同时间即暂停，新增遮挡仍立即生效。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .all(|drop| !drop.active)
    ); // 不留下屋檐内旧滴。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .weather
        .w = 0.2;
    app.update(); // 恢复时间重新尝试候选。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .all(|drop| !drop.active)
    ); // 真实发射读取无Collider遮雨证据。
    app.insert_resource(RainShelterState::default()); // 拆除屋顶回到开放水面。
    app.world_mut()
        .resource_mut::<SurfaceWeatherGpu>()
        .weather
        .w = 0.3;
    app.update(); // 下一批应正常发射。
    assert!(
        app.world_mut()
            .query::<&SplashDrop>()
            .iter(app.world())
            .any(|drop| drop.active)
    ); // 删除代理不永久停雨。
} // 完成动态遮雨实际水花链回归。

fn install_water(app: &mut App) -> Entity {
    // 各档位都走真实足迹、材质和目标选择链。
    use super::super::components::{
        DynamicWaterCpuInteraction, DynamicWaterSurface, WaterJumpCycle,
    }; // 正式互动水面组件。
    use super::super::footprint::{PaintedWaterFootprint, WaterInteractionFootprint};
    let material = app
        .world_mut()
        .resource_mut::<Assets<DynamicWaterMaterial>>()
        .add(super::super::material::create_dynamic_water_material(
            StandardMaterial::default(),
            0.0,
            Vec2::X,
            1.0,
            None,
        )); // 使用正式材质构造以包含四组实际宏观波。
    let surface = DynamicWaterSurface {
        // 真实足迹覆盖玩家周围全部候选区域。
        interaction_footprint: WaterInteractionFootprint::Painted(PaintedWaterFootprint::from_visible_mesh(
            [1, 1], [-20.0; 2], [20.0; 2], vec![1.0; 4],
        )), // 保留真实接触与水花授权验证，移除旧地图岸线格式。
        rest_water_level: 0.0,              // 水面位于玩家脚下附近。
        flow_direction: Vec2::X,            // 与材质一致的稳定流向。
        flow_speed: 1.0,                    // 与材质一致的基础流速。
        player_ripples: Vec::new(),         // 初始没有玩家事件，生产系统可正常生成入水波。
        last_player_takeoff_sequence: None, // 使用真实玩家必需组件建立基线。
        previous_player_local_xz: None,     // 首次接触不伪造移动距离。
        previous_player_local_y: None,      // 首次接触不伪造竖直速度。
        distance_since_ripple: 0.0,         // 初始累计距离为零。
        player_was_in_water: false,         // 由真正足迹判定玩家首次入水。
        jump_cycle: WaterJumpCycle::Idle,   // 初始没有等待落水的跳跃。
    }; // 完成真实水体输入。
    let water = app
        .world_mut()
        .spawn((
            surface,
            DynamicWaterCpuInteraction,
            MeshMaterial3d(material),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id(); // 安装真正进入既有遍历的水体。
    water // 返回真实水体身份。
} // 完成测试水域安装。
