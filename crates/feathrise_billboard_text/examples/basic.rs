//! basic — feathrise_billboard_text 最小可运行示例。
//!
//! 演示三种用法：
//!   1. 固定世界坐标的常驻文字；
//!   2. 跟随一个上下浮动方块的头顶名牌；
//!   3. 带装饰底盘的复合徽标（描边圈 + 底盘 + 文字）。
//!
//! 运行：cargo run -p feathrise_billboard_text --example basic

use bevy::prelude::*;
use feathrise_billboard_text::{
    BillboardCamera, BillboardDecoration, BillboardText, BillboardTextPlugin,
};

/// 标记会上下浮动的方块，用于演示头顶名牌跟随。
#[derive(Component)]
struct Floater;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(BillboardTextPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, bob_floater)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // 主 3D 相机 —— 必须带 `BillboardCamera`，Billboard 才能朝向它。
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 4.0, 10.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        BillboardCamera,
    ));

    // 方向光，便于看清场景几何体。
    commands.spawn((
        DirectionalLight {
            illuminance: 8000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // 地面。
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(20.0, 20.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.25, 0.3, 0.25))),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // 用法 1：固定世界坐标的常驻文字。
    commands.spawn((
        Transform::from_xyz(-3.0, 2.0, 0.0),
        Visibility::default(),
        BillboardText {
            text: "Fixed Label".to_string(),
            follow: false, // 固定在此世界坐标
            ..default()
        },
    ));

    // 用法 2：上下浮动的方块 + 跟随其头顶的名牌。
    let floater = commands
        .spawn((
            Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
            MeshMaterial3d(materials.add(Color::srgb(0.4, 0.5, 0.9))),
            Transform::from_xyz(0.0, 1.0, 0.0),
            Floater,
        ))
        .id();
    commands.entity(floater).insert(BillboardText {
        text: "Follows Me".to_string(),
        follow: true, // 跟随宿主头顶
        y_offset: 1.2,
        color: Color::srgb(0.7, 0.9, 1.0),
        ..default()
    });

    // 用法 3：带装饰底盘的复合徽标（用独立锚点承载，固定在右侧）。
    commands.spawn((
        Transform::from_xyz(3.0, 2.0, 0.0),
        Visibility::default(),
        BillboardText {
            text: "!".to_string(),
            follow: false,
            font_size: 96.0,
            color: Color::srgb(0.12, 0.09, 0.05),
            texture_size: UVec2::splat(128),
            world_height: 0.6,
            crop_to_text: false, // 内容填满纹理，按方形映射
            decorations: vec![
                // 描边圈（最底层）
                BillboardDecoration {
                    radius_px: 60.0,
                    color: Color::srgb(0.1, 0.08, 0.05),
                    z: 0.0,
                },
                // 黄色底盘
                BillboardDecoration {
                    radius_px: 52.0,
                    color: Color::srgb(0.95, 0.82, 0.2),
                    z: 1.0,
                },
            ],
            ..default()
        },
    ));
}

/// 让方块上下浮动，验证头顶名牌实时跟随。
fn bob_floater(time: Res<Time>, mut q: Query<&mut Transform, With<Floater>>) {
    for mut t in &mut q {
        t.translation.y = 1.0 + (time.elapsed_secs() * 1.5).sin() * 0.5;
    }
}
