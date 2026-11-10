//! 活动门 lifecycle 无窗口回归，使用正式合同和系统。
use super::*; // 测试入口已经导入生产依赖。

#[test] // 暂停、等待、采用和模式退出均保留同一实体。
fn entity_waits_then_resumes_without_replacing_identity() {
    // 无窗口 ECS 回归覆盖实际 Commands 安装与恢复。
    let mut app = App::new(); // 不加载渲染或物理插件。
    app.insert_resource(TerrainCollisionRegion {
        enabled: true,
        revision: 1,
        max_distance_meters: 16.0,
        ..default()
    }); // 初始无地面。
    app.add_systems(Update, update_terrain_activity); // 测试正式活动门。
    let original = LockedAxes::ROTATION_LOCKED.lock_translation_x(); // 模拟生成方拥有的旋转和 X 约束。
    let entity = app
        .world_mut()
        .spawn((
            Transform::from_xyz(20.0, 3.0, 0.0),
            RigidBody::Dynamic,
            original,
            LinearVelocity(Vec3::Y),
            Name::new("保持身份的物品"),
        ))
        .id(); // 固定实体作为角色/掉落物身份代理。
    app.update(); // 进入最外范围之外。
    assert_eq!(
        app.world().get::<TerrainActivityPaused>(entity),
        Some(&TerrainActivityPaused::OutsideRange)
    ); // 远场原因不是短暂等待。
    assert_eq!(
        app.world().get::<LinearVelocity>(entity).unwrap().0,
        Vec3::ZERO
    ); // 没有继续下落或运动。
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation
        .x = 0.0; // 模拟玩家靠近后活动资格恢复。
    app.update(); // 仍未采用对应地面。
    assert_eq!(
        app.world().get::<TerrainActivityPaused>(entity),
        Some(&TerrainActivityPaused::AwaitingSupport)
    ); // 保留等待，不提前解冻。
    {
        // 原子发布完整落点区域。
        let mut region = app.world_mut().resource_mut::<TerrainCollisionRegion>(); // 正式发布者只修改共享资源。
        region.revision += 1; // 覆盖更新使缓存失效。
        region
            .supported_bounds
            .push((Vec2::splat(-4.0), Vec2::splat(4.0))); // 提供已安装碰撞覆盖。
    } // 释放发布资源借用。
    app.update(); // 地面完成才恢复。
    assert!(app.world().get::<TerrainActivityPaused>(entity).is_none()); // 公共暂停原因已移除。
    assert_eq!(
        app.world().get::<LockedAxes>(entity).unwrap().to_bits(),
        original.to_bits()
    ); // 原有轴锁保持。
    assert_eq!(
        app.world().get::<Name>(entity).unwrap().as_str(),
        "保持身份的物品"
    ); // 实体身份没有销毁重建。
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation
        .x = 20.0; // 再次进入远场。
    app.update(); // 再次冻结。
    app.world_mut()
        .resource_mut::<TerrainCollisionRegion>()
        .enabled = false; // 切回旧模式或退出场景。
    app.update(); // 即使位置和 revision 未改变也必须恢复。
    assert!(app.world().get::<TerrainActivityPaused>(entity).is_none()); // 禁用模式不留下暂停标记。
    assert_eq!(
        app.world().get::<LockedAxes>(entity).unwrap().to_bits(),
        original.to_bits()
    ); // 没有残留自己的平移位。
} // 结束生命周期回归。

#[test] // 无轴锁的掉落刚体也不能穿地，外部禁用不冒领。
fn missing_axes_and_external_disable_keep_ownership() {
    // 覆盖非角色的通用物理参与者。
    let mut app = App::new(); // 无窗口世界。
    app.insert_resource(TerrainCollisionRegion {
        enabled: true,
        revision: 1,
        max_distance_meters: 16.0,
        ..default()
    }); // 未发布地面。
    app.add_systems(Update, update_terrain_activity); // 正式系统。
    let item = app
        .world_mut()
        .spawn((
            Transform::default(),
            RigidBody::Dynamic,
            LinearVelocity(Vec3::NEG_Y),
        ))
        .id(); // 未显式配置 LockedAxes 的动态物品。
    app.world_mut().entity_mut(item).remove::<LockedAxes>(); // 不依赖 Avian 默认 Required 组件版本。
    let external = app
        .world_mut()
        .spawn((Transform::default(), RigidBody::Dynamic, RigidBodyDisabled))
        .id(); // 外部拥有暂停权。
    app.update(); // 只暂停实际活动且缺地面的物品。
    assert!(app.world().get::<TerrainActivityPaused>(item).is_some()); // 原本没有轴锁也进入等待。
    assert!(app.world().get::<TerrainActivityPaused>(external).is_none()); // 外部禁用没有被冒领。
    app.world_mut()
        .resource_mut::<TerrainCollisionRegion>()
        .enabled = false; // 退出模式。
    app.update(); // 归还独占锁。
    assert!(app.world().get::<LockedAxes>(item).is_none()); // 恢复原来的组件缺省状态。
    assert!(app.world().get::<RigidBodyDisabled>(external).is_some()); // 从未触碰外部禁用状态。
} // 结束缺省所有权测试。
