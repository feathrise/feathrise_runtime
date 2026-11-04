//! 验证相机在Update旋转后，文字及引导在同帧变换传播前完成同步。
use crate::*;
use bevy::prelude::*; // 最小ECS和正式变换传播插件。 // 使用生产组件、材质与调度入口。

#[test] // 左右转动连续多帧，不能留下一帧朝向差。
fn billboard_pose_follows_current_update_camera_before_propagation() {
    // 不依赖GPU或调度碰巧按注册顺序运行。
    let mut app = App::new(); // 最小主世界。
    app.add_plugins(TransformPlugin)
        .init_resource::<Time>() // 使用实际GlobalTransform传播。
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<Mesh>>() // 文字生成需要的资产容器。
        .init_resource::<Assets<BillboardColorMaterial>>() // 可见文字材质。
        .init_resource::<Assets<BillboardTemporalGuideMaterial>>() // 子实体引导材质。
        .init_resource::<Assets<ColorMaterial>>()
        .init_resource::<BillboardBakePool>(); // 离屏请求容器，不运行烘焙。
    app.add_systems(Update, spawn_billboard_texts); // 创建真实文字及其引导子实体。
    super::configure_billboard_pose_update(&mut app); // 测试生产使用的排序，避免只测试手写测试排序。
    let camera = app
        .world_mut()
        .spawn((BillboardCamera, Transform::from_xyz(0.0, 0.0, 5.0)))
        .id(); // 主相机。
    let owner = app
        .world_mut()
        .spawn((Transform::default(), BillboardText::default()))
        .id(); // 跟随宿主。
    app.update(); // 完成实体创建。
    let billboard = app.world().get::<BillboardTextHandle>(owner).unwrap().0; // 实际文字实体。
    let guide = app
        .world()
        .get::<BillboardTextLink>(billboard)
        .unwrap()
        .temporal_guide_entity; // 实际引导子实体。
    app.world_mut()
        .get_mut::<BillboardTextLink>(billboard)
        .unwrap()
        .bake_ready = true; // 跳过无关的GPU烘焙。
    app.add_systems(Update, move_camera); // 在Update改变朝向，PostUpdate必须读取这一帧的新值。
    for _ in 0..8 {
        // 重复转动避免仅验证初始默认姿态。
        app.update(); // 执行完整Update和PostUpdate。
        let expected = app.world().get::<Transform>(camera).unwrap().rotation; // 当前相机姿态。
        for entity in [billboard, guide] {
            // 文字和引导都必须同步，不能只检查局部Transform。
            let actual = app
                .world()
                .get::<GlobalTransform>(entity)
                .unwrap()
                .compute_transform()
                .rotation; // 渲染抽取的最终姿态。
            assert!(actual.abs_diff_eq(expected, 1e-5)); // 同帧误差只允许浮点传播误差。
        } // 两个网格共享当前相机朝向。
    } // 连续帧验证结束。
} // 回归测试结束。

fn move_camera(mut cameras: Query<&mut Transform, With<BillboardCamera>>) {
    // 模拟玩法Update相机旋转。
    for mut camera in &mut cameras {
        camera.rotate_y(0.17);
    } // 每帧改变朝向，使上一帧姿态不能通过断言。
} // 相机运动结束。
