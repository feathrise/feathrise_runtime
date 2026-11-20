//! 两个 SDK 示例共用工程数据、演示场景和交互，方便单独比较启动方式。

mod capture;

use bevy::prelude::*;
use feathrise_runtime::{
    ProjectContext,
    project::{PrimarySceneView, SceneRuntimeObserver},
};
use std::path::PathBuf;

pub(super) fn load() -> Result<(ProjectContext, MinimalProjectPlugin), String> {
    let capture = capture::parse_arguments()?;
    // 与普通工程使用同一个运行时加载入口，按当前目录和 Cargo 包名核对工程归属。
    let directory =
        std::env::current_dir().map_err(|error| format!("工程启动目录不可读：{error}"))?;
    let project = ProjectContext::load(&directory, env!("CARGO_PKG_NAME"))?;
    Ok((project, MinimalProjectPlugin { capture }))
}

/// SDK 保持通用；工程的 Plugin 决定看到什么、如何交互。
pub(super) struct MinimalProjectPlugin {
    capture: Option<PathBuf>,
}

#[derive(Component)]
struct RotatingCube;

impl Plugin for MinimalProjectPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, rotate_and_handle_input);
        if let Some(path) = &self.capture {
            capture::install(app, path.clone());
        }
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut clear_color: ResMut<ClearColor>,
) {
    clear_color.0 = Color::srgb(0.055, 0.075, 0.11);
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(8.0, 0.1, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.22, 0.26, 0.32))),
        Transform::from_xyz(0.0, -0.1, 0.0),
    ));
    commands.spawn((
        RotatingCube,
        Mesh3d(meshes.add(Cuboid::new(1.4, 1.4, 1.4))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.12, 0.65, 0.74),
            perceptual_roughness: 0.45,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.8, 0.0),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 9_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // 没有已保存场景时，示例自行创建唯一视图；场景工程应初始化已有 PrimarySceneView。
    commands.spawn((
        Camera3d::default(),
        PrimarySceneView,
        IsDefaultUiCamera,
        Transform::from_xyz(5.0, 3.5, 6.0).looking_at(Vec3::Y * 0.7, Vec3::Y),
    ));
    commands.spawn((SceneRuntimeObserver, Transform::default()));

    // 不指定 TextFont.font，直接展示运行时插件安装的 SDK 默认中文字体。
    // 关闭 default_font 时使用 Bevy 自带字体和 ASCII 文案；该字体不保证包含中点等标点。
    let message = if cfg!(feature = "default_font") {
        "羽立 · 最小工程\nSDK 运行时 + 自定义 Plugin\n空格：暂停 / 继续旋转    Esc：退出"
    } else {
        "Feathrise - Minimal project\nSDK runtime + your Plugin\nSpace: pause / resume    Esc: exit"
    };
    commands.spawn((
        Text::new(message),
        TextFont {
            font_size: FontSize::Px(25.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: px(24),
            top: px(24),
            ..default()
        },
    ));
    info!(
        default_font = cfg!(feature = "default_font"),
        "FEATHRISE_MINIMAL_READY"
    );
}

fn rotate_and_handle_input(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut paused: Local<bool>,
    mut cubes: Query<&mut Transform, With<RotatingCube>>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
    if keys.just_pressed(KeyCode::Space) {
        *paused = !*paused;
    }
    if !*paused {
        for mut transform in &mut cubes {
            transform.rotate_y(time.delta_secs() * 0.7);
        }
    }
}
