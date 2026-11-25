//! 显式真实窗口验收。相机看见的红色模型附在 GLB 动画骨骼上，绿色实例由场景摆放轨控制。
use super::{
    tests::{placement, scene, write_frame},
    *,
};
use bevy::{
    gltf::GltfAssetLabel,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::WindowResolution,
};
use feathrise_cinematic::Cinematic;
use feathrise_scene_content::ActiveDynamicScene;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn write_rig(dir: &Path) {
    // 两个真实 GLB 节点，一条从 0 到 1 秒向上平移的具名动作。
    let bin: Vec<_> = [0., 1., 0., 0., 0., 0., 1., 0.]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let mut json = serde_json::to_vec(&json!({
        "asset":{"version":"2.0"}, "scene":0, "scenes":[{"nodes":[0]}],
        "nodes":[{"name":"Rig","children":[1]},{"name":"AnimatedBone"}], "buffers":[{"byteLength":bin.len()}],
        "bufferViews":[{"buffer":0,"byteLength":8},{"buffer":0,"byteOffset":8,"byteLength":24}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR","min":[0],"max":[1]},
            {"bufferView":1,"componentType":5126,"count":2,"type":"VEC3"}],
        "animations":[{"name":"Raise","channels":[{"sampler":0,"target":{"node":1,"path":"translation"}}],
            "samplers":[{"input":0,"output":1,"interpolation":"LINEAR"}]}]
    })).unwrap();
    json.resize(json.len().next_multiple_of(4), b' ');
    let mut glb = b"glTF".to_vec();
    glb.extend(2u32.to_le_bytes());
    glb.extend(((28 + json.len() + bin.len()) as u32).to_le_bytes());
    for (tag, bytes) in [(b"JSON", json), (b"BIN\0", bin)] {
        glb.extend((bytes.len() as u32).to_le_bytes());
        glb.extend(tag);
        glb.extend(bytes);
    }
    fs::write(dir.join("rig.glb"), glb).unwrap();
}

#[derive(Resource)]
struct Proof {
    out: PathBuf,
    start: Instant,
    doc: Arc<Cinematic>,
    bone: Option<Entity>,
    pending: bool,
    captures: usize,
    finish: u8,
    centers: Vec<[Vec2; 2]>,
}

fn cube(world: &mut World, color: Color) -> (Mesh3d, MeshMaterial3d<StandardMaterial>) {
    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(0.7, 0.7, 0.25));
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: color,
            unlit: true,
            ..default()
        });
    (Mesh3d(mesh), MeshMaterial3d(material))
}

fn setup(world: &mut World) {
    world.spawn((
        Camera3d::default(),
        PrimarySceneView,
        Transform::from_xyz(0., 0., 9.),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        bevy::core_pipeline::tonemapping::DebandDither::Disabled,
    ));
    let handle = world
        .resource::<AssetServer>()
        .load(GltfAssetLabel::Scene(0).from_asset("rig.glb"));
    world.spawn((
        CinematicActor {
            target: "player".into(),
            instance: None,
            player: None,
            model: Some("rig.glb".into()),
        },
        Transform::from_xyz(-1., 0., 0.),
        WorldAssetRoot(handle),
    ));
    let green = cube(world, Color::srgb(0., 1., 0.));
    world.spawn((
        green,
        placement(&scene(), "gate"),
        Transform::from_xyz(1., 0., 0.),
    ));
}

fn check(world: &mut World) {
    world.resource_scope(|world, mut proof: Mut<Proof>| {
        assert!(
            proof.start.elapsed() < Duration::from_secs(45),
            "骨骼/渲染窗口超时，已拍 {} 帧，收尾阶段 {}",
            proof.captures,
            proof.finish
        );
        let Some(bone) = proof.bone else {
            let Some((bone, player)) = world
                .query::<(Entity, &Name, &bevy::animation::AnimatedBy)>()
                .iter(world)
                .find_map(|(e, n, p)| (n.as_str() == "AnimatedBone").then_some((e, p.0)))
            else {
                return;
            };
            let actor = world
                .query_filtered::<Entity, With<CinematicActor>>()
                .single(world)
                .unwrap();
            let mut bound = world.get::<CinematicActor>(actor).unwrap().clone();
            bound.player = Some(player);
            world.entity_mut(actor).insert(bound);
            let red = cube(world, Color::srgb(1., 0., 0.));
            world.spawn((red, Transform::IDENTITY, ChildOf(bone)));
            write_frame(world, &proof.doc, 37, FrameMode::Preview, 0.);
            proof.bone = Some(bone);
            return;
        };
        if proof.pending {
            return;
        }
        if proof.captures == 3 {
            match proof.finish {
                0 => {
                    world.resource_mut::<CinematicFrame>().active = None;
                }
                1 => {
                    assert_eq!(
                        world.get::<Transform>(bone).unwrap().translation,
                        Vec3::ZERO
                    );
                    let camera = world
                        .query_filtered::<&Transform, With<PrimarySceneView>>()
                        .single(world)
                        .unwrap();
                    assert_eq!(camera.translation, Vec3::Z * 9.);
                    write_frame(world, &proof.doc, 38, FrameMode::Game, 1.);
                }
                2 => {
                    assert_eq!(
                        world.resource::<CinematicPresentation>().error(38, 1.),
                        None
                    );
                    if !world.resource::<CinematicPresentation>().ready(38, 1.) {
                        return;
                    }
                    let frame = world
                        .resource_mut::<CinematicFrame>()
                        .active
                        .take()
                        .unwrap();
                    world
                        .resource_mut::<CinematicWorldState>()
                        .completed
                        .push(frame);
                }
                3 => {
                    use feathrise_scene_core::cinematic::{
                        CinematicRestingActor, ResumeCinematicActor,
                    };
                    let root = world
                        .query_filtered::<Entity, With<CinematicRestingActor>>()
                        .single(world)
                        .unwrap();
                    assert_eq!(
                        world.get::<Transform>(bone).unwrap().translation,
                        Vec3::Y,
                        "结算不得闪回原动作"
                    );
                    world.trigger(ResumeCinematicActor { entity: root });
                }
                _ => {}
            }
            proof.finish += 1;
            if proof.finish < 5 {
                return;
            }
            assert_eq!(
                world.get::<Transform>(bone).unwrap().translation,
                Vec3::ZERO,
                "归还角色原动画控制器"
            );
            for pair in proof.centers.windows(2) {
                assert!(
                    pair[0][0].y - pair[1][0].y > 15. && pair[1][1].y - pair[0][1].y > 8.,
                    "骨骼与摆放应沿相反方向移动：{pair:?}"
                );
            }
            println!(
                "真实剧情镜头、骨骼与摆放像素质心 {:?}，截图目录 {}",
                proof.centers,
                proof.out.display()
            );
            world.write_message(AppExit::Success);
            return;
        }
        let time = [0., 0.5, 1.][proof.captures];
        if !world
            .resource::<CinematicFrame>()
            .active
            .as_ref()
            .is_some_and(|f| f.time == time)
        {
            write_frame(world, &proof.doc, 37, FrameMode::Preview, time);
            return;
        }
        let receipt = world.resource::<CinematicPresentation>();
        assert_eq!(receipt.error(37, time), None);
        if !receipt.ready(37, time) {
            return;
        }
        assert!(
            (world.get::<Transform>(bone).unwrap().translation.y - time).abs() < 1e-5,
            "骨骼未真实求值"
        );
        let Projection::Perspective(projection) =
            world.get::<Projection>(receipt.camera.unwrap()).unwrap()
        else {
            panic!("应使用透视镜头")
        };
        assert!((projection.fov.to_degrees() - (50. - 10. * time)).abs() < 1e-4);
        proof.pending = true;
        world.spawn(Screenshot::primary_window()).observe(captured);
    });
}

fn captured(event: On<ScreenshotCaptured>, mut proof: ResMut<Proof>) {
    let image = event.image.clone().try_into_dynamic().unwrap().to_rgba8();
    let mut totals = [(Vec2::ZERO, 0u32); 2];
    for (x, y, p) in image.enumerate_pixels() {
        let kind = if p[0] > 150 && p[1] < 80 {
            0
        } else if p[1] > 150 && p[0] < 80 {
            1
        } else {
            continue;
        };
        totals[kind].0 += Vec2::new(x as f32, y as f32);
        totals[kind].1 += 1;
    }
    proof.pending = false;
    if totals.iter().any(|&(_, count)| count < 60) {
        return;
    } // 着色管线和模型必须已绘制。
    let centers = totals.map(|(sum, count)| sum / count as f32);
    image
        .save(proof.out.join(format!("frame-{}.png", proof.captures)))
        .unwrap();
    proof.centers.push(centers);
    proof.captures += 1;
}

#[test]
#[ignore = "需要真实图形设备和独立窗口"]
fn real_camera_bone_and_placement_follow_the_same_sample() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let out = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/tmp")
        .join(format!("cinematic-window-{stamp}"));
    fs::create_dir_all(out.parent().unwrap()).unwrap();
    fs::create_dir(&out).unwrap();
    write_rig(&out);
    let doc = Arc::new(
        Cinematic::from_slice(
            br#"{"version":1,"id":"shot","name":"shot","duration":1,
        "camera":[{"time":0,"pose":{"translation":[0,0,6]},"fov_y_degrees":50},
            {"time":1,"smooth":true,"pose":{"translation":[0.2,0,6]},"fov_y_degrees":40}],
        "actors":[{"target":"player","clips":[{"time":0,"duration":1,"name":"Raise"}]}],
        "objects":[{"target":"gate","keys":[{"time":0,"pose":{"translation":[1,0.5,0]}},
            {"time":1,"pose":{"translation":[1,-0.5,0]}}]}]}"#,
        )
        .unwrap(),
    );
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: out.to_string_lossy().into_owned(),
                watch_for_changes_override: Some(false),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "剧情演出 · 实机取样".into(),
                    resolution: WindowResolution::new(640, 400),
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
    .add_plugins(CinematicPresentationPlugin)
    .insert_resource(ClearColor(Color::srgb(0.01, 0.02, 0.04)))
    .insert_resource(ActiveDynamicScene {
        scene_id: Some(scene()),
    })
    .insert_resource(Proof {
        out,
        start: Instant::now(),
        doc,
        bone: None,
        pending: false,
        captures: 0,
        finish: 0,
        centers: vec![],
    })
    .add_systems(Startup, setup)
    .add_systems(Last, check);
    assert_eq!(app.run(), AppExit::Success);
}
