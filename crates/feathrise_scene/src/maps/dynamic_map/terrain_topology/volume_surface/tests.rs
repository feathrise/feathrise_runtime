//! 生产接管系统覆盖预览、保存GLB、驻留粗细网格、作者地表换代和Deferred租约。
use super::*;
use crate::shared::placement_data::terrain_topology_test_support::collision_fixture;
use avian3d::prelude::Collider;
use bevy::{
    asset::uuid::Uuid, material::OpaqueRendererMethod, pbr::ExtendedMaterial,
    render::storage::ShaderBuffer,
};
use feathrise_environment::terrain_surface::{
    TerrainSurfaceCarrier, TerrainSurfaceExtension, TerrainSurfaceIndexedDescriptor,
};

fn surface() -> TerrainSurfaceMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(0.4, 0.3, 0.2),
            ..default()
        },
        extension: TerrainSurfaceExtension::new_indexed(TerrainSurfaceIndexedDescriptor {
            world_min: Vec2::new(-32., -64.),
            world_max: Vec2::new(128., 256.),
            weight_map: Handle::Uuid(Uuid::from_u128(1), Default::default()),
            index_map: Handle::Uuid(Uuid::from_u128(2), Default::default()),
            base_color_array: Handle::Uuid(Uuid::from_u128(3), Default::default()),
            normal_array: Handle::Uuid(Uuid::from_u128(4), Default::default()),
            arm_height_array: Handle::Uuid(Uuid::from_u128(5), Default::default()),
            layer_table: Handle::<ShaderBuffer>::Uuid(Uuid::from_u128(6), Default::default()),
            layer_count: 4,
            zero_weight: Handle::default(),
        }),
    }
}

fn setup() -> (App, MapId, Entity, Handle<TerrainSurfaceMaterial>) {
    let mut app = App::new();
    app.init_resource::<Assets<TerrainSurfaceMaterial>>()
        .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .init_resource::<VolumeSurfaceState>()
        .init_resource::<TerrainTopologyRuntime>()
        .init_resource::<TerrainVolumePreviewRequest>()
        .init_resource::<StaticTerrainRaytracingSources>()
        .init_resource::<SparseHeightPageRuntime>()
        .add_message::<AssetEvent<TerrainSurfaceMaterial>>()
        .add_systems(Update, sync_volume_surface);
    let id = MapId("MAP-volume-surface".into());
    let root = app.world_mut().spawn_empty().id();
    app.insert_resource(ActiveDynamicMap {
        map_id: Some(id.clone()),
    })
    .insert_resource(DynamicMapRuntimeState {
        active_map_id: Some(id.clone()),
        root: Some(root),
        sparse_height_path: Some("height".into()),
        ..default()
    });
    let handle = adopt(&mut app, &id, surface());
    (app, id, root, handle)
}

fn adopt(
    app: &mut App,
    id: &MapId,
    material: TerrainSurfaceMaterial,
) -> Handle<TerrainSurfaceMaterial> {
    use crate::maps::dynamic_map::clipmap::{
        ClipmapBounds,
        material::{ClipmapFlatHeightPage, ClipmapHeightExtension},
    };
    let source = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(material.clone());
    let mut visible = material;
    visible.base.opaque_render_method = OpaqueRendererMethod::Deferred;
    visible
        .extension
        .set_dynamic_terrain_shadow_terminator_data(true); // 故意借入高度租约，体积副本必须解除。
    let extension = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::splat(-128.), Vec2::splat(512.)).unwrap(),
        UVec2::splat(512),
        &ClipmapFlatHeightPage {
            heights: Handle::default(),
            page_table: Handle::default(),
        },
    )
    .unwrap();
    let wrapper = app
        .world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .add(ExtendedMaterial {
            base: visible,
            extension,
        });
    let previous: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, With<DynamicSparseClipmapPrimitive>>()
        .iter(app.world())
        .collect();
    for entity in previous {
        app.world_mut().entity_mut(entity).despawn();
    }
    app.world_mut().spawn((
        DynamicSparseClipmapPrimitive,
        MeshMaterial3d(wrapper.clone()),
        SparseClipmapSurfaceLease::new(&source, &wrapper),
    ));
    let root = app
        .world()
        .resource::<DynamicMapRuntimeState>()
        .root
        .unwrap();
    app.world_mut()
        .resource_mut::<SparseHeightPageRuntime>()
        .adopt_volume_surface_for_test(id.clone(), root, source.clone());
    source
}

fn volume(app: &mut App, id: &MapId, sculpted: bool, streamed: bool) -> (Entity, Entity) {
    let record: feathrise_map_content::MapPlacementRecord = serde_json::from_value(serde_json::json!({
        "id": format!("volume-{sculpted}-{streamed}"), "asset_path":"maps/volume/v.glb",
        "translation":[16,0,24],"yaw_degrees":0,"scale":[1,1,1],
        "terrain_patch":{"sculpted_volume":sculpted,"streamed_volume":streamed,
            "volume_cell_size":if streamed {Some(0.25)} else {None},
            "cutout":{"min":[0,0],"max":[8,8]}}
    }))
    .unwrap();
    let mesh: Handle<Mesh> = Handle::default();
    let child = app
        .world_mut()
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(Handle::<StandardMaterial>::default()),
        ))
        .id();
    let root = app
        .world_mut()
        .spawn((
            crate::MapPlacementInstance::new(id, &record),
            collision_fixture(1, Collider::cuboid(1., 1., 1.), Arc::from([(child, mesh)])),
            Visibility::Hidden,
        ))
        .id();
    app.world_mut()
        .resource_mut::<TerrainTopologyRuntime>()
        .active_roots
        .push(root);
    app.world_mut()
        .resource_mut::<StaticTerrainRaytracingSources>()
        .batches
        .push(Arc::from([(child, Handle::default())]));
    (root, child)
}

fn bound(app: &App, entity: Entity) -> Handle<TerrainSurfaceMaterial> {
    app.world()
        .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(entity)
        .unwrap()
        .0
        .clone()
}

#[test]
fn large_volume_materials_bind_in_batches_and_grant_credit_only_when_complete() {
    let (mut app, id, _, _) = setup();
    let (root, first) = volume(&mut app, &id, true, true);
    let mut parts = vec![(first, Handle::default())];
    for _ in 1..65 {
        let entity = app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                MeshMaterial3d(Handle::<StandardMaterial>::default()),
            ))
            .id();
        parts.push((entity, Handle::default()));
    }
    {
        let mut ready = app
            .world_mut()
            .get_mut::<TerrainPatchColliderReady>(root)
            .unwrap();
        ready.meshes = parts.clone().into();
        ready.surface_ready = false;
    }
    for expected in [32, 64, 65] {
        app.update();
        let bound = parts
            .iter()
            .filter(|(e, _)| app.world().get::<TerrainVolumeSurface>(*e).is_some())
            .count();
        assert_eq!(bound, expected);
        let ready = app.world().get::<TerrainPatchColliderReady>(root).unwrap();
        let request = app
            .world()
            .get::<crate::TerrainPatchCollisionRequest>(root)
            .unwrap();
        assert_eq!(ready.matches(request), expected == 65);
    }
    app.update();
    assert_eq!(
        app.world().resource::<VolumeSurfaceState>().bindings[&root].2,
        65
    );
}

#[test]
fn preview_streamed_and_saved_volume_reuse_complete_adopted_pbr_without_height_or_loading_credit() {
    let (mut app, id, _, _) = setup();
    let (_, resident) = volume(&mut app, &id, true, true); // 同一聚合网格承载近处精细和远处简化面。
    let (saved_root, saved) = volume(&mut app, &id, true, false); // 旧v1/v2的正式GLB同样接管。
    let (_, ordinary) = volume(&mut app, &id, false, false);
    let (preview_root, preview) = volume(&mut app, &id, true, true);
    app.world_mut()
        .resource_mut::<TerrainTopologyRuntime>()
        .active_roots
        .retain(|e| *e != preview_root);
    let record = serde_json::from_value(serde_json::json!({"id":"preview","asset_path":"v.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1],"terrain_patch":{"sculpted_volume":true}})).unwrap();
    app.world_mut()
        .resource_mut::<TerrainVolumePreviewRequest>()
        .target = Some(TerrainVolumePreviewTarget {
        map_id: id,
        entity: preview_root,
        record,
        world_cutout: MapTerrainCutout {
            min: [0., 0.],
            max: [8., 8.],
        },
        removal: false,
    });
    app.update();
    let handle = bound(&app, resident);
    assert_eq!(bound(&app, saved), handle);
    assert_eq!(bound(&app, preview), handle);
    let material = app
        .world()
        .resource::<Assets<TerrainSurfaceMaterial>>()
        .get(&handle)
        .unwrap();
    let mut expected = surface();
    expected.extension.set_volume_projection(true);
    assert_eq!(material.extension, expected.extension); // 图、表、世界范围、层数及PBR均保留，未复制高度裁切。
    assert_eq!(material.base.base_color, expected.base.base_color);
    assert_eq!(
        material.base.opaque_render_method,
        expected.base.opaque_render_method
    );
    assert!(
        !material
            .extension
            .dynamic_terrain_shadow_terminator_data_enabled()
    );
    assert!(app.world().get::<TerrainVolumeSurface>(resident).is_some());
    assert!(app.world().get::<TerrainSurfaceCarrier>(resident).is_none()); // 隐藏候选不污染整图唯一预热证据。
    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(resident)
            .is_none()
    );
    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(ordinary)
            .is_some()
    );
    assert_eq!(
        app.world().get::<Visibility>(saved_root),
        Some(&Visibility::Hidden)
    );
    let count = app
        .world()
        .resource::<Assets<TerrainSurfaceMaterial>>()
        .len();
    let revision = app
        .world()
        .resource::<StaticTerrainRaytracingSources>()
        .revision;
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        app.world()
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .len(),
        count
    );
    assert_eq!(
        app.world()
            .resource::<StaticTerrainRaytracingSources>()
            .revision,
        revision
    );
}

#[test]
fn material_candidate_waits_for_adoption_and_paint_undo_replaces_exact_deferred_lease() {
    let (mut app, id, _, original) = setup();
    let (_, entity) = volume(&mut app, &id, true, true);
    app.update();
    let source = bound(&app, entity);
    let mut painted = surface();
    painted
        .extension
        .set_puddle_mask(Some(Handle::Uuid(Uuid::from_u128(7), Default::default())));
    // 只创建作者候选，当前显示租约仍指向旧源，不能提前用新图。
    let _candidate = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(painted.clone());
    app.update();
    assert_eq!(bound(&app, entity), source);
    let mut deferred = app
        .world()
        .resource::<Assets<TerrainSurfaceMaterial>>()
        .get(&source)
        .unwrap()
        .clone();
    deferred.base.opaque_render_method = OpaqueRendererMethod::Deferred;
    let deferred = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(deferred);
    app.world_mut().entity_mut(entity).insert((
        MeshMaterial3d(deferred.clone()),
        TerrainSurfaceMaterialOverride::new(&source, &deferred),
    ));
    app.update();
    assert_eq!(bound(&app, entity), deferred); // 稳定帧保留合法租约。
    adopt(&mut app, &id, painted.clone());
    app.update();
    let new = bound(&app, entity);
    assert_ne!(new, source);
    assert_ne!(new, deferred);
    assert!(
        app.world()
            .get::<TerrainSurfaceMaterialOverride>(entity)
            .is_none()
    );
    let mut expected = painted.extension;
    expected.set_volume_projection(true);
    assert_eq!(
        app.world()
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .get(&new)
            .unwrap()
            .extension,
        expected
    );
    // 撤销采用旧源快照后，洞面也要恢复旧图，不能只更新Clipmap。
    let undo = app
        .world()
        .resource::<Assets<TerrainSurfaceMaterial>>()
        .get(&original)
        .unwrap()
        .clone();
    adopt(&mut app, &id, undo);
    app.update();
    let mut expected = surface().extension;
    expected.set_volume_projection(true);
    assert_eq!(
        app.world()
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .get(&bound(&app, entity))
            .unwrap()
            .extension,
        expected
    );
}

#[test]
fn missing_surface_preserves_last_good_material_and_map_switch_rejects_old_source() {
    let (mut app, id, root, _) = setup();
    let (_, entity) = volume(&mut app, &id, true, true);
    app.update();
    let original = bound(&app, entity);
    let display = app
        .world_mut()
        .query_filtered::<Entity, With<DynamicSparseClipmapPrimitive>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .entity_mut(display)
        .remove::<SparseClipmapSurfaceLease>();
    app.update();
    assert_eq!(bound(&app, entity), original);
    let next = MapId("MAP-next".into());
    app.world_mut().resource_mut::<ActiveDynamicMap>().map_id = Some(next.clone());
    app.world_mut()
        .resource_mut::<DynamicMapRuntimeState>()
        .active_map_id = Some(next.clone());
    let (_, next_entity) = volume(&mut app, &next, true, true);
    app.update();
    assert!(
        app.world()
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(next_entity)
            .is_none()
    );
    assert!(
        app.world()
            .resource::<VolumeSurfaceState>()
            .material
            .is_none()
    );
    assert_eq!(
        app.world().resource::<VolumeSurfaceState>().scope,
        Some((next.clone(), root))
    );
    adopt(&mut app, &next, surface());
    app.update();
    assert_ne!(bound(&app, next_entity), original);
    assert_eq!(bound(&app, entity), original); // 旧地图根不能被新图涂绘更新。
}

#[test]
fn in_place_author_reload_rebinds_once_and_stale_mesh_proof_does_not_overwrite_replacement() {
    let (mut app, id, _, source) = setup();
    let (_, entity) = volume(&mut app, &id, true, true);
    app.update();
    let previous = bound(&app, entity);
    let color = Color::srgb(0.7, 0.2, 0.1);
    app.world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .get_mut(&source)
        .unwrap()
        .base
        .base_color = color;
    let display = app.world_mut().query_filtered::<&MeshMaterial3d<SparseClipmapSurfaceMaterial>, With<DynamicSparseClipmapPrimitive>>().single(app.world()).unwrap().0.clone();
    app.world_mut()
        .resource_mut::<Assets<SparseClipmapSurfaceMaterial>>()
        .get_mut(&display)
        .unwrap()
        .base
        .base
        .base_color = color;
    app.world_mut()
        .write_message(AssetEvent::<TerrainSurfaceMaterial>::Modified { id: source.id() });
    app.update();
    let current = bound(&app, entity);
    assert_ne!(current, previous);
    assert_eq!(
        app.world()
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .get(&current)
            .unwrap()
            .base
            .base_color,
        color
    );
    app.update();
    assert_eq!(bound(&app, entity), current);
    let foreign: Handle<Mesh> = bevy::asset::uuid_handle!("64000000-0000-0000-0000-000000000001");
    app.world_mut().entity_mut(entity).insert((
        Mesh3d(foreign),
        MeshMaterial3d(Handle::<StandardMaterial>::default()),
    ));
    app.world_mut()
        .entity_mut(entity)
        .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>();
    app.update();
    assert!(
        app.world()
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(entity)
            .is_none()
    );
}

#[test]
fn reentering_same_map_does_not_borrow_previous_scene_root_material() {
    let (mut app, id, _, _) = setup();
    let (_, old) = volume(&mut app, &id, true, true);
    app.update();
    let old_material = bound(&app, old);
    let new_root = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<DynamicMapRuntimeState>()
        .root = Some(new_root);
    app.world_mut()
        .resource_mut::<TerrainTopologyRuntime>()
        .active_roots
        .clear();
    let (_, new) = volume(&mut app, &id, true, true);
    app.update(); // 同名地图，但页源和显示租约仍属于前一场景根。
    assert!(
        app.world()
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(new)
            .is_none()
    );
    assert!(
        app.world()
            .resource::<VolumeSurfaceState>()
            .material
            .is_none()
    );
    adopt(&mut app, &id, surface());
    app.update();
    assert_ne!(bound(&app, new), old_material);
}

#[test]
fn volume_author_fields_keep_their_buffer_when_base_pbr_is_replaced() {
    let (mut app, id, _, _) = setup();
    app.init_resource::<Assets<ShaderBuffer>>();
    let (root, entity) = volume(&mut app, &id, true, true);
    let mut fields = feathrise_map_terrain::volume::sparse::paged::VolumeFieldSnapshot::default();
    fields.cell_size = 0.25;
    let buffer =
        feathrise_environment::terrain_surface::build_volume_field_buffer(0.25, std::iter::empty())
            .unwrap();
    app.world_mut()
        .entity_mut(root)
        .insert(crate::VolumeSurfaceFields(
            Arc::new(fields),
            Arc::new(std::sync::Mutex::new(Some(buffer))),
        ));
    app.update();
    let previous = bound(&app, entity);
    let field_buffer = app
        .world()
        .resource::<VolumeSurfaceState>()
        .fields
        .get(&root)
        .unwrap()
        .1
        .id();
    let mut next = surface();
    next.base.base_color = Color::srgb(0.2, 0.6, 0.8);
    next.base.opaque_render_method = OpaqueRendererMethod::Deferred;
    adopt(&mut app, &id, next.clone()); // 仅基础PBR换代，扩展与作者字段不变。
    app.update();
    let current = bound(&app, entity);
    assert_ne!(current, previous, "带三维字段的材质也必须跟随基础PBR换代");
    let material = app
        .world()
        .resource::<Assets<TerrainSurfaceMaterial>>()
        .get(&current)
        .unwrap();
    assert_eq!(material.base.base_color, next.base.base_color);
    assert_eq!(
        material.base.opaque_render_method,
        OpaqueRendererMethod::Deferred
    );
    assert_eq!(
        app.world()
            .resource::<VolumeSurfaceState>()
            .fields
            .get(&root)
            .unwrap()
            .1
            .id(),
        field_buffer
    );
    app.update();
    assert_eq!(bound(&app, entity), current, "稳定帧不能重复登记材质");
    let next_root = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<DynamicMapRuntimeState>()
        .root = Some(next_root);
    adopt(&mut app, &id, next);
    app.update();
    assert_eq!(
        app.world().resource::<VolumeSurfaceState>().fields[&root]
            .1
            .id(),
        field_buffer,
        "同图保存更换场景根不能丢掉已移交GPU的字段缓冲"
    );
    assert!(
        app.world()
            .get::<TerrainPatchColliderReady>(root)
            .unwrap()
            .surface_ready
    );
}
