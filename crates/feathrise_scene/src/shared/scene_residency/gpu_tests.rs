//! 独立真实GPU窗口验证三张完整场景：Entity、顶点缓冲和纹理保留，后台视图暂停。

use super::*;
use bevy::{
    diagnostic::FrameCount,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        mesh::allocator::MeshAllocator,
        render_asset::RenderAssets,
        sync_world::RenderEntity,
        texture::GpuImage,
        view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    },
    winit::WinitPlugin,
};
use std::sync::Mutex;

#[derive(Default, Debug)]
struct GpuEvidence {
    render_entity: Option<Entity>,
    mesh: Option<bevy::asset::AssetId<Mesh>>,
    image: Option<bevy::asset::AssetId<Image>>,
    allocation: Option<(String, String)>,
    paused_seen: bool,
    resumed_seen: bool,
    captured: bool,
    mesh_seen: bool,
    image_seen: bool,
    render_frames: u32,
    peer_alive: bool,
    closed_all: bool,
    remaining_scene_views: usize,
    remaining_scene_lights: usize,
}
#[derive(Resource, Clone, ExtractResource, Default)]
struct GpuProbe(Arc<Mutex<GpuEvidence>>);
#[derive(Resource, Default)]
struct Smoke {
    phase: u8,
    since: u32,
    root: Option<Entity>,
    paused_tick: u64,
    pixels: Arc<Vec<u8>>,
}

fn build_gpu_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut counts: ResMut<ConstructionCounts>,
    mut proof: ResMut<MapRuntimeProof>,
) {
    counts.entered += 1;
    proof.serial = counts.entered;
    proof.pixels = Arc::new(vec![counts.entered as u8; 16_384]);
    proof.image = images.add(Image::new_target_texture(
        64,
        64,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    let colors = [
        Color::srgb(0.8, 0.12, 0.08),
        Color::srgb(0.08, 0.7, 0.15),
        Color::srgb(0.08, 0.2, 0.8),
    ];
    commands
        .spawn((
            SceneActor(0),
            DespawnOnExit(ScenePhase::Active),
            Mesh3d(meshes.add(Cuboid::new(2.0, 2.0, 2.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: colors[counts.entered - 1],
                unlit: true,
                ..default()
            })),
            Transform::default(),
            RigidBody::Static,
            Collider::cuboid(2.0, 2.0, 2.0),
        ))
        .with_children(|parent| {
            parent.spawn((
                Camera3d::default(),
                Transform::from_xyz(4.0, 3.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ));
            // 实际地图含阴影方向光；只用 unlit 网格会漏掉旧灯光引用新相机的级联崩溃。
            parent.spawn((
                DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_xyz(3.0, 5.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
            ));
        });
}

fn observe_gpu_allocations(
    probe: Res<GpuProbe>,
    allocator: Res<MeshAllocator>,
    images: Res<RenderAssets<GpuImage>>,
    disabled: Query<(), With<Disabled>>,
    peers: Query<(), (With<bevy::render::sync_world::MainEntity>, Allow<Disabled>)>,
    cameras: Query<(), (With<Camera3d>, Allow<Disabled>)>,
    lights: Query<(), (With<bevy::pbr::ExtractedDirectionalLight>, Allow<Disabled>)>,
) {
    let mut proof = probe.0.lock().unwrap();
    proof.render_frames += 1;
    proof.remaining_scene_views = cameras.iter().count();
    proof.remaining_scene_lights = lights.iter().count();
    let (Some(entity), Some(mesh), Some(image)) = (proof.render_entity, proof.mesh, proof.image)
    else {
        return;
    };
    proof.peer_alive = peers.contains(entity);
    let vertices = allocator.mesh_vertex_slice(&mesh);
    let image = images.get(image);
    proof.mesh_seen |= vertices.is_some();
    proof.image_seen |= image.is_some();
    let (Some(vertices), Some(image)) = (vertices, image) else {
        return;
    };
    let current = (
        format!("{:?}", vertices.buffer.id()),
        format!("{:?}", image.texture.id()),
    );
    if let Some(original) = proof.allocation.as_ref() {
        assert_eq!(&current, original, "切页不得释放并重建原GPU顶点缓冲或纹理");
    } else {
        proof.allocation = Some(current);
    }
    if disabled.contains(entity) {
        proof.paused_seen = true;
    } else if proof.paused_seen {
        proof.resumed_seen = true;
    }
}

fn start_target(world: &mut World, map: &MapId, warm: bool) {
    world.resource_mut::<ActiveDynamicMap>().select(map.clone());
    arm_resident_scene_transition(world);
    world.resource_mut::<NextState<ScenePhase>>().set(if warm {
        ScenePhase::Active
    } else {
        ScenePhase::Loading
    });
}

fn drive_gpu_smoke(world: &mut World) {
    let frame = world.resource::<FrameCount>().0;
    if frame > 900 {
        panic!(
            "GPU驻留验收未完成：阶段{}，证据{:?}",
            world.resource::<Smoke>().phase,
            world.resource::<GpuProbe>().0.lock().unwrap()
        );
    }
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let (a, b, c) = (
            id("MAP-gpu-resident-a"),
            id("MAP-gpu-resident-b"),
            id("MAP-gpu-resident-c"),
        );
        let scene = *world.resource::<State<ScenePhase>>().get();
        match smoke.phase {
            0 if frame > 30 => {
                let root = world
                    .query_filtered::<Entity, With<SceneActor>>()
                    .single(world)
                    .unwrap();
                let camera = world.get::<Children>(root).unwrap()[0];
                let Some(render) = world.get::<RenderEntity>(camera) else {
                    return;
                };
                let probe = world.resource::<GpuProbe>().0.clone();
                let mut proof = probe.lock().unwrap();
                proof.render_entity = Some(render.id());
                proof.mesh = Some(world.get::<Mesh3d>(root).unwrap().0.id());
                proof.image = Some(world.resource::<MapRuntimeProof>().image.id());
                if proof.allocation.is_none() {
                    return;
                }
                smoke.root = Some(root);
                smoke.pixels = world.resource::<MapRuntimeProof>().pixels.clone();
                mark_resident_document_open(world, a.clone());
                prepare_resident_scene(world, b.clone(), ScenePhase::Active);
                smoke.phase = 1;
            }
            1 if world.resource::<ResidentScenes>().preparation(&b)
                == ResidentScenePreparation::ColdReady =>
            {
                smoke.paused_tick = world.get::<SceneActor>(smoke.root.unwrap()).unwrap().0;
                start_target(world, &b, false);
                smoke.phase = 2;
            }
            2 if scene == ScenePhase::Loading => {
                world
                    .resource_mut::<NextState<ScenePhase>>()
                    .set(ScenePhase::Active);
                smoke.since = frame;
                smoke.phase = 3;
            }
            3 if scene == ScenePhase::Active && frame > smoke.since + 30 => {
                mark_resident_document_open(world, b.clone());
                assert_eq!(
                    world.get::<SceneActor>(smoke.root.unwrap()).unwrap().0,
                    smoke.paused_tick
                );
                prepare_resident_scene(world, c.clone(), ScenePhase::Active);
                smoke.phase = 4;
            }
            4 if world.resource::<ResidentScenes>().preparation(&c)
                == ResidentScenePreparation::ColdReady =>
            {
                start_target(world, &c, false);
                smoke.phase = 5;
            }
            5 if scene == ScenePhase::Loading => {
                world
                    .resource_mut::<NextState<ScenePhase>>()
                    .set(ScenePhase::Active);
                smoke.since = frame;
                smoke.phase = 6;
            }
            6 if scene == ScenePhase::Active && frame > smoke.since + 30 => {
                mark_resident_document_open(world, c.clone());
                prepare_resident_scene(world, a.clone(), ScenePhase::Active);
                smoke.phase = 7;
            }
            7 if world.resource::<ResidentScenes>().preparation(&a)
                == ResidentScenePreparation::WarmReady =>
            {
                let image = world.resource::<MapRuntimeProof>().image.id();
                world
                    .resource_mut::<Messages<feathrise_scene_core::residency::ResidentAssetEvent<Image>>>()
                    .write(feathrise_scene_core::residency::ResidentAssetEvent(
                        AssetEvent::Modified { id: image },
                    )); // CPU重校验不得向GPU上传系统伪造重新修改。
                start_target(world, &a, true);
                smoke.phase = 8;
                smoke.since = frame;
            }
            8 if world.resource::<ResidentScenes>().restored(&a) && frame > smoke.since + 20 => {
                let root = world
                    .query_filtered::<Entity, With<SceneActor>>()
                    .single(world)
                    .unwrap();
                assert_eq!(Some(root), smoke.root);
                assert!(Arc::ptr_eq(
                    &smoke.pixels,
                    &world.resource::<MapRuntimeProof>().pixels
                ));
                assert_eq!(world.resource::<ConstructionCounts>().entered, 3);
                let proof = world.resource::<GpuProbe>().0.lock().unwrap();
                assert!(proof.paused_seen && proof.resumed_seen);
                let camera = world.get::<Children>(root).unwrap()[0];
                assert_eq!(
                    world.get::<RenderEntity>(camera).unwrap().id(),
                    proof.render_entity.unwrap()
                );
                drop(proof);
                let path =
                    std::path::PathBuf::from(std::env::var("RPG_RESIDENT_SCENE_GPU_PATH").unwrap());
                world
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path))
                    .observe(|_: On<ScreenshotCaptured>, probe: Res<GpuProbe>| {
                        probe.0.lock().unwrap().captured = true;
                    });
                smoke.phase = 9;
            }
            9 if world.resource::<GpuProbe>().0.lock().unwrap().captured => {
                // 关闭当前 A 并恢复 C：原用例只切换不销毁，漏掉 Disabled 的 Remove 也会在 despawn 时触发。
                close_resident_scene(world, a.clone());
                prepare_resident_scene(world, c.clone(), ScenePhase::Active);
                smoke.phase = 10;
            }
            10 if world.resource::<ResidentScenes>().preparation(&c)
                == ResidentScenePreparation::WarmReady =>
            {
                start_target(world, &c, true);
                smoke.phase = 11;
                smoke.since = frame;
            }
            11 if world.resource::<ResidentScenes>().restored(&c) && frame > smoke.since + 20 => {
                assert!(world.get_entity(smoke.root.unwrap()).is_err());
                assert!(
                    !world.resource::<GpuProbe>().0.lock().unwrap().peer_alive,
                    "地图关闭后原渲染相机必须真正退役，不能成为被重新激活的孤儿视图"
                );
                close_resident_scene(world, b.clone());
                close_resident_scene(world, c.clone());
                world
                    .resource_mut::<NextState<ScenePhase>>()
                    .set(ScenePhase::Idle);
                smoke.phase = 12;
                smoke.since = frame;
            }
            12 if scene == ScenePhase::Idle && frame > smoke.since + 20 => {
                assert!(
                    world.resource::<ResidentScenes>().maps.is_empty(),
                    "最后一张地图必须完成关闭"
                );
                let mut proof = world.resource::<GpuProbe>().0.lock().unwrap();
                assert_eq!(
                    proof.remaining_scene_views, 0,
                    "关闭全部地图后不能残留禁用相机"
                );
                assert_eq!(
                    proof.remaining_scene_lights, 0,
                    "关闭全部地图后不能残留禁用灯光"
                );
                proof.closed_all = true;
                drop(proof);
                world.write_message(AppExit::Success);
                smoke.phase = 13;
            }
            _ => {}
        }
    });
}

#[test]
#[ignore = "需要真实GPU和独立窗口；RPG_RESIDENT_SCENE_GPU_PATH指定截图，单项运行"]
fn three_full_gpu_scenes_keep_original_vertex_buffer_texture_and_render_entity() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WinitPlugin {
                run_on_any_thread: true,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "完整三维场景驻留验收".into(),
                    resolution: (640, 480).into(),
                    focused: false,
                    ..default()
                }),
                ..default()
            }),
    );
    app.init_state::<ScenePhase>()
        .init_resource::<ActiveDynamicMap>()
        .init_resource::<ConstructionCounts>();
    install(&mut app);
    app.world_mut()
        .resource_mut::<SceneResidencyActivity>()
        .enabled = true;
    app.init_resident_resource::<MapRuntimeProof>()
        .init_resource::<Smoke>()
        .add_plugins(ExtractResourcePlugin::<GpuProbe>::default())
        .init_resource::<GpuProbe>()
        .add_systems(OnEnter(ScenePhase::Active), build_gpu_scene)
        .add_resident_systems(Update, update_actors)
        .add_systems(
            Last,
            drive_gpu_smoke.after(super::super::systems::advance_scene_residency),
        );
    app.sub_app_mut(RenderApp).add_systems(
        Render,
        observe_gpu_allocations
            .in_set(RenderSystems::PrepareResources)
            .after(RenderSystems::PrepareAssets),
    );
    let probe = app.world().resource::<GpuProbe>().0.clone();
    app.world_mut()
        .resource_mut::<ActiveDynamicMap>()
        .select(id("MAP-gpu-resident-a"));
    app.world_mut()
        .resource_mut::<NextState<ScenePhase>>()
        .set(ScenePhase::Active);
    assert_eq!(
        app.run(),
        AppExit::Success,
        "渲染线程崩溃必须使回归失败，截图成功不能替代完整关闭验收"
    );
    assert!(probe.lock().unwrap().captured);
    assert!(probe.lock().unwrap().closed_all);
}
