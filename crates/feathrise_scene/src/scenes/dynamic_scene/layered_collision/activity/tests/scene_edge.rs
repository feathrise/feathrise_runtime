//! 真实场景边界的活动门回归：分轴限速、历史等待恢复与缺页区别。
use super::*; // 使用正式ECS系统与真实承托合同，不复制被测逻辑。
use feathrise_scene_core::residency::TerrainSupportRows; // 已采用行携带真实场景边界。
use std::{collections::BTreeMap, sync::Arc}; // 小型确定性覆盖夹具。

fn edge_app() -> App {
    // 十米场景完整采用，同一版本不会因测试重复更新而改变。
    let mut app = App::new(); // 无窗口、无GPU。
    app.insert_resource(TerrainCollisionRegion {
        // 与正式发布格式一致。
        enabled: true,              // 仅启用Clipmap活动门。
        revision: 1,                // 固定版本验证缓存快路径。
        center: Vec2::splat(5.0),   // 全图都在活动范围内。
        max_distance_meters: 100.0, // 排除最外层活动资格干扰。
        supported_bounds: vec![(Vec2::ZERO, Vec2::splat(10.0))], // 已安装碰撞完整覆盖场景。
        supported_rows: Some(Arc::new(TerrainSupportRows {
            // 真实覆盖并非无限平面。
            origin: [0.0; 2],                        // 固定原点。
            spacing: [1.0; 2],                       // 一米覆盖格。
            bounds: [Vec2::ZERO, Vec2::splat(10.0)], // 权威场景边界。
            rows: (0..10)
                .map(|row| (row, [0, 10]))
                .collect::<BTreeMap<_, _>>(), // 十行完整物理格。
        })), // 不引入预取或额外地面。
        ..default()                 // 不使用旧凸多边形合同。
    }); // 完成已采用资源安装。
    app.add_systems(
        Update,
        (update_terrain_activity, freeze_terrain_activity).chain(),
    ); // 含最后冻结步骤，验证成功解除后不会再次归零。
    app // 返回可重复推进的真实世界。
} // 固定规模夹具。

#[test] // 四边与四角都不能因身体半径跨场景边缘而永久等待。
fn scene_edges_and_corners_allow_inward_and_tangent_movement() {
    // 逐位置检查向外限制与反向恢复。
    for position in [
        Vec3::new(0.0, 2.0, 5.0),
        Vec3::new(10.0, 2.0, 5.0),
        Vec3::new(5.0, 2.0, 0.0),
        Vec3::new(5.0, 2.0, 10.0),
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::new(0.0, 2.0, 10.0),
        Vec3::new(10.0, 2.0, 0.0),
        Vec3::new(10.0, 2.0, 10.0),
    ] {
        // 包含负方向和双轴角落。
        let mut app = edge_app(); // 每例独立所有权。
        let outward = Vec3::new((position.x - 5.0) * 1.2, -2.0, (position.z - 5.0) * 1.2); // 边中点切向为零，边法向为正负六米每秒。
        let actor = app
            .world_mut()
            .spawn((
                Transform::from_translation(position),
                RigidBody::Dynamic,
                LinearVelocity(outward),
            ))
            .id(); // 真实动态实体，不写暂停状态。
        for _ in 0..3 {
            // 同位置同版本反复输入，覆盖缓存快路径。
            app.world_mut().get_mut::<LinearVelocity>(actor).unwrap().0 = outward; // 模拟每物理步电机持续向外推。
            app.update(); // 执行真实活动门及冻结。
            assert!(app.world().get::<TerrainActivityPaused>(actor).is_none()); // 到场景边界不是等待后台。
            assert_eq!(
                app.world().get::<LinearVelocity>(actor).unwrap().0,
                Vec3::new(0.0, -2.0, 0.0)
            ); // 只去掉向外水平速度，重力保持。
        } // 连续外推结束。
        let inward = Vec3::new(-outward.x, 1.0, -outward.z); // 反向输入可以立即退回。
        app.world_mut().get_mut::<LinearVelocity>(actor).unwrap().0 = inward; // 不需要碰撞版本变化。
        app.update(); // 下一物理检查。
        assert_eq!(app.world().get::<LinearVelocity>(actor).unwrap().0, inward); // 不冻结向内或向上运动。
        assert!(app.world().get::<TerrainActivityPaused>(actor).is_none()); // 没有遗留全轴暂停。
    } // 四边四角完成。
    let mut app = edge_app(); // 单边斜向输入保留切向分量。
    let actor = app
        .world_mut()
        .spawn((
            Transform::from_xyz(10.0, 2.0, 5.0),
            RigidBody::Dynamic,
            LinearVelocity(Vec3::new(6.0, 3.0, 4.0)),
        ))
        .id(); // X向外、Z沿边。
    app.update(); // 正式分轴限制。
    assert_eq!(
        app.world().get::<LinearVelocity>(actor).unwrap().0,
        Vec3::new(0.0, 3.0, 4.0)
    ); // 不能用整条速度清零。
} // 结束连续输入回归。

#[test] // 边内高速一步跨出场景时必须先限制速度，而不是下一帧全锁等待。
fn predicted_scene_exit_stops_before_center_crosses_boundary() {
    // 身体靠边但中心仍在场景内。
    let mut app = edge_app(); // 完整地面无需构建。
    let actor = app
        .world_mut()
        .spawn((
            Transform::from_xyz(9.98, 2.0, 5.0),
            RigidBody::Dynamic,
            LinearVelocity(Vec3::new(60.0, -1.0, 2.0)),
        ))
        .id(); // 一步会跨出右边。
    app.update(); // 与真实物理前检查一致。
    assert!(app.world().get::<TerrainActivityPaused>(actor).is_none()); // 图外足迹被裁去，图内仍查真实覆盖。
    assert_eq!(
        app.world().get::<LinearVelocity>(actor).unwrap().0,
        Vec3::new(0.0, -1.0, 2.0)
    ); // 保留沿边移动。
    assert_eq!(
        app.world().get::<Transform>(actor).unwrap().translation.x,
        9.98
    ); // 不偷偷传送或吸附位置。
} // 结束一步预测回归。

#[test]
fn adopted_interior_allows_unbounded_underground_vertical_fall() {
    let mut app = edge_app();
    let actor = app.world_mut().spawn((Transform::from_xyz(5.0, -600.0, 5.0),
        RigidBody::Dynamic, LinearVelocity(Vec3::NEG_Y * 80.0))).id();
    for _ in 0..3 {
        app.update(); // 已采用XZ覆盖不代表玩家脚下有地板；不能因低于旧高度域锁住重力。
        assert!(app.world().get::<TerrainActivityPaused>(actor).is_none());
        assert!(app.world().get::<TerrainActivityLock>(actor).is_none());
        assert_eq!(app.world().get::<LinearVelocity>(actor).unwrap().0, Vec3::NEG_Y * 80.0);
    }
}

#[test] // 已存在的旧边缘等待无需重新加载场景即可归还本域锁。
fn old_outside_prediction_recovers_without_losing_external_axes() {
    // 重现截图中的零速度、外向缓存。
    for predicted in [Vec3::new(11.0, 2.0, 11.0), Vec3::new(10.0, 2.0, 10.0)] {
        // 同时覆盖图外预测和原地足迹跨边导致的旧缓存。
        let mut app = edge_app(); // 同版本完整场景。
        let position = Vec3::new(10.0, 2.0, 10.0); // 实体在真实角落。
        let original = LockedAxes::ROTATION_LOCKED.lock_translation_x(); // 外部已有X锁与旋转锁。
        let actor = app
            .world_mut()
            .spawn((
                Transform::from_translation(position),
                RigidBody::Dynamic,
                LinearVelocity(Vec3::ZERO),
                lock_translation(original),
                TerrainActivityPaused::AwaitingSupport,
                TerrainActivityLock {
                    previous_axes: original,
                    inserted_axes: false,
                },
                TerrainSupportCache {
                    source: None, frame: default(),
                    block: Some(0),
                    revision: 1,
                    position,
                    predicted,
                    footprint: Vec2::splat(0.05),
                },
                Name::new("保留原身份"),
            ))
            .id(); // 完整复现历史缓存，不修改资源版本。
        app.update(); // 裁掉图外预测后正式复查角落内的已采用地面。
        assert!(app.world().get::<TerrainActivityPaused>(actor).is_none()); // 不再无限等待。
        assert!(app.world().get::<TerrainActivityLock>(actor).is_none()); // 本域所有权已归还。
        assert_eq!(
            app.world().get::<LockedAxes>(actor).unwrap().to_bits(),
            original.to_bits()
        ); // 外部平移与旋转约束不丢失。
        assert_eq!(
            app.world().get::<Name>(actor).unwrap().as_str(),
            "保留原身份"
        ); // 没有销毁重生。
        app.world_mut().get_mut::<LinearVelocity>(actor).unwrap().0 = Vec3::NEG_Z; // 玩家随后向场景内退回。
        app.update(); // 不需发布新版本。
        assert_eq!(
            app.world().get::<LinearVelocity>(actor).unwrap().0,
            Vec3::NEG_Z
        ); // 退回输入不会再被等待门吞掉。
    } // 两类遗留缓存均可恢复。
} // 结束历史等待回归。

#[test] // 只裁真正场景外，不能把图内没有加载的格子假装成地面。
fn scene_edge_fix_does_not_relax_missing_interior_or_outside_centers() {
    // 场景右半部暂缺碰撞。
    let mut app = edge_app(); // 先使用完整夹具。
    {
        // 修改为场景内缺覆盖，保留场景真实边界十米。
        let mut region = app.world_mut().resource_mut::<TerrainCollisionRegion>(); // 模拟完整但较小的已采用窗口。
        Arc::make_mut(region.supported_rows.as_mut().unwrap())
            .rows
            .values_mut()
            .for_each(|range| range[1] = 5); // X大于5属于场景内未覆盖。
        region.supported_bounds[0].1.x = 5.0; // 实际碰撞块一致收缩。
    } // 完成夹具配置。
    let actor = app
        .world_mut()
        .spawn((
            Transform::from_xyz(4.0, 2.0, 5.0),
            RigidBody::Dynamic,
            LinearVelocity(Vec3::X * 120.0),
        ))
        .id(); // 下一步到场景内6米缺页处。
    app.update(); // 仍按原承托保护冻结。
    assert_eq!(
        app.world().get::<TerrainActivityPaused>(actor),
        Some(&TerrainActivityPaused::AwaitingSupport)
    ); // 不允许穿过未采用覆盖。
    app.update(); // 自己归零不能导致假恢复。
    assert!(app.world().get::<TerrainActivityPaused>(actor).is_some()); // 仍等待图内真实地面。
    let outside = app
        .world_mut()
        .spawn((
            Transform::from_xyz(10.1, 2.0, 5.0),
            RigidBody::Dynamic,
            LinearVelocity(Vec3::NEG_X),
        ))
        .id(); // 非本修复造成的图外中心。
    app.update(); // 不伪造承托或自动传送。
    assert!(app.world().get::<TerrainActivityPaused>(outside).is_some()); // 图外中心仍无实际地面。
    assert_eq!(
        app.world().get::<Transform>(outside).unwrap().translation.x,
        10.1
    ); // 原位置未被偷偷改写。
} // 结束安全边界回归。

#[test]
fn adjoining_subscene_terrain_allows_crossing_both_ways_but_not_missing_support() {
    use avian3d::prelude::{Collider, PhysicsPlugins};
    use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce};
    use feathrise_scene_content::{SceneId, SceneInstanceId, SceneInstancePath};
    use feathrise_scene_core::residency::{InstanceResourceTable, SceneInstanceOwner};
    use crate::scenes::dynamic_scene::instances::{SubsceneInstance, SubsceneActive};
    let mut app = edge_app();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin,
        bevy::mesh::MeshPlugin, PhysicsPlugins::default()));
    app.finish();
    app.world_mut().spawn((RigidBody::Static, Collider::cuboid(10.0, 1.0, 10.0), Transform::from_xyz(5.0, -0.5, 5.0)));
    let batch = app.world_mut().spawn((RigidBody::Static, Transform::IDENTITY, SubsceneActive)).id();
    let source = app.world_mut().spawn((Transform::from_xyz(10.0, 0.0, 0.0), ChildOf(batch))).id();
    app.world_mut().entity_mut(source).insert(SubsceneInstance {
        path: SceneInstancePath::root(SceneId::parse("MAP-parent").unwrap()).child(SceneInstanceId::parse("child").unwrap()),
        source_scene_id: SceneId::parse("MAP-child").unwrap(), generation: 1, batch });
    app.world_mut().spawn((Collider::cuboid(10.0, 1.0, 10.0), Transform::from_xyz(5.0, -0.5, 5.0),
        ChildOf(source), SceneInstanceOwner(source)));
    let mut regions = InstanceResourceTable::<TerrainCollisionRegion>::default();
    regions.values.insert(source, app.world().resource::<TerrainCollisionRegion>().clone());
    app.insert_resource(regions);
    for _ in 0..5 { app.update(); }
    let actor = app.world_mut().spawn((Transform::from_xyz(9.98, 2.0, 5.0), RigidBody::Dynamic,
        LinearVelocity(Vec3::X * 60.0))).id();
    app.world_mut().run_system_once(update_terrain_activity).unwrap();
    assert_eq!(app.world().get::<LinearVelocity>(actor).unwrap().0, Vec3::X * 60.0);
    assert!(app.world().get::<TerrainActivityPaused>(actor).is_none());
    app.world_mut().get_mut::<Transform>(actor).unwrap().translation.x = 10.98;
    app.world_mut().get_mut::<LinearVelocity>(actor).unwrap().0 = Vec3::NEG_X * 60.0;
    app.world_mut().run_system_once(update_terrain_activity).unwrap();
    assert_eq!(app.world().get::<LinearVelocity>(actor).unwrap().0, Vec3::NEG_X * 60.0);
    app.world_mut().get_mut::<Transform>(actor).unwrap().translation.x = 9.98;
    app.world_mut().get_mut::<LinearVelocity>(actor).unwrap().0 = Vec3::X * 60.0;
    app.world_mut().resource_mut::<InstanceResourceTable<TerrainCollisionRegion>>()
        .values.get_mut(&source).unwrap().supported_bounds.clear();
    app.world_mut().run_system_once(update_terrain_activity).unwrap();
    assert_eq!(app.world().get::<LinearVelocity>(actor).unwrap().0, Vec3::ZERO); // 实体存在不等于该源图碰撞已采用。
}
