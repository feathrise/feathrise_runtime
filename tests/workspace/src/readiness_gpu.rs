//! 无窗口真实GPU验证正式地表预热门；根图和两个实例分别取得同实体、同视图的排队证据。
//! 【功能设计】共享网格、材质和相机刻意产生相同管线，隐藏其中一个来源后不能借其他来源放行。

use feathrise_environment::terrain_surface::{
    MAX_TERRAIN_SURFACE_LAYERS, TerrainSurfaceCarrier, TerrainSurfaceDescriptor,
    TerrainSurfaceExtension, TerrainSurfaceFallbackImages, TerrainSurfaceInstanceFeedback,
    TerrainSurfaceInstanceScope, TerrainSurfaceInstanceWarmup, TerrainSurfaceLayer,
    TerrainSurfaceMaterial, TerrainSurfacePhase, TerrainSurfacePlugin, TerrainSurfaceRenderStats,
    TerrainSurfaceTextureProbe, TerrainSurfaceWarmup, terrain_surface_warmup_can_finalize,
};
use bevy::{
    camera::visibility::NoFrustumCulling,
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass, NormalPrepass},
    material::OpaqueRendererMethod,
    prelude::*,
    render::{
        RenderPlugin,
        error_handler::RenderErrorHandler,
        render_resource::{TextureFormat, WgpuFeatures},
        settings::WgpuSettings,
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::time::{Duration, Instant};

#[derive(Resource)]
struct DiagnosticMode(OpaqueRendererMethod);

/// 失败定位只打印有限载体的管线身份和状态；不输出Shader全文，也不参与正式就绪判定。
#[allow(clippy::too_many_arguments)]
fn pipeline_diagnostics(
    mode: Res<DiagnosticMode>,
    mut last: Local<Option<Instant>>,
    carriers: Query<(&bevy::render::sync_world::MainEntity, Option<&TerrainSurfaceInstanceScope>), With<TerrainSurfaceCarrier>>,
    views: Query<(&bevy::render::view::ExtractedView, &bevy::render::view::RenderVisibleEntities)>,
    pipelines: Res<bevy::pbr::SpecializedMaterialPipelineCache>,
    prepass: Res<bevy::pbr::SpecializedPrepassMaterialPipelineCache>,
    cache: Res<bevy::render::render_resource::PipelineCache>,
) {
    if last.is_some_and(|last| last.elapsed() < Duration::from_secs(1)) { return; }
    *last = Some(Instant::now());
    let describe = |id| {
        use bevy::render::render_resource::CachedPipelineState;
        let state = cache.get_render_pipeline_state(id);
        let phase = match state {
            CachedPipelineState::Queued => "Queued",
            CachedPipelineState::Creating(_) => "Creating",
            CachedPipelineState::Ok(_) => "Ok",
            CachedPipelineState::Err(_) => "Err",
        };
        // 未处理的Queued新管线尚无可读descriptor；其余状态只读取短label，不打印错误中的WGSL正文。
        let label = if matches!(state, CachedPipelineState::Queued) {
            None
        } else { cache.get_render_pipeline_descriptor(id).label.as_deref() };
        format!("{id:?}/{phase}/label={label:?}")
    };
    for (entity, scope) in &carriers {
        for (view, visible) in &views {
            let view_id = view.retained_view_entity;
            let visible = visible.get::<Mesh3d>().is_some_and(|visible| {
                visible.entities_cpu_culling.binary_search_by_key(entity, |(_, main)| *main).is_ok()
                    || visible.entities_gpu_culling.contains_key(entity)
            });
            let forward = pipelines.get(&view_id).and_then(|entries| entries.get(entity)).map(|id| describe(*id));
            let prepass = prepass.get(&view_id).and_then(|entries| entries.get(entity)).map(|(_, id, _)| describe(*id));
            eprintln!("地表GPU诊断 mode={:?} carrier={entity:?} scope={scope:?} view={view_id:?} visible={visible} forward={forward:?} prepass={prepass:?}", mode.0);
        }
    }
}

struct Fixture {
    app: App,
    root: Entity,
    first: Entity,
    second: Entity,
    second_probe: Entity,
    instances: [Entity; 2],
    images: Vec<Handle<Image>>,
    mesh: Handle<Mesh>,
    material: Handle<TerrainSurfaceMaterial>,
}

/// Bevy先收集条件导入再应用shader defs；独立环境夹具也必须登记正式宿主提供的高度函数。
fn install_host_height_shader(app: &mut App) {
    // 复用实际WGSL，并按生产clipmap_stitch_shader_source(false)剥离compute分支。
    // 本测试不启用blade后端，避免为一份公共导入安装植被渲染和额外GPU调度。
    let mut enabled = vec![true];
    let mut stitch_source = String::new();
    for line in include_str!("../../../crates/feathrise_vegetation/src/blade/shaders/clipmap_stitches.wgsl").lines() {
        let trimmed = line.trim();
        if trimmed.strip_prefix("#ifdef ").is_some() {
            enabled.push(false);
            continue;
        }
        if trimmed == "#else" {
            let old = enabled.pop().unwrap();
            enabled.push(*enabled.last().unwrap() && !old);
            continue;
        }
        if trimmed == "#endif" {
            enabled.pop();
            continue;
        }
        if !trimmed.starts_with('#') && *enabled.last().unwrap() {
            stitch_source.push_str(line);
            stitch_source.push('\n');
        }
    }
    let source = format!("{stitch_source}\n{}", include_str!(
        "../../../crates/feathrise_scene/src/maps/dynamic_map/clipmap/shaders/height.wgsl"
    ));
    let handle: Handle<Shader> = bevy::asset::uuid_handle!("7931b3c4-7851-4c15-9f6f-f20102d0ed00");
    app.world_mut().resource_mut::<Assets<Shader>>().insert(
        handle.id(),
        Shader::from_wgsl(source, "readiness_gpu_tests/host_clipmap_height.wgsl"),
    ).unwrap();
}

fn fixture(method: OpaqueRendererMethod) -> Fixture {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: WgpuSettings {
                    // 正式Deferred地表片元使用三角编号；不需要安装Solari或启动游戏场景。
                    backends: Some(bevy::render::settings::Backends::VULKAN), // 与当前正式光追程序的后端一致，不能仅在其他后端通过。
                    features: WgpuFeatures::PRIMITIVE_INDEX | WgpuFeatures::IMMEDIATES,
                    ..default()
                }
                .into(),
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<bevy::log::LogPlugin>(),
    )
    .add_plugins(TerrainSurfacePlugin)
    .insert_resource(RenderErrorHandler(|error, _, _| {
        panic!("地表预热门真实GPU验证失败：{error:?}")
    }));
    install_host_height_shader(&mut app);
    let render = app.get_sub_app_mut(bevy::render::RenderApp).unwrap();
    render.world_mut().insert_resource(DiagnosticMode(method));
    render.add_systems(bevy::render::Render, pipeline_diagnostics
        .in_set(bevy::render::RenderSystems::QueueMeshes)
        .after(bevy::pbr::queue_material_meshes)
        .after(bevy::pbr::queue_prepass_material_meshes));
    app.finish();
    app.cleanup();

    let target = app.world_mut().resource_mut::<Assets<Image>>().add(
        Image::new_target_texture(64, 64, TextureFormat::Bgra8UnormSrgb, None),
    );
    let mut camera = app.world_mut().spawn((
        Camera3d::default(),
        bevy::camera::RenderTarget::from(target),
        Msaa::Off,
        Transform::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    if method == OpaqueRendererMethod::Deferred {
        camera.insert((DepthPrepass, NormalPrepass, DeferredPrepass));
    }
    let fallback = app.world().resource::<TerrainSurfaceFallbackImages>().clone();
    let images = vec![
        fallback.zero_weight.clone(),
        fallback.zero_index.clone(),
        fallback.white_base_color_array.clone(),
        fallback.neutral_normal_array.clone(),
        fallback.neutral_arm_array.clone(),
    ];
    let extension = TerrainSurfaceExtension::new(TerrainSurfaceDescriptor {
        world_min: Vec2::splat(-8.0),
        world_max: Vec2::splat(8.0),
        weight_maps: [fallback.zero_weight.clone(), fallback.zero_weight.clone()],
        weight_map_count: 1,
        base_color_array: fallback.white_base_color_array,
        arm_array: fallback.neutral_arm_array,
        normal_array: fallback.neutral_normal_array,
        index_map: fallback.zero_index,
        layer_table: fallback.neutral_layer_table,
        layers: [TerrainSurfaceLayer::default(); MAX_TERRAIN_SURFACE_LAYERS],
        layer_count: 1,
        residual_layer: 0,
        grass_layer: 0,
        grass_micro_normal_strength: 0.0,
        grass_variation_cell_meters: 1.0,
    });
    let material = app.world_mut().resource_mut::<Assets<TerrainSurfaceMaterial>>().add(
        TerrainSurfaceMaterial {
            base: StandardMaterial {
                opaque_render_method: method,
                ..default()
            },
            extension,
        },
    );
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(Cuboid::default());
    let root = app.world_mut().spawn((
        Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()),
        TerrainSurfaceCarrier, NoFrustumCulling, Transform::from_xyz(-2.0, 0.0, 0.0),
    )).id();
    app.world_mut().spawn(TerrainSurfaceTextureProbe::new(images.clone()));
    {
        let mut stats = app.world_mut().resource_mut::<TerrainSurfaceRenderStats>();
        stats.generation = 41;
        stats.phase = TerrainSurfacePhase::Warming;
    }
    let instances = [app.world_mut().spawn_empty().id(), app.world_mut().spawn_empty().id()];
    let mut carriers = Vec::new();
    let mut probes = Vec::new();
    for (index, instance) in instances.into_iter().enumerate() {
        let scope = TerrainSurfaceInstanceScope {
            instance,
            generation: [7, 23][index],
        };
        carriers.push(app.world_mut().spawn((
            Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()),
            TerrainSurfaceCarrier, TerrainSurfaceInstanceWarmup, scope,
            NoFrustumCulling, Transform::from_xyz(index as f32 * 2.0, 0.0, 0.0),
        )).id());
        probes.push(app.world_mut().spawn((
            TerrainSurfaceTextureProbe::new(images.clone()), TerrainSurfaceInstanceWarmup, scope,
        )).id());
    }
    Fixture { app, root, first: carriers[0], second: carriers[1], second_probe: probes[1], instances, images, mesh, material }
}

fn instance_stats(app: &App, instance: Entity) -> Option<TerrainSurfaceRenderStats> {
    app.world().resource::<TerrainSurfaceInstanceFeedback>().0.iter()
        .find_map(|(entity, stats)| (*entity == instance).then_some(*stats))
}

fn ready(stats: &TerrainSurfaceRenderStats) -> bool {
    terrain_surface_warmup_can_finalize(stats, 1, 2)
}

fn wait_for(app: &mut App, label: &str, condition: impl Fn(&App) -> bool) {
    let started = Instant::now();
    let mut settled_frames = 0;
    loop {
        app.update();
        // 生产证据晚一完整渲染帧回写；连续三帧成立才能排除修改前最后一帧的旧快照。
        settled_frames = if condition(app) { settled_frames + 1 } else { 0 };
        if settled_frames >= 3 { return; }
        assert!(started.elapsed() < Duration::from_secs(40),
            "{label}超时；root={:?}；instances={:?}",
            app.world().resource::<TerrainSurfaceRenderStats>(),
            app.world().resource::<TerrainSurfaceInstanceFeedback>().0);
        // 只在测试夹具中给异步管线编译和渲染线程让出执行时间，生产系统不阻塞。
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "需要真实GPU；无窗口验证Forward/Deferred根图与两个实例的地表预热及同管线隔离"]
fn forward_and_deferred_surface_readiness_keeps_root_instances_and_hidden_sources_isolated() {
    for method in [OpaqueRendererMethod::Forward, OpaqueRendererMethod::Deferred] {
        let Fixture { mut app, root, first, second, second_probe, instances, images, mesh, material } = fixture(method);
        wait_for(&mut app, "三份正式地表完整GPU证据", |app| {
            ready(app.world().resource::<TerrainSurfaceRenderStats>())
                && instances.into_iter().all(|instance| instance_stats(app, instance).is_some_and(|stats| ready(&stats)))
        });
        let root_stats = *app.world().resource::<TerrainSurfaceRenderStats>();
        assert_eq!(root_stats.generation, 41, "渲染反馈必须保留根图自己的地表世代");
        assert_eq!(root_stats.texture_expected, 5, "根图不能聚合两个实例的纹理探针");
        assert_eq!(root_stats.carrier_instances, 1, "根图不能聚合两个实例的承接实体");
        for (instance, generation) in instances.into_iter().zip([7, 23]) {
            let stats = instance_stats(&app, instance).unwrap();
            assert_eq!(stats.generation, generation);
            assert_eq!(stats.texture_expected, 5);
            assert_eq!(stats.carrier_instances, 1);
            assert_eq!(stats.pipeline_error, 0);
        }

        // 两个实例仍在绘制同一管线时，隐藏根图不能借用它们的排队结果。
        app.world_mut().entity_mut(root).insert(Visibility::Hidden);
        wait_for(&mut app, "隐藏根图撤回GPU证据", |app| {
            !ready(app.world().resource::<TerrainSurfaceRenderStats>())
                && app.world().resource::<TerrainSurfaceRenderStats>().phase_queued == 0
                && instances.into_iter().all(|instance| instance_stats(app, instance).is_some_and(|stats| ready(&stats)))
        });

        // 热更新会保留隐藏的旧活动实体；此时只验收新离屏候选，不能要求旧实体也重新进入视口。
        let original_extension = app.world().resource::<Assets<TerrainSurfaceMaterial>>()
            .get(&material).unwrap().extension.clone();
        let warmup = app.world_mut().spawn((
            Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()),
            TerrainSurfaceCarrier, TerrainSurfaceWarmup, NoFrustumCulling,
            Transform::from_xyz(-2.0, -100_000.0, 0.0).with_scale(Vec3::splat(0.000_001)),
        )).id();
        wait_for(&mut app, "隐藏旧根图时新候选仍可完成预热", |app| {
            let stats = app.world().resource::<TerrainSurfaceRenderStats>();
            ready(stats) && stats.carrier_instances == 1 && stats.texture_expected == 5
        });
        assert_eq!(app.world().get::<Visibility>(root), Some(&Visibility::Hidden));
        assert_eq!(app.world().get::<Mesh3d>(root).unwrap().0, mesh);
        assert_eq!(app.world().get::<MeshMaterial3d<TerrainSurfaceMaterial>>(root).unwrap().0, material);
        assert_eq!(app.world().resource::<Assets<TerrainSurfaceMaterial>>()
            .get(&material).unwrap().extension, original_extension,
            "预热证据筛选不能改写旧活动材质参数");
        assert_eq!(app.world().resource::<Assets<TerrainSurfaceMaterial>>()
            .get(&material).unwrap().base.opaque_render_method, method);
        app.world_mut().despawn(warmup);
        wait_for(&mut app, "取消候选后隐藏旧根图不能保留旧Ready", |app| {
            let stats = app.world().resource::<TerrainSurfaceRenderStats>();
            !ready(stats) && stats.phase_queued == 0
                && instances.into_iter().all(|instance| instance_stats(app, instance).is_some_and(|stats| ready(&stats)))
        });
        app.world_mut().entity_mut(root).insert(Visibility::Inherited);
        wait_for(&mut app, "移除候选后旧根图恢复独立GPU证据", |app| {
            let stats = app.world().resource::<TerrainSurfaceRenderStats>();
            ready(stats) && stats.carrier_instances == 1
        });
        app.world_mut().entity_mut(second).insert(Visibility::Hidden);
        wait_for(&mut app, "隐藏第二实例撤回同管线排队证据", |app| {
            ready(app.world().resource::<TerrainSurfaceRenderStats>())
                && instance_stats(app, instances[0]).is_some_and(|stats| ready(&stats))
                && instance_stats(app, instances[1]).is_some_and(|stats| !ready(&stats) && stats.phase_queued == 0)
        });

        // 新世代只改第二实例；根图和第一实例不能被旧世代回执或另一个实例的代次污染。
        let next_scope = TerrainSurfaceInstanceScope { instance: instances[1], generation: 24 };
        app.world_mut().entity_mut(second).insert((Visibility::Inherited, next_scope));
        app.world_mut().entity_mut(second_probe).insert(next_scope);
        wait_for(&mut app, "第二实例新世代完整预热", |app| {
            ready(app.world().resource::<TerrainSurfaceRenderStats>())
                && instance_stats(app, instances[0]).is_some_and(|stats| ready(&stats) && stats.generation == 7)
                && instance_stats(app, instances[1]).is_some_and(|stats| ready(&stats) && stats.generation == 24)
        });

        // 独立探针缺少一份真实GPU纹理时，即使其他同材质来源完全就绪也不能误放行。
        let missing = bevy::asset::uuid_handle!("08b373f0-f389-4c8a-9fb8-30eb618e50ae");
        let mut incomplete = images.clone();
        incomplete.push(missing);
        app.world_mut().entity_mut(second_probe).insert(TerrainSurfaceTextureProbe::new(incomplete));
        wait_for(&mut app, "第二实例独立纹理门", |app| {
            ready(app.world().resource::<TerrainSurfaceRenderStats>())
                && instance_stats(app, instances[0]).is_some_and(|stats| ready(&stats))
                && instance_stats(app, instances[1]).is_some_and(|stats| {
                    !ready(&stats) && stats.texture_expected == 6 && stats.texture_ready == 5 && stats.texture_waiting == 1
                })
        });
        app.world_mut().entity_mut(second_probe).insert(TerrainSurfaceTextureProbe::new(images));
        wait_for(&mut app, "第二实例纹理恢复后完整预热", |app| {
            instance_stats(app, instances[1]).is_some_and(|stats| ready(&stats) && stats.generation == 24)
        });
        assert_eq!(app.world().resource::<TerrainSurfaceRenderStats>().generation, 41);
        assert_eq!(app.world().get::<TerrainSurfaceInstanceScope>(first).unwrap().generation, 7);
    }
}
