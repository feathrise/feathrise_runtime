//! 真实窗口验证开局传送文字；只使用本轮独占字体夹具与程序网格，不触碰玩家存档。

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use bevy::{
    core_pipeline::core_2d::Transparent2d,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        gpu_readback::{ReadbackComplete, ReadbackOnce},
        render_phase::ViewSortedRenderPhases,
        render_resource::TextureUsages,
        view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    },
    sprite_render::ExtractedSprites,
    window::{PresentMode, WindowResolution},
};
use feathrise_billboard_text::{
    BillboardCamera, BillboardDecoration, BillboardText, BillboardTextHandle, BillboardTextLink,
    BillboardTextOutline, BillboardTextPlugin,
};

use crate::{
    PortalLabel, PortalLabelFont, PortalPlugin, PortalSpawnSpec, PortalVisualMeshes,
    spawn_portal_entity,
};

#[derive(Resource, Clone, Default)]
struct DelayedDraws(Arc<AtomicU32>);

#[derive(Resource)]
struct Proof {
    output: PathBuf,
    frame: u32,
    requested: bool,
    captures: u32,
    failed: bool,
    stress: bool,
}

/// 模拟首次字形管线/图集尚未就绪：真实布局继续运行，首批2D绘制延后32帧再正常提交。
fn delay_first_bake_draws(
    mut phases: ResMut<ViewSortedRenderPhases<Transparent2d>>,
    delayed: Res<DelayedDraws>,
    sprites: Res<ExtractedSprites>,
    mut started: Local<bool>,
) {
    if phases.values().any(|phase| {
        phase.items.values().any(|item| {
            sprites
                .sprites
                .get(item.extracted_index)
                .is_some_and(|sprite| sprite.render_entity == item.entity.0)
        })
    }) {
        *started = true;
    }
    if *started && delayed.0.load(Ordering::Relaxed) < 32 {
        for phase in phases.values_mut() {
            // 只延迟字形。Mesh2d使用保留式排队，不能删除其已登记绘制项冒充字体延迟。
            phase.items.retain(|_, item| {
                sprites
                    .sprites
                    .get(item.extracted_index)
                    .is_none_or(|sprite| sprite.render_entity != item.entity.0)
            });
        }
        delayed.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn setup(
    mut commands: Commands,
    visual_meshes: Res<PortalVisualMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    font: Res<PortalLabelFont>,
    proof: Res<Proof>,
) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.04, 0.06, 0.09)),
            ..default()
        },
        Transform::from_xyz(0.0, 4.0, 14.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y),
        BillboardCamera,
    ));
    for (x, label) in [(-3.0, "前往东郊粮仓"), (3.0, "前往柳湾")] {
        spawn_portal_entity(
            &mut commands,
            &visual_meshes,
            &mut materials,
            PortalSpawnSpec {
                position: Vec3::new(x, 0.0, 0.0),
                target_scene: feathrise_scene_core::state::ScenePhase::Active,
                target_scene_id: Some("MAP-portal-startup-fixture"),
                target_display_name: label,
                spawn_position: Vec3::ZERO,
                label,
                color: Color::srgb(0.8, 0.65, 0.25),
                owner_scene: feathrise_scene_core::state::ScenePhase::Active,
                direction_degrees: 0.0,
            },
        );
    }
    if proof.stress {
        // 两个真实传送标签之外再排四项，覆盖固定四槽复用、描边、阴影和两种透明模式的装饰。
        for (x, label) in [
            (-5.0, "描边甲"),
            (-1.7, "描边乙"),
            (1.7, "描边丙"),
            (5.0, "描边丁"),
        ] {
            commands.spawn((
                Transform::from_xyz(x, 0.4, 0.0),
                PortalLabel {
                    source: label.into(),
                },
                BillboardText {
                    text: label.into(),
                    font: font.0.clone(),
                    font_size: 40.0,
                    texture_size: UVec2::new(256, 128),
                    crop_to_text: false,
                    y_offset: 0.0,
                    distance_scaling: false,
                    outline: Some(BillboardTextOutline {
                        color: Color::BLACK,
                        width_px: 2.0,
                    }),
                    decorations: vec![
                        BillboardDecoration {
                            radius_px: 55.0,
                            color: Color::srgba(0.0, 1.0, 0.0, 0.8),
                            z: 1.0,
                        },
                        BillboardDecoration {
                            radius_px: 40.0,
                            color: Color::srgb(0.0, 0.0, 1.0),
                            z: 2.0,
                        },
                    ],
                    ..default()
                },
            ));
        }
    }
}

fn capture(world: &mut World) {
    world.resource_scope(|world, mut proof: Mut<Proof>| {
        proof.frame += 1;
        assert!(
            world.resource::<Time<Real>>().elapsed_secs() < 40.0,
            "启动文字验收超时"
        );
        let labels: Vec<_> = world
            .query_filtered::<(&PortalLabel, &BillboardTextHandle), ()>()
            .iter(world)
            .map(|(label, handle)| (label.source.clone(), handle.0))
            .collect();
        let images: Vec<_> = labels
            .iter()
            .filter_map(|(label, entity)| {
                let link = world.get::<BillboardTextLink>(*entity)?;
                Some((label.clone(), link.image.clone(), link.is_ready()))
            })
            .collect();
        for (_, image, _) in &images {
            // 仅验收夹具允许读回纹理；生产烘焙不添加持续读回或像素扫描。
            let mut assets = world.resource_mut::<Assets<Image>>();
            let mut image = assets.get_mut(image).unwrap();
            if !image
                .texture_descriptor
                .usage
                .contains(TextureUsages::COPY_SRC)
            {
                image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
            }
        }
        if proof.requested
            || proof.frame < 120
            || images.len() != if proof.stress { 6 } else { 2 }
            || images.iter().any(|(_, _, ready)| !ready)
            || world.resource::<DelayedDraws>().0.load(Ordering::Relaxed) < 32
        {
            return;
        }
        proof.requested = true;
        for (index, (label, image, _)) in images.into_iter().enumerate() {
            let output = proof.output.join(format!("label-{index}.png"));
            world.spawn(ReadbackOnce::texture(image.clone())).observe(
                move |event: On<ReadbackComplete>,
                      assets: Res<Assets<Image>>,
                      mut proof: ResMut<Proof>| {
                    let pixels = &event.data;
                    let white = pixels
                        .chunks_exact(4)
                        .filter(|px| px[0] > 128 && px[1] > 128 && px[2] > 128 && px[3] > 128)
                        .count();
                    let mut captured = assets.get(&image).unwrap().clone();
                    captured.data = Some(pixels.clone());
                    captured.texture_descriptor.format =
                        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb;
                    captured.try_into_dynamic().unwrap().save(&output).unwrap();
                    eprintln!("启动传送标签 {label}：真实GPU白色字形像素={white}");
                    proof.failed |= white < 80;
                    if label.starts_with("描边") {
                        let blue = pixels
                            .chunks_exact(4)
                            .filter(|px| px[0] < 64 && px[1] < 64 && px[2] > 128 && px[3] > 128)
                            .count();
                        let green = pixels
                            .chunks_exact(4)
                            .filter(|px| px[0] < 64 && px[1] > 128 && px[2] < 64 && px[3] > 128)
                            .count();
                        eprintln!("启动装饰标签 {label}：不透明蓝盘={blue}，透明绿环={green}");
                        proof.failed |= blue < 80 || green < 80;
                    }
                    proof.captures += 1;
                },
            );
        }
        world
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(proof.output.join("startup-window.png")))
            .observe(|_: On<ScreenshotCaptured>, mut proof: ResMut<Proof>| proof.captures += 1);
    });
    if world.resource::<Proof>().captures
        == if world.resource::<Proof>().stress {
            7
        } else {
            3
        }
    {
        world.write_message(if world.resource::<Proof>().failed {
            AppExit::error()
        } else {
            AppExit::Success
        });
    }
}

#[test]
#[ignore = "需要真实GPU与独占窗口；用FEATHRISE_PORTAL_STARTUP_VALIDATION_DIR指定本轮字体和截图夹具"]
fn startup_portal_labels_survive_delayed_first_draw() {
    let output = PathBuf::from(
        std::env::var_os("FEATHRISE_PORTAL_STARTUP_VALIDATION_DIR").expect("指定本轮夹具"),
    )
    .canonicalize()
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(output.join("owner.txt"))
            .unwrap()
            .trim(),
        "portal-startup-labels-20261114-v1"
    );
    assert!(
        !output.join("startup-window.png").exists(),
        "不能覆盖已有实窗证据"
    );
    let delayed = DelayedDraws::default();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: output.join("assets").to_string_lossy().into_owned(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "传送点启动文字验收".into(),
                    resolution: WindowResolution::new(960, 640),
                    present_mode: PresentMode::AutoNoVsync,
                    focused: false,
                    ..default()
                }),
                ..default()
            })
            .set(bevy::winit::WinitPlugin {
                run_on_any_thread: true,
                ..default()
            })
            .disable::<bevy::audio::AudioPlugin>(),
    )
    .insert_resource(delayed.clone())
    .insert_resource(Proof {
        output,
        frame: 0,
        requested: false,
        captures: 0,
        failed: false,
        stress: std::env::var_os("FEATHRISE_PORTAL_STARTUP_STRESS").is_some(),
    })
    .add_plugins((BillboardTextPlugin, PortalPlugin))
    .add_systems(
        PreStartup,
        |mut commands: Commands, assets: Res<AssetServer>| {
            commands.insert_resource(PortalLabelFont(assets.load("fonts/NotoSansCJKsc-VF.otf")));
        },
    )
    .add_systems(PostStartup, setup)
    .add_systems(Last, capture);
    app.sub_app_mut(RenderApp)
        .insert_resource(delayed)
        .add_systems(
            Render,
            delay_first_bake_draws.in_set(RenderSystems::PhaseSort),
        );
    assert_eq!(app.run(), AppExit::Success);
}
