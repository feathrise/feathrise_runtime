//! 技能消息经真实Avian碰撞归属路由到独立网格；只驻内存，不读取或改写作者地图文件。

use super::*;
use avian3d::prelude::{Collider, PhysicsPlugins, RigidBody};
use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce};
use feathrise_map_content::{
    MAP_SCHEMA_VERSION, MapContentHandles, MapContentRegistry, MapInstancePath, MapManifestAsset,
    MapTerrainDescriptor, ResolvedSurfacePresetCatalog, SURFACE_PRESET_SCHEMA_VERSION,
    SparseHeightManifest, SparseHeightManifestAsset, SparseHeightSpec, SurfacePresetCatalogAsset,
    SurfaceSourceSaveIdentity, TerrainId,
};
use feathrise_scene_core::residency::{
    InstanceResourceTable, MapInstanceContextAppExt, MapInstanceOwner, MapRuntimeScope,
    initialize_instance_context,
};

fn fixture() -> (App, MapId) {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default(),
    ))
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ))
    .init_asset::<MapManifestAsset>()
    .init_asset::<MapSurfaceAsset>()
    .init_asset::<MapSurfaceSourceAsset>()
    .init_asset::<SparseHeightManifestAsset>()
    .init_instance_resource::<ActiveDynamicMap>()
    .init_instance_resource::<MapContentRegistry>()
    .init_instance_resource::<DynamicMapRuntimeState>()
    .init_instance_resource::<SparseHeightPageRuntime>()
    .init_instance_resource::<TerrainCollisionRegion>()
    .init_instance_resource::<SparseHeightSessionCrater>()
    .init_instance_resource::<CraterSurfaceSource>()
    .init_instance_resource::<DynamicSurfaceAuthorPreview>()
    .add_message::<SparseHeightCraterRequest>()
    .add_systems(Update, receive_crater_requests);
    let catalog = SurfacePresetCatalogAsset {
        schema_version: SURFACE_PRESET_SCHEMA_VERSION,
        texture_resolution: [1, 1],
        base_color_array_path: "config/surface/base.ktx2".into(),
        normal_array_path: "config/surface/normal.ktx2".into(),
        arm_height_array_path: "config/surface/arm.ktx2".into(),
        presets: vec![feathrise_map_content::SurfacePresetDescriptor {
            id: "soil".into(),
            label: "土壤".into(),
            aliases: Vec::new(),
            base_color_srgba: [1.0; 4],
            base_color_texture_path: None,
            normal_texture_path: None,
            arm_height_texture_path: None,
            tile_meters: 1.0,
            normal_strength: 1.0,
            normal_y_flip: false,
            height_blend_strength: 0.0,
            perceptual_roughness: 1.0,
            metallic: 0.0,
            crater_response: 1.0,
            projection: Default::default(),
        }],
    };
    app.insert_resource(SurfacePresetRegistry {
        catalog: Handle::default(),
        revision: 0,
        last_error: None,
        resolved: ResolvedSurfacePresetCatalog::build(&catalog).unwrap(),
    });
    let root = MapId::parse("MAP-crater-root").unwrap();
    app.world_mut()
        .resource_mut::<ActiveDynamicMap>()
        .select(root.clone());
    app.finish();
    (app, root)
}

fn get_mut<T: Resource<Mutability = bevy::ecs::component::Mutable>>(
    world: &mut World,
    instance: Option<Entity>,
) -> Mut<'_, T> {
    if let Some(instance) = instance {
        world
            .resource_mut::<InstanceResourceTable<T>>()
            .map_unchanged(|table| table.values.get_mut(&instance).unwrap())
    } else {
        world.resource_mut::<T>()
    }
}

fn context(
    app: &mut App,
    root_map: &MapId,
    source: &MapId,
    pose: Transform,
    independent: bool,
) -> (Option<Entity>, SparseHeightSpec, Entity) {
    let frame = MapSourceFrame::from_transform(pose);
    let owner = independent.then(|| {
        let batch = app
            .world_mut()
            .spawn((super::super::instances::SubMapActive, Transform::IDENTITY))
            .id();
        let entity = app.world_mut().spawn_empty().id();
        initialize_instance_context(app.world_mut(), entity);
        let mut path = MapInstancePath::root(root_map.clone());
        if source != root_map {
            path = path.child(
                feathrise_map_content::MapInstanceId::parse(format!("child-{}", entity.to_bits()))
                    .unwrap(),
            );
        }
        let path = path.terrain(TerrainId::parse(format!("grid-{}", entity.to_bits())).unwrap());
        app.world_mut().entity_mut(entity).insert((
            super::super::instances::SubMapInstance {
                path,
                source_map_id: source.clone(),
                generation: 1,
                batch,
            },
            frame,
            pose,
        ));
        get_mut::<ActiveDynamicMap>(app.world_mut(), Some(entity)).select(source.clone());
        entity
    });
    let collider_pose = Transform {
        translation: frame.to_world(Vec3::new(0.0, -0.25, 0.0)),
        ..pose
    };
    let collider = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(64.0, 0.5, 64.0),
            DynamicSparseTerrainCollider,
            collider_pose,
        ))
        .id();
    if let Some(owner) = owner {
        app.world_mut()
            .entity_mut(collider)
            .insert(MapInstanceOwner(owner));
    }
    // 使用生产规格允许的最小64米边长；保留0.125米格距，世界原点对应第256格。
    let spec = SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.125, -8.0, 16.0).unwrap();
    let manifest = app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .add(MapManifestAsset {
            vegetation_paint_groups: Default::default(),
            schema_version: MAP_SCHEMA_VERSION,
            id: source.clone(),
            display_name: "浅坑路由回归".into(),
            notes: String::new(),
            runtime_scene: "DynamicMapScene".into(),
            world_min: spec.world_min,
            world_max: spec.world_max,
            default_spawn: [0.0; 3],
            environment: Default::default(),
            terrain: MapTerrainDescriptor::empty(),
            terrains: None,
            sub_maps: Default::default(),
            puddles: None,
            asset_roots: Vec::new(),
        });
    get_mut::<MapContentRegistry>(app.world_mut(), owner)
        .maps
        .insert(
            source.clone(),
            MapContentHandles {
                manifest,
                portals: Handle::default(),
                placements: Handle::default(),
                characters_path: None,
                surface: None,
                vegetation: None,
                water: None,
                boundaries: None,
            },
        );
    let height = SparseHeightManifest::empty(source.clone(), spec.clone()).unwrap();
    get_mut::<SparseHeightPageRuntime>(app.world_mut(), owner).adopt_raytracing_source_for_test(
        collider,
        Handle::default(),
        &height,
        Handle::default(),
        1,
        None,
        None,
    );
    {
        let mut runtime = get_mut::<DynamicMapRuntimeState>(app.world_mut(), owner);
        runtime.active_map_id = Some(source.clone());
        runtime.root = Some(collider);
        runtime.sparse_height_path = Some("maps/test/height_tiles.map_height_manifest.json".into());
    }
    (owner, spec, collider)
}

fn enter_scope(world: &mut World, owner: Option<Entity>) {
    let frame = owner
        .and_then(|owner| world.get::<MapSourceFrame>(owner).copied())
        .unwrap_or_default();
    *world.resource_mut::<MapRuntimeScope>() = MapRuntimeScope {
        instance: owner,
        frame,
    };
}

fn prepare_and_apply(app: &mut App, owner: Option<Entity>) {
    enter_scope(app.world_mut(), owner);
    app.world_mut()
        .run_system_once(prepare_crater_surface)
        .unwrap();
    app.world_mut()
        .run_system_once(apply_pending_craters)
        .unwrap();
    enter_scope(app.world_mut(), None);
}

fn cast(app: &mut App, map: &MapId, position: Vec3) {
    app.world_mut().write_message(SparseHeightCraterRequest {
        map_id: map.clone(),
        origin: position + Vec3::Y,
        radius: 4.0,
        depth: 0.5,
        ground_query_mask: u32::MAX,
    });
    app.update();
}

#[test]
fn crater_routes_real_hit_to_one_independent_mesh_and_bare_soil_deforms_in_world_meters() {
    let (mut app, root) = fixture();
    let child = MapId::parse("MAP-crater-source").unwrap();
    let pose = Transform::from_xyz(20.0, 5.0, -3.0)
        .with_rotation(Quat::from_rotation_y(37_f32.to_radians()))
        .with_scale(Vec3::new(2.0, 3.0, 0.5));
    let (owner, spec, _) = context(&mut app, &root, &child, pose, true);
    let (lower, _, _) = context(
        &mut app,
        &root,
        &root,
        Transform::from_xyz(20.0, 0.0, -3.0),
        true,
    );
    for _ in 0..5 {
        app.update();
    }
    cast(&mut app, &root, pose.translation);
    assert_eq!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
            .pending
            .len(),
        1
    );
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), lower)
            .pending
            .is_empty()
    );
    assert!(
        app.world()
            .resource::<CraterSurfaceSource>()
            .pending
            .is_empty(),
        "空容器不能吞掉命中独立网格的请求"
    );
    prepare_and_apply(&mut app, owner);
    let craters = get_mut::<SparseHeightSessionCrater>(app.world_mut(), owner);
    assert_eq!(craters.map_id(), Some(&child));
    let center = feathrise_map_content::sparse_height::sample_crater_height(
        &spec,
        craters.tiles(),
        [256, 256],
        0.0,
    );
    assert!(
        (center * pose.scale.y + 0.5).abs() < 0.001,
        "世界坑深保持0.5米，不随网格Y缩放扩大"
    );
    let x = feathrise_map_content::sparse_height::sample_crater_height(
        &spec,
        craters.tiles(),
        [264, 256],
        0.0,
    ) * pose.scale.y;
    let z = feathrise_map_content::sparse_height::sample_crater_height(
        &spec,
        craters.tiles(),
        [256, 288],
        0.0,
    ) * pose.scale.y;
    assert!(
        (x - z).abs() < 0.001 && x < 0.0,
        "等世界距离沿两轴得到同一圆坑高度"
    );
    drop(craters);
    assert!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), lower)
            .tiles()
            .is_empty()
    );
    assert!(
        app.world()
            .resource::<SparseHeightSessionCrater>()
            .tiles()
            .is_empty()
    );
    for _ in 0..6 {
        app.update();
    }
    prepare_and_apply(&mut app, owner);
    assert_eq!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), owner)
            .changed_footprints_since(0)
            .count(),
        1,
        "同图普通移动/轮转调度保留原坑，消息只受理一次"
    );
}

#[test]
fn crater_real_glb_root_triangle_collider_blocks_lower_sparse_grid_without_primitive_on_root() {
    let (mut app, root) = fixture();
    let (lower, _, _) = context(&mut app, &root, &root, Transform::IDENTITY, true);
    let (upper, _, glb_root) = context(
        &mut app,
        &root,
        &root,
        Transform::from_xyz(0.0, 2.0, 0.0),
        true,
    );
    // 原始GLB正式链只在根安装整套三角Collider；可见Mesh后代才有Primitive标记。
    app.world_mut()
        .entity_mut(glb_root)
        .remove::<DynamicSparseTerrainCollider>()
        .insert((
            super::super::systems::DynamicMapTerrainRoot,
            Collider::trimesh(
                vec![
                    Vec3::new(-32.0, 0.25, -32.0),
                    Vec3::new(32.0, 0.25, -32.0),
                    Vec3::new(32.0, 0.25, 32.0),
                    Vec3::new(-32.0, 0.25, 32.0),
                ],
                vec![[0, 2, 1], [0, 3, 2]],
            ),
        ));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Plane3d::default().mesh().size(64.0, 64.0));
    let visual = app
        .world_mut()
        .spawn((
            Mesh3d(mesh),
            DynamicMapTerrainPrimitive,
            Transform::from_xyz(0.0, 0.25, 0.0),
            ChildOf(glb_root),
        ))
        .id();
    {
        let mut runtime = get_mut::<DynamicMapRuntimeState>(app.world_mut(), upper);
        runtime.sparse_height_path = None;
        runtime.glb_path = Some("maps/real-glb-root/scene.glb".into());
    }
    assert!(
        app.world()
            .get::<DynamicMapTerrainPrimitive>(glb_root)
            .is_none()
    );
    assert!(app.world().get::<Collider>(visual).is_none());
    for _ in 0..5 {
        app.update();
    }
    cast(&mut app, &root, Vec3::new(0.0, 2.0, 0.0));
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), lower)
            .pending
            .is_empty(),
        "射线首个真实地面是GLB根，不能跳过它给下面的SparseClipmap造坑"
    );
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), upper)
            .pending
            .is_empty(),
        "不支持浅坑的GLB也不生成伪请求"
    );
    prepare_and_apply(&mut app, lower);
    assert!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), lower)
            .tiles()
            .is_empty()
    );
}

#[test]
fn crater_legacy_single_terrain_uses_frozen_surface_response_and_clear_restores_author_height() {
    let (mut app, map) = fixture();
    let (owner, spec, _) = context(&mut app, &map, &map, Transform::IDENTITY, false);
    for _ in 0..5 {
        app.update();
    }
    cast(&mut app, &map, Vec3::ZERO);
    let response = Arc::new(
        SurfaceCraterResponse::new(
            Vec2::splat(-32.0),
            Vec2::splat(32.0),
            [1, 1],
            Arc::from([0_u8; 16]),
            Arc::from([255_u8, 0, 0, 0]),
            vec![0.1],
        )
        .unwrap(),
    );
    get_mut::<CraterSurfaceSource>(app.world_mut(), owner).snapshot = Some(response);
    app.world_mut()
        .run_system_once(apply_pending_craters)
        .unwrap();
    let session = app.world().resource::<SparseHeightSessionCrater>();
    let height = feathrise_map_content::sparse_height::sample_crater_height(
        &spec,
        session.tiles(),
        [256, 256],
        0.0,
    );
    assert!((height + 0.05).abs() < 0.0001, "纯岩0.1响应保留0.05米深度");
    app.world_mut()
        .run_system_once(clear_session_craters)
        .unwrap();
    assert!(
        app.world()
            .resource::<SparseHeightSessionCrater>()
            .tiles()
            .is_empty()
    );
    assert_eq!(
        feathrise_map_content::sparse_height::sample_crater_height(
            &spec,
            app.world().resource::<SparseHeightSessionCrater>().tiles(),
            [256, 256],
            0.0
        ),
        0.0,
        "重进清空会话，作者基础高度仍为原值"
    );
}

#[test]
fn crater_pending_route_rejects_changed_root_map_pose_or_terrain_root() {
    for variant in 0..3 {
        let (mut app, map) = fixture();
        let (owner, _, _) = context(&mut app, &map, &map, Transform::IDENTITY, true);
        for _ in 0..5 {
            app.update();
        }
        cast(&mut app, &map, Vec3::ZERO);
        assert_eq!(
            get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
                .pending
                .len(),
            1
        );
        match variant {
            0 => app
                .world_mut()
                .resource_mut::<ActiveDynamicMap>()
                .select(MapId::parse("MAP-crater-other").unwrap()),
            1 => {
                app.world_mut()
                    .entity_mut(owner.unwrap())
                    .insert(MapSourceFrame::from_transform(Transform::from_xyz(
                        2.0, 0.0, 0.0,
                    )));
            }
            _ => {
                let next = app.world_mut().spawn_empty().id();
                get_mut::<DynamicMapRuntimeState>(app.world_mut(), owner).root = Some(next);
            }
        }
        prepare_and_apply(&mut app, owner);
        assert!(
            get_mut::<SparseHeightSessionCrater>(app.world_mut(), owner)
                .tiles()
                .is_empty(),
            "变更{variant}不能补写旧坑"
        );
        assert!(
            get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
                .pending
                .is_empty()
        );
    }
}

#[test]
fn crater_repeated_nested_source_instances_keep_distinct_pits_through_resident_pause_restore() {
    let (mut app, root) = fixture();
    let source = MapId::parse("MAP-repeated-crater-source").unwrap();
    let (left, spec, _) = context(
        &mut app,
        &root,
        &source,
        Transform::from_xyz(-20.0, 0.0, 0.0),
        true,
    );
    let (right, _, _) = context(
        &mut app,
        &root,
        &source,
        Transform::from_xyz(20.0, 0.0, 0.0),
        true,
    );
    for (owner, suffix) in [(left.unwrap(), "left"), (right.unwrap(), "right")] {
        let terrain = TerrainId::parse("same-source-grid").unwrap();
        app.world_mut()
            .get_mut::<super::super::instances::SubMapInstance>(owner)
            .unwrap()
            .path = MapInstancePath::root(root.clone())
            .child(feathrise_map_content::MapInstanceId::parse("parent").unwrap())
            .child(feathrise_map_content::MapInstanceId::parse(suffix).unwrap())
            .terrain(terrain);
    }
    for _ in 0..5 {
        app.update();
    }
    cast(&mut app, &root, Vec3::new(-20.0, 0.0, 0.0));
    prepare_and_apply(&mut app, left);
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), right)
            .pending
            .is_empty()
    );
    assert!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), right)
            .tiles()
            .is_empty()
    );
    let mut parked = feathrise_scene_core::residency::take_resident_resources(app.world_mut());
    assert!(
        app.world()
            .resource::<InstanceResourceTable<SparseHeightSessionCrater>>()
            .values
            .is_empty()
    );
    feathrise_scene_core::residency::restore_resident_resources(app.world_mut(), &mut parked);
    assert_eq!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), left)
            .changed_footprints_since(0)
            .count(),
        1
    );
    assert!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), right)
            .tiles()
            .is_empty()
    );
    let left_height = feathrise_map_content::sparse_height::sample_crater_height(
        &spec,
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), left).tiles(),
        [256, 256],
        0.0,
    );
    assert!((left_height + 0.5).abs() < 0.001);
    // 即使请求误用同一源MapId，也不能绕过玩法根身份写入任一重复实例。
    cast(&mut app, &source, Vec3::new(20.0, 0.0, 0.0));
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), right)
            .pending
            .is_empty()
    );
    cast(&mut app, &root, Vec3::new(20.0, 0.0, 0.0));
    prepare_and_apply(&mut app, right);
    assert_eq!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), right)
            .changed_footprints_since(0)
            .count(),
        1
    );
    assert_eq!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), left)
            .changed_footprints_since(0)
            .count(),
        1
    );
}

#[test]
fn crater_declared_missing_surface_waits_without_guessing_and_bare_soil_can_resume() {
    let (mut app, map) = fixture();
    let (owner, _, _) = context(&mut app, &map, &map, Transform::IDENTITY, true);
    prepare_and_apply(&mut app, owner);
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
            .snapshot
            .is_some()
    );
    get_mut::<MapContentRegistry>(app.world_mut(), owner)
        .maps
        .get_mut(&map)
        .unwrap()
        .surface = Some(Handle::default());
    for _ in 0..5 {
        app.update();
    }
    cast(&mut app, &map, Vec3::ZERO);
    prepare_and_apply(&mut app, owner);
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
            .snapshot
            .is_none()
    );
    assert_eq!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
            .pending
            .len(),
        1
    );
    assert!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), owner)
            .tiles()
            .is_empty(),
        "已声明但尚未到达/损坏的地表不能冒充白模默认土壤"
    );
    get_mut::<MapContentRegistry>(app.world_mut(), owner)
        .maps
        .get_mut(&map)
        .unwrap()
        .surface = None;
    prepare_and_apply(&mut app, owner);
    assert!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
            .pending
            .is_empty()
    );
    assert_eq!(
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), owner)
            .changed_footprints_since(0)
            .count(),
        1
    );
}

#[test]
fn crater_visible_author_response_survives_close_and_waits_for_paired_saved_cpu_sources() {
    author_surface_handoff_case(false, false);
}

#[test]
fn crater_saved_source_identity_rejects_unread_old_instance_journal_for_both_raw_arrival_orders() {
    for weights_first in [false, true] {
        author_surface_handoff_case(true, weights_first);
    }
}

#[derive(Resource)]
struct SurfaceTestScope {
    owner: Option<Entity>,
    enabled: bool,
    notices: usize,
}

fn enter_surface_test_scope(world: &mut World) {
    let owner = world.resource::<SurfaceTestScope>().owner;
    enter_scope(world, owner);
}

fn leave_surface_test_scope(world: &mut World) {
    enter_scope(world, None);
}

fn surface_test_enabled(scope: Res<SurfaceTestScope>) -> bool {
    scope.enabled
}

fn record_surface_test_notices(
    mut events: feathrise_scene_core::residency::ResidentAssetEventReader<MapSurfaceSourceAsset>,
    mut scope: ResMut<SurfaceTestScope>,
) {
    scope.notices = events.read().count(); // 独立Reader只证明测试确实包含旧journal，不消费生产Reader的进度。
}

fn author_surface_handoff_case(unread_journal: bool, weights_first: bool) {
    let (mut app, map) = fixture();
    let (owner, spec, _) = context(&mut app, &map, &map, Transform::IDENTITY, unread_journal);
    if unread_journal {
        feathrise_scene_core::residency::install_instance_asset_journal::<MapSurfaceSourceAsset>(&mut app);
    }
    let mut preset = app
        .world()
        .resource::<SurfacePresetRegistry>()
        .resolve("soil")
        .unwrap()
        .clone();
    preset.crater_response = 0.1;
    app.world_mut()
        .resource_mut::<SurfacePresetRegistry>()
        .resolved = ResolvedSurfacePresetCatalog::build(&SurfacePresetCatalogAsset {
        schema_version: SURFACE_PRESET_SCHEMA_VERSION,
        texture_resolution: [1; 2],
        base_color_array_path: "config/surface/base.ktx2".into(),
        normal_array_path: "config/surface/normal.ktx2".into(),
        arm_height_array_path: "config/surface/arm.ktx2".into(),
        presets: vec![preset],
    })
    .unwrap();
    app.world_mut()
        .resource_mut::<SurfacePresetRegistry>()
        .revision = 1;
    let definition = app
        .world_mut()
        .resource_mut::<Assets<MapSurfaceAsset>>()
        .add(MapSurfaceAsset {
            schema_version: feathrise_map_content::MAP_SURFACE_SCHEMA_VERSION,
            map_id: map.clone(),
            resolution: [1; 2],
            layers: vec![feathrise_map_content::MapSurfaceLayer {
                id: "hard".into(),
                label: "硬地".into(),
                preset_id: "soil".into(),
            }],
            index_source_path: "maps/crater-memory/indices.rgba32".into(),
            weight_source_path: "maps/crater-memory/weights.rgba8".into(),
            index_texture_path: "maps/crater-memory/indices.ktx2".into(),
            weight_texture_path: "maps/crater-memory/weights.ktx2".into(),
        });
    get_mut::<MapContentRegistry>(app.world_mut(), owner)
        .maps
        .get_mut(&map)
        .unwrap()
        .surface = Some(definition.clone());
    let indices: Handle<MapSurfaceSourceAsset> = app
        .world()
        .resource::<AssetServer>()
        .load("maps/crater-memory/indices.rgba32");
    let weights: Handle<MapSurfaceSourceAsset> = app
        .world()
        .resource::<AssetServer>()
        .load("maps/crater-memory/weights.rgba8");
    app.world_mut()
        .resource_mut::<Assets<MapSurfaceSourceAsset>>()
        .insert(
            indices.id(),
            MapSurfaceSourceAsset::from_bytes(Arc::from([
                1_u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ])),
        )
        .unwrap();
    app.world_mut()
        .resource_mut::<Assets<MapSurfaceSourceAsset>>()
        .insert(
            weights.id(),
            MapSurfaceSourceAsset::from_bytes(Arc::from([0_u8; 4])),
        )
        .unwrap();
    app.insert_resource(SurfaceTestScope {
        owner,
        enabled: !unread_journal,
        notices: 0,
    });
    app.add_systems(
        Update,
        (
            enter_surface_test_scope,
            record_surface_test_notices,
            prepare_crater_surface,
            apply_pending_craters,
            leave_surface_test_scope,
        )
            .chain()
            .run_if(surface_test_enabled)
            .after(receive_crater_requests),
    );
    if unread_journal {
        prepare_and_apply(&mut app, owner); // 旧CPU响应已建立，但即将轮转的持久Reader还未读过journal。
        app.world_mut()
            .write_message(AssetEvent::<MapSurfaceSourceAsset>::Modified { id: indices.id() });
        app.world_mut()
            .write_message(AssetEvent::<MapSurfaceSourceAsset>::Modified { id: weights.id() });
    }
    for _ in 0..5 {
        app.update();
    } // 子实例版本故意暂停prepare：旧Modified进入真实journal并超过原两帧通道，不能靠排空旧事件过测。
    assert_eq!(
        get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
            .snapshot
            .as_ref()
            .unwrap()
            .sample(Vec2::ZERO),
        1.0
    );
    let author_response = Arc::new(
        SurfaceCraterResponse::from_author_buffers(
            Vec2::splat(-32.0),
            Vec2::splat(32.0),
            [1; 2],
            Arc::new(vec![0; 16]),
            Arc::new(vec![255, 0, 0, 0]),
            vec![0.1],
        )
        .unwrap(),
    );
    get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner)
        .publish_for_terrain_with_crater_response(
            map.clone(),
            None,
            3,
            Handle::default(),
            Some(author_response),
        ); // 与F12关闭后保留的同版完整发布值相同。
    let expected = Arc::new(SurfaceSourceSaveIdentity::for_author_buffers(
        app.world()
            .resource::<Assets<MapSurfaceAsset>>()
            .get(&definition)
            .unwrap(),
        &[0_u8; 16],
        &[255_u8, 0, 0, 0],
    ));
    {
        let mut author = get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner);
        author.begin_saved_handoff(&map, 3, false, true, Some(expected));
        author.pending_images = 0; // KTX2先就绪，CPU门仍须按本批实际内容而非旧journal证明。
    }
    app.world_mut().resource_mut::<SurfaceTestScope>().enabled = true;
    app.update(); // 第一次轮转同时看见新save_ticket与未读旧Modified，必须保持等待。
    if unread_journal {
        assert!(
            app.world().resource::<SurfaceTestScope>().notices >= 2,
            "旧Modified必须由真实实例journal保留，不能排空后绕过回归"
        );
    }
    assert_ne!(
        get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner).pending_sources,
        0
    );
    cast(&mut app, &map, Vec3::ZERO);
    let height = feathrise_map_content::sparse_height::sample_crater_height(
        &spec,
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), owner).tiles(),
        [256, 256],
        0.0,
    );
    assert!(
        (height + 0.05).abs() < 0.001,
        "可见未保存硬地必须挖出0.1响应的真坑，而非永远pending或采用旧裸地"
    );
    let (first, first_bytes, second, second_bytes): (_, Arc<[u8]>, _, Arc<[u8]>) = if weights_first
    {
        (
            weights.id(),
            Arc::from([255_u8, 0, 0, 0]),
            indices.id(),
            Arc::from([0_u8; 16]),
        )
    } else {
        (
            indices.id(),
            Arc::from([0_u8; 16]),
            weights.id(),
            Arc::from([255_u8, 0, 0, 0]),
        )
    };
    app.world_mut()
        .resource_mut::<Assets<MapSurfaceSourceAsset>>()
        .insert(first, MapSurfaceSourceAsset::from_bytes(first_bytes))
        .unwrap();
    for _ in 0..2 {
        app.update();
    }
    assert_ne!(
        get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner).pending_sources,
        0,
        "只有一份新raw不能借旧journal的另一份Modified交还文件权威"
    );
    assert!(
        (get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner)
            .crater_response
            .as_ref()
            .unwrap()
            .sample(Vec2::ZERO)
            - 0.1)
            .abs()
            < 0.001
    );
    app.world_mut()
        .resource_mut::<Assets<MapSurfaceSourceAsset>>()
        .insert(second, MapSurfaceSourceAsset::from_bytes(second_bytes))
        .unwrap();
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(
        get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner).pending_sources,
        0,
        "两份实际raw摘要与本批保存完全匹配才允许正式GPU接管"
    );
    assert!(
        (get_mut::<CraterSurfaceSource>(app.world_mut(), owner)
            .snapshot
            .as_ref()
            .unwrap()
            .sample(Vec2::ZERO)
            - 0.1)
            .abs()
            < 0.001
    );
    if unread_journal {
        app.world_mut()
            .resource_mut::<Assets<MapSurfaceSourceAsset>>()
            .insert(
                weights.id(),
                MapSurfaceSourceAsset::from_bytes(Arc::from([0_u8; 4])),
            )
            .unwrap();
        for _ in 0..2 {
            app.update();
        }
        assert_ne!(
            get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner).pending_sources,
            0,
            "已匹配过的源若在正式GPU采用前回退旧内容，必须重新关门，不能永久锁存零掩码"
        );
        app.world_mut()
            .resource_mut::<Assets<MapSurfaceSourceAsset>>()
            .insert(
                weights.id(),
                MapSurfaceSourceAsset::from_bytes(Arc::from([255_u8, 0, 0, 0])),
            )
            .unwrap();
        for _ in 0..2 {
            app.update();
        }
        assert_eq!(
            get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner).pending_sources,
            0
        );
    }
    get_mut::<DynamicSurfaceAuthorPreview>(app.world_mut(), owner).clear(); // 正式同代材质采用，之后只读同版文件CPU响应。
    cast(&mut app, &map, Vec3::new(6.0, 0.0, 0.0));
    let height = feathrise_map_content::sparse_height::sample_crater_height(
        &spec,
        get_mut::<SparseHeightSessionCrater>(app.world_mut(), owner).tiles(),
        [304, 256],
        0.0,
    );
    assert!(
        (height + 0.05).abs() < 0.001,
        "保存重载交还后不能闪回旧磁盘系数1"
    );
}
