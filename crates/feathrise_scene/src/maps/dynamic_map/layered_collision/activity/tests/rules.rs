//! 活动门 rules 无窗口回归，使用正式合同和系统。
use super::*; // 测试入口已经导入生产依赖。

#[test] // 命中旧 AABB 缓存时仍检查真实圆边界及足迹。
fn clipped_polygon_rejects_cached_block_corner_and_footprint() {
    // 无需真实物理插件即可检查恢复门。
    let region = TerrainCollisionRegion {
        // 同一已采用版本的块与凸外环。
        enabled: true,            // 使用共享碰撞活动规则。
        max_distance_meters: 4.0, // 外环之外仍可能具有活动资格。
        supported_bounds: vec![(Vec2::splat(-2.0), Vec2::splat(2.0))], // 缓存命中不能证明裁边后仍有地面。
        supported_polygon: vec![
            Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 2.0),
            Vec2::new(-2.0, 0.0),
            Vec2::new(0.0, -2.0),
        ], // 真实凸外环。
        ..default()                                                    // 纯规则无需额外身份。
    }; // 数据完整。
    assert_eq!(
        activity_reason(&region, Vec3::new(1.5, 0.0, 1.5), Some(0)),
        (Some(TerrainActivityPaused::AwaitingSupport), None)
    ); // 活动圆内、实际弦外，即使缓存命中也等待。
    assert_eq!(
        activity_reason(&region, Vec3::ZERO, Some(0)),
        (None, Some(0))
    ); // 中心正常复用缓存。
    assert!(footprint_supported(
        &region,
        Vec3::ZERO,
        Vec2::splat(0.5),
        Some(0)
    )); // 整个足迹位于外环内部。
    assert!(!footprint_supported(
        &region,
        Vec3::new(0.9, 0.0, 0.9),
        Vec2::splat(0.2),
        Some(0)
    )); // 中心在地面上但足迹越过弦边，不提前恢复。
} // 结束裁圆缓存保护回归。

#[test] // 区分范围外和等待实际地面。
fn activity_reasons_and_cache_revalidation() {
    // 缓存块重排不能误恢复。
    let mut region = TerrainCollisionRegion {
        enabled: true,
        max_distance_meters: 16.0,
        ..default()
    }; // 有活动范围但未采用块。
    assert_eq!(
        activity_reason(&region, Vec3::ZERO, None).0,
        Some(TerrainActivityPaused::AwaitingSupport)
    ); // 首次进入等待。
    assert_eq!(
        activity_reason(&region, Vec3::X * 17.0, None).0,
        Some(TerrainActivityPaused::OutsideRange)
    ); // 范围外永久暂停直到接近。
    region
        .supported_bounds
        .push((Vec2::splat(-4.0), Vec2::splat(4.0))); // 发布完整落点块。
    assert_eq!(activity_reason(&region, Vec3::ZERO, None), (None, Some(0))); // 地面完成才恢复。
    region.supported_bounds[0] = (Vec2::splat(8.0), Vec2::splat(12.0)); // 模拟块数组重排。
    assert_eq!(
        activity_reason(&region, Vec3::ZERO, Some(0)).0,
        Some(TerrainActivityPaused::AwaitingSupport)
    ); // 不信任旧下标。
} // 结束状态测试。

#[test] // 外部约束不能被恢复逻辑清掉。
fn restore_preserves_external_rotation_and_previous_translation() {
    // 与旧承托所有权口径一致。
    let previous = LockedAxes::ROTATION_LOCKED.lock_translation_x(); // 外部原有旋转与 X 锁。
    assert_eq!(
        restore_translation(lock_translation(previous), previous).to_bits(),
        previous.to_bits()
    ); // 恢复只撤销追加的 Y/Z 位。
} // 结束锁测试。

#[test] // 中心有支持不代表下一物理步与角色边缘都有支持。
fn footprint_and_predicted_motion_require_complete_ground() {
    // 保守边缘保护不修改碰撞几何。
    let region = TerrainCollisionRegion {
        enabled: true,
        max_distance_meters: 16.0,
        supported_bounds: vec![(Vec2::splat(-4.0), Vec2::splat(4.0))],
        ..default()
    }; // 单个已采用块。
    assert!(footprint_supported(
        &region,
        Vec3::ZERO,
        Vec2::splat(0.5),
        Some(0)
    )); // 足迹全部位于块内。
    assert!(!footprint_supported(
        &region,
        Vec3::X * 3.8,
        Vec2::splat(0.5),
        Some(0)
    )); // 角色边缘越过未就绪边界不能恢复。
} // 结束足迹回归。
