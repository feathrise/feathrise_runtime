//! 活动门 recovery 无窗口回归，使用正式合同和系统。
use super::*; // 测试入口已经导入生产依赖。

#[test] // 传送必须在驱动 Update 之前发现旧地面无法支撑新位置。
fn teleport_center_updates_before_support_decision() {
    // 玩家中心与显示相机无关。
    let mut app = App::new(); // 无窗口 ECS。
    app.insert_resource(TerrainCollisionRegion {
        enabled: true,
        revision: 1,
        max_distance_meters: 16.0,
        supported_bounds: vec![(Vec2::splat(-4.0), Vec2::splat(4.0))],
        ..default()
    }); // 原地面仅在原点附近。
    app.add_systems(Update, update_terrain_activity); // 不安装完整场景驱动，独立验证实时中心。
    let player = app
        .world_mut()
        .spawn((
            Transform::from_xyz(100.0, 2.0, 0.0),
            RigidBody::Dynamic,
            LockedAxes::ROTATION_LOCKED,
            SceneRuntimeObserver,
        ))
        .id(); // 模拟已传送但后台候选未完成。
    app.update(); // 同次活动门同步中心并检查实际旧覆盖。
    assert_eq!(
        app.world().resource::<TerrainCollisionRegion>().center,
        Vec2::new(100.0, 0.0)
    ); // 不再用旧中心误判玩家范围外。
    assert_eq!(
        app.world().get::<TerrainActivityPaused>(player),
        Some(&TerrainActivityPaused::AwaitingSupport)
    ); // 新落点等待承托，未恢复运动。
} // 结束传送回归。

#[test] // 高速角色必须在下一步离开已有地面前冻结。
fn predicted_missing_ground_waits_until_new_coverage_is_adopted() {
    // 防止冻结自己清零速度导致下一帧误恢复。
    let mut app = App::new(); // 无窗口纯 ECS。
    app.insert_resource(TerrainCollisionRegion {
        enabled: true,
        revision: 1,
        max_distance_meters: 16.0,
        supported_bounds: vec![(Vec2::splat(-4.0), Vec2::splat(4.0))],
        ..default()
    }); // 当前中心有地面，但前方没有。
    app.add_systems(Update, update_terrain_activity); // 使用正式预测保护。
    let actor = app
        .world_mut()
        .spawn((
            Transform::default(),
            RigidBody::Dynamic,
            LinearVelocity(Vec3::X * 600.0),
        ))
        .id(); // 一步十米跨出已采用块。
    app.update(); // 在物理移动前冻结。
    assert_eq!(
        app.world().get::<TerrainActivityPaused>(actor),
        Some(&TerrainActivityPaused::AwaitingSupport)
    ); // 当前脚下有地面也不能冲进缺口。
    app.update(); // 清零后的下一次检查仍等待此前需要的前方地面。
    assert!(app.world().get::<TerrainActivityPaused>(actor).is_some()); // 不产生冻结/恢复振荡。
    {
        // 模拟后台完整采用前方覆盖。
        let mut region = app.world_mut().resource_mut::<TerrainCollisionRegion>(); // 唯一原子发布入口。
        region.supported_bounds[0] = (Vec2::splat(-12.0), Vec2::splat(12.0)); // 新块覆盖预测落点和足迹。
        region.revision += 1; // 采用身份变化使缓存复核。
    } // 完成发布。
    app.update(); // 新地面现在完整有效。
    assert!(app.world().get::<TerrainActivityPaused>(actor).is_none()); // 只在承托真正完成后恢复。
} // 结束高速前方承托回归。
