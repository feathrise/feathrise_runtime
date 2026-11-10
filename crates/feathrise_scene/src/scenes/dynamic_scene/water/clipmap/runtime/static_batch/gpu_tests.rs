//! 隐藏独立地形的静水预热、真实PBR回执、换高度后重验及有界退役回归。

use super::*;
use bevy::ecs::world::CommandQueue;
use bevy::render::render_resource::{CachedPipelineState, Extent3d, PipelineCache, TextureDimension, TextureFormat};
use feathrise_scene_core::residency::{
    InstanceResourceTable, SceneDetachedRoots, SceneInstanceContextAppExt, SceneInstanceDetached,
    SceneInstanceOwner, SceneRuntimeScope, initialize_instance_context,
};
use crate::scenes::dynamic_scene::clipmap::{
    ClipmapBounds, ClipmapFlatHeightPage, ClipmapHeightAtlas, ClipmapHeightMaterialPlugin, SparseHeightPageRuntime,
    encode_page_table_entry, test_ground_snapshot,
};

fn hidden_instance(app: &mut App) -> Entity {
    let instance = app.world_mut().spawn_empty().id();
    app.world_mut().entity_mut(instance).insert((SceneInstanceOwner(instance), Transform::IDENTITY, Visibility::Hidden));
    initialize_instance_context(app.world_mut(), instance);
    instance
}

/// 通过正式生产构造器生成一层；CommandQueue仍在实例作用域中应用，完整触发归属观察者。
fn stage_hidden_layer(
    app: &mut App,
    instance: Entity,
    mesh: &Mesh3d,
    height: &ClipmapHeightExtension,
    mask: Handle<Image>,
    height_revision: u64,
    water_level: f32,
) -> StaticBatch {
    let mut ground = test_ground_snapshot();
    ground.revision[1] = height_revision;
    let layer: SceneWaterLayer = serde_json::from_value(serde_json::json!({
        "id":"hidden-still", "label":"隐藏候选静水", "mask_source_path":"test.r8",
        "mask_texture_path":"test.ktx2", "resolution":[2,2], "water_level":water_level
    })).unwrap();
    let layer = PendingLayer {
        image: Some(mask),
        prepared: PreparedLayer {
            descriptor: WaterSurfaceDescriptor::from_layer(&layer),
            layer,
            pixels: Arc::from(vec![255; 4]),
            coverage_bounds: Some([0, 0, 2, 2]),
            draft_image: None,
        },
    };
    app.world_mut().resource_mut::<SceneRuntimeScope>().instance = Some(instance);
    let mut batch = app.world_mut().resource_scope(|world, mut materials: Mut<Assets<ClipmapWaterMaterial>>| {
        let mut queue = CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        let mut batch = StaticBatch::new(&mut commands, ground.revision);
        stage_layer(&mut commands, &mut materials, &mut batch, &layer, &ground.scene_id,
            (Vec2::ZERO, Vec2::new(96., 64.)), &ground, mesh, Transform::IDENTITY, height, 0.);
        drop(commands);
        queue.apply(world);
        batch
    });
    *app.world_mut().resource_mut::<SceneRuntimeScope>() = SceneRuntimeScope::default();
    batch.ticket = Some(app.world_mut().resource_mut::<WaterGpuUploadQueue>().submit_assets(
        vec![mesh.0.id()], vec![batch.layers[0].material.id().untyped()], vec![batch.warmups[0]],
    ));
    batch
}

fn assert_detached_owner(app: &App, instance: Entity, batch: &StaticBatch) {
    let warmup = batch.warmups[0];
    assert!(app.world().get::<SceneInstanceDetached>(warmup).is_some());
    assert_eq!(app.world().get::<SceneInstanceOwner>(warmup), Some(&SceneInstanceOwner(instance)));
    assert!(app.world().get::<ChildOf>(warmup).is_none(),
        "离屏预热不能继承隐藏候选父链，否则材质专用化与整批采用会互相等待");
    assert!(app.world().resource::<InstanceResourceTable<SceneDetachedRoots>>().values[&instance].0.contains(&warmup),
        "脱离层级的预热仍必须在实例退役登记中");
    assert_eq!(app.world().get::<ChildOf>(batch.root).unwrap().parent(), instance);
    assert_eq!(app.world().get::<ChildOf>(batch.layers[0].entity).unwrap().parent(), batch.root);
    assert!(!batch.published.load(Ordering::Acquire), "GPU就绪之前正式静水不能先发布");
}

fn retire_batch(app: &mut App, batch: StaticBatch) {
    app.world_mut().resource_scope(|world, mut runtime: Mut<ClipmapWaterRuntime>| {
        world.resource_scope(|world, mut uploads: Mut<WaterGpuUploadQueue>| {
            let mut queue = CommandQueue::default();
            batch.retire(&mut Commands::new(&mut queue, world), &mut runtime, &mut uploads);
            queue.apply(world);
        });
    });
}

#[test]
fn hidden_static_water_warmup_keeps_owner_and_retires_outside_candidate_tree() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_instance_resource::<ClipmapWaterRuntime>()
        .init_resource::<WaterGpuUploadQueue>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<ClipmapWaterMaterial>>()
        .add_systems(First, reap);
    crate::shared::large_scene::install_water_retirement(&mut app);
    let instance = hidden_instance(&mut app);
    let mesh = Mesh3d(app.world_mut().resource_mut::<Assets<Mesh>>().add(Cuboid::new(1., 1., 1.)));
    let height = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::ZERO, Vec2::new(96., 64.)).unwrap(), UVec2::new(96, 64),
        &ClipmapFlatHeightPage { heights: default(), page_table: default() },
    ).unwrap();
    let batch = stage_hidden_layer(&mut app, instance, &mesh, &height, default(), 1, 1.);
    assert_detached_owner(&app, instance, &batch);
    let (root, water, warmup, material) =
        (batch.root, batch.layers[0].entity, batch.warmups[0], batch.layers[0].material.id());
    retire_batch(&mut app, batch);
    for _ in 0..3 { app.update(); }
    for entity in [root, water, warmup] { assert!(app.world().get_entity(entity).is_err()); }
    assert!(app.world().get_entity(instance).is_ok());
    assert!(app.world().resource::<Assets<Mesh>>().get(&mesh.0).is_some(), "共享地形Mesh不能随静水退役");
    assert!(app.world().resource::<Assets<ClipmapWaterMaterial>>().get(material).is_none());
}

fn gpu_app() -> App {
    gpu_app_with(|app| WaterGpuUploadQueue::install_test_support(app))
}

/// 允许像素回归在finish前安装完整水插件；原预热用例继续沿用相同的最小宿主。
fn gpu_app_with(install_water: impl FnOnce(&mut App)) -> App {
    use bevy::render::{RenderPlugin, settings::{RenderCreation, WgpuSettings}, render_resource::WgpuFeatures};
    let mut app = App::new();
    // 复用三角线GPU回归的正式PBR链，保留真实GPU证据且不打开游戏窗口。
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin,
        WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() },
        RenderPlugin { render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
            features: WgpuFeatures::IMMEDIATES, ..default()
        })), ..default() },
        bevy::image::ImagePlugin::default(), bevy::mesh::MeshPlugin,
        bevy::camera::CameraPlugin, bevy::light::LightPlugin,
        bevy::core_pipeline::CorePipelinePlugin, bevy::gltf::GltfPlugin::default(), bevy::pbr::PbrPlugin::default(),
        bevy::state::app::StatesPlugin));
    // LightPlugin会注册灯光Gizmo系统；手动PBR宿主仍须提供其资产表和渲染资源。
    app.add_plugins((bevy::gizmos::GizmoPlugin, bevy::gizmos_render::GizmoRenderPlugin));
    app.init_state::<ScenePhase>()
        .add_plugins((feathrise_environment::terrain_surface::TerrainSurfacePlugin, ClipmapHeightMaterialPlugin,
            super::super::super::material::ClipmapWaterMaterialPlugin))
        .init_resource::<feathrise_scene_content::ActiveDynamicScene>()
        .init_resource::<crate::DynamicClipmapGroundSurface>()
        .init_instance_resource::<SparseClipmapRuntime>()
        .init_instance_resource::<SparseHeightPageRuntime>()
        .init_instance_resource::<ClipmapWaterRuntime>()
        .add_systems(First, reap);
    install_water(&mut app);
    app.sub_app_mut(bevy::render::RenderApp).add_systems(bevy::render::Render,
        (|cache: Res<PipelineCache>| {
            for pipeline in cache.pipelines() {
                if let CachedPipelineState::Err(error) = &pipeline.state {
                    if matches!(error, bevy::shader::ShaderCacheError::ShaderNotLoaded(_)
                        | bevy::shader::ShaderCacheError::ShaderImportNotYetAvailable) { continue; }
                    panic!("静水正式PBR管线失败：{error:?}");
                }
            }
        }).in_set(bevy::render::RenderSystems::Cleanup));
    app.finish(); app.cleanup();
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(
        Image::new_target_texture(64, 64, TextureFormat::Rgba8UnormSrgb, None));
    app.world_mut().spawn((Camera3d::default(), bevy::camera::RenderTarget::from(target),
        Transform::from_xyz(0., 6., 6.).looking_at(Vec3::ZERO, Vec3::Y), Msaa::Off));
    app
}

#[path = "painted_gpu_tests.rs"]
mod painted_gpu_tests;

fn gpu_frames(app: &mut App, count: usize) {
    for _ in 0..count { app.update(); std::thread::sleep(std::time::Duration::from_millis(16)); }
}

fn wait_for_ticket(app: &mut App, ticket: WaterGpuUploadTicket) {
    let start = std::time::Instant::now();
    loop {
        app.update();
        let receipts = app.world().resource::<WaterGpuUploadReceipts>();
        assert!(!receipts.has_failed(ticket), "静水PBR GPU回执明确失败：{ticket:?}");
        if receipts.is_ready(ticket) { return; }
        assert!(start.elapsed().as_secs() < 30, "隐藏独立地形的静水PBR GPU回执超时：{ticket:?}");
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

#[test]
#[ignore = "需要真实GPU；无窗口验证隐藏独立地形的静水PBR预热与高度换代回执"]
fn hidden_static_water_gpu_ticket_completes_and_rechecks_after_height_change() {
    let mut app = gpu_app();
    let instance = hidden_instance(&mut app);
    let plane = Plane3d::default().mesh().size(2., 2.).build();
    let vertices = plane.count_vertices();
    let mesh = Mesh3d(app.world_mut().resource_mut::<Assets<Mesh>>().add(
        plane.with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0., 0.]; vertices])));
    let mask = app.world_mut().resource_mut::<Assets<Image>>().add(Image::new_fill(
        Extent3d { width: 2, height: 2, depth_or_array_layers: 1 }, TextureDimension::D2,
        &[255], TextureFormat::R8Unorm, bevy::asset::RenderAssetUsages::default()));
    let mut height = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::ZERO, Vec2::new(96., 64.)).unwrap(), UVec2::new(96, 64),
        app.world().resource::<ClipmapFlatHeightPage>(),
    ).unwrap();
    let first = stage_hidden_layer(&mut app, instance, &mesh, &height, mask.clone(), 1, 1.);
    assert_detached_owner(&app, instance, &first);
    let first_ticket = first.ticket.unwrap();
    assert!(!app.world().resource::<WaterGpuUploadReceipts>().is_ready(first_ticket),
        "此用例必须运行真实RenderApp回执，不能使用纯ECS立即放行");

    // 还原缺少Detached时形成的隐藏父链：相同Mesh和材质即使已上传，也无法取得专用管线证据。
    app.world_mut().entity_mut(first.warmups[0]).insert(ChildOf(instance));
    gpu_frames(&mut app, 90);
    assert!(!app.world().resource::<WaterGpuUploadReceipts>().is_ready(first_ticket),
        "隐藏父链负对照应重现预热死等，不能误借其他实体的同材质管线信用");
    app.world_mut().entity_mut(first.warmups[0]).remove::<ChildOf>().insert(Visibility::Inherited);
    wait_for_ticket(&mut app, first_ticket);

    // 地形换代后继续修改静水水位；新R16页、页表与新水位材质不能复用旧ticket的已就绪信用。
    let mut heights = Image::new_fill(Extent3d { width: 257, height: 514, depth_or_array_layers: 1 },
        TextureDimension::D2, &32768u16.to_le_bytes(), TextureFormat::R16Unorm,
        bevy::asset::RenderAssetUsages::default());
    heights.reinterpret_stacked_2d_as_array(2).unwrap();
    heights.sampler = ImageSampler::linear();
    let entry = encode_page_table_entry(0, -1., 4.).unwrap();
    let table = Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D2,
        entry.into_iter().flat_map(u32::to_le_bytes).collect(), TextureFormat::Rgba32Uint,
        bevy::asset::RenderAssetUsages::default());
    let heights = app.world_mut().resource_mut::<Assets<Image>>().add(heights);
    let page_table = app.world_mut().resource_mut::<Assets<Image>>().add(table);
    height.set_atlas(ClipmapHeightAtlas { heights, page_table, max_mip: 0 }).unwrap();
    let second = stage_hidden_layer(&mut app, instance, &mesh, &height, mask, 2, 3.5);
    assert_detached_owner(&app, instance, &second);
    assert_eq!(app.world().get::<ClipmapWaterSurface>(first.layers[0].entity).unwrap().water_level, 1.);
    assert_eq!(app.world().get::<ClipmapWaterSurface>(second.layers[0].entity).unwrap().water_level, 3.5,
        "第二批CPU静水描述必须采用修改后的水位");
    assert_eq!(app.world().resource::<Assets<ClipmapWaterMaterial>>()
        .get(&second.layers[0].material).unwrap().extension.water.surface.x, 3.5,
        "正式GPU材质uniform必须与第二批静水水位一致");
    let second_ticket = second.ticket.unwrap();
    assert_ne!(first_ticket, second_ticket);
    assert!(!app.world().resource::<WaterGpuUploadReceipts>().is_ready(second_ticket));
    wait_for_ticket(&mut app, second_ticket);
    assert_eq!(*app.world().get::<Visibility>(instance).unwrap(), Visibility::Hidden,
        "GPU预热不能通过提前显示独立候选来解除等待");

    let retired = [first.root, first.layers[0].entity, first.warmups[0],
        second.root, second.layers[0].entity, second.warmups[0]];
    retire_batch(&mut app, first);
    retire_batch(&mut app, second);
    gpu_frames(&mut app, 6);
    for entity in retired { assert!(app.world().get_entity(entity).is_err()); }
    assert!(app.world().resource::<Assets<Mesh>>().get(&mesh.0).is_some());
}
