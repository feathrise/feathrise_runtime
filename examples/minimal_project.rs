//! SDK 最小工程：公共 run 装配运行时，工程的 Plugin 提供自己的几何、视图、文字与交互。
//! 运行 cargo run --example minimal_project；空格暂停旋转，Esc 退出。

#[path = "minimal_project/capture.rs"]
mod capture;

use bevy::{
    prelude::*,
    window::{PrimaryWindow, WindowMode},
};
use feathrise_runtime::{
    ProjectContext,
    project::{PrimarySceneView, SceneRuntimeObserver},
};
use std::path::{Path, PathBuf};

fn main() -> Result<(), String> {
    let capture = capture::parse_arguments()?;
    // Cargo example 共用 SDK 的清单，不伪造第二份游戏 Cargo.toml 或编辑器工程描述。
    // 此路径只属于随源码运行的示例；真实工程应在自己的入口调用 ProjectContext::load。
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/minimal_project")
        .canonicalize()
        .map_err(|error| format!("示例数据目录不可读：{error}"))?;
    let project = ProjectContext {
        asset_roots: vec![root.join("assets")],
        root,
        id: "feathrise-minimal-example".into(),
        name: "Feathrise 最小工程".into(),
    };
    feathrise_runtime::run(project, MinimalProjectPlugin { capture })
}

/// SDK 保持通用；工程的 Plugin 决定看到什么、如何交互。
struct MinimalProjectPlugin {
    capture: Option<PathBuf>,
}

#[derive(Component)]
struct RotatingCube;

impl Plugin for MinimalProjectPlugin {
    fn build(&self, app: &mut App) {
        // run 已注册 WindowPlugin；在 Winit 真正创建窗口前设置，避免先全屏再切回窗口。
        let world = app.world_mut();
        let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
        if let Ok(mut window) = windows.single_mut(world) {
            window.title = "Feathrise · Minimal project".into();
            window.mode = WindowMode::Windowed;
            window.resolution.set(960.0, 640.0);
        }
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
    // 没有已保存地图时，示例自行创建唯一视图；地图工程应初始化已有 PrimarySceneView。
    commands.spawn((
        Camera3d::default(),
        PrimarySceneView,
        IsDefaultUiCamera,
        Transform::from_xyz(5.0, 3.5, 6.0).looking_at(Vec3::Y * 0.7, Vec3::Y),
    ));
    commands.spawn((SceneRuntimeObserver, Transform::default()));

    // 不指定 TextFont.font，直接展示 run 自动安装的 SDK 默认中文字体。
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
