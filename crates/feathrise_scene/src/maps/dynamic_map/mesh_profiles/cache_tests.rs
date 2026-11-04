//! 多网格共享租约与同ID换代回收；真实GLTF加载由实例调度集成回归覆盖。

use super::*;
use bevy::{asset::AssetPlugin, camera::visibility::VisibilityRange};
use feathrise_environment::vegetation::{
    VegetationMeshLod, VegetationMeshLodLevel, VegetationMeshPart, VegetationMeshPrototype,
};
use super::super::generic::BuiltGenericMeshProfile;
use super::super::{resources::{DynamicMeshProfilePhase, DynamicMeshProfileRequests,
    DynamicMeshProfileState, DynamicMeshProfileStatus}, systems};
use bevy::{diagnostic::FrameCount, ecs::system::RunSystemOnce};
use feathrise_map_content::{ResolvedVegetationPresetCatalog, VegetationPresetCatalogAsset};
use feathrise_scene_core::residency::{initialize_instance_context, InstanceResourceTable, MapInstanceContextAppExt,
    MapRuntimeScope};

fn key(revision: u64) -> DynamicMeshProfileKey {
    DynamicMeshProfileKey { catalog_revision: revision, preset_id: "shared-tree".to_owned() }
}

fn install(cache: &mut DynamicMeshProfileCache, registry: &mut VegetationMeshPrototypeRegistry,
    key: DynamicMeshProfileKey, mesh: Handle<Mesh>, material: Handle<StandardMaterial>) -> u64 {
    let id = cache.entries[&key].prototype_id;
    let prototype = VegetationMeshPrototype { id, lods: vec![VegetationMeshLod {
        level: VegetationMeshLodLevel(0), visibility_range: VisibilityRange::default(),
        parts: vec![VegetationMeshPart { part_id: 0, mesh: mesh.clone(), material: material.clone().into(),
            local_transform: Transform::IDENTITY, culling_aabb: None, casts_shadow: true,
            billboard: None, wind: None, batch_vertex_mode: Default::default() }],
    }] };
    registry.register(prototype.clone());
    let revision = registry.revision(id).unwrap();
    cache.entries.get_mut(&key).unwrap().loaded = Some((revision, BuiltGenericMeshProfile {
        prototype, wind: vec![], _billboards: vec![], _generated_meshes: vec![mesh],
        _generated_materials: vec![material],
    }));
    cache.registered.insert(id, (key, revision));
    revision
}

#[test]
fn shared_mesh_profile_sibling_lease_keeps_the_only_registered_prototype() {
    let mut cache = DynamicMeshProfileCache::default();
    let mut registry = VegetationMeshPrototypeRegistry::default();
    let id = VegetationPrototypeId(9);
    let first = cache.lease(key(1), id);
    let sibling = cache.lease(key(1), id);
    let revision = install(&mut cache, &mut registry, key(1), Handle::default(), Handle::default());
    assert_eq!(cache.entries.len(), 1);
    assert_eq!(Arc::strong_count(&cache.entries[&key(1)].token), 3);
    drop(first);
    cache.collect_one(&mut registry);
    assert_eq!(registry.revision(id), Some(revision));
    assert!(cache.entries[&key(1)].ready(&registry));
    drop(sibling);
    cache.collect_one(&mut registry);
    assert!(cache.entries.is_empty());
    assert!(!registry.contains(id), "只有最后一个网格租约退出才可移除公共原型");
}

#[test]
fn shared_mesh_profile_old_catalog_and_external_replacement_cannot_be_deleted_by_old_owner() {
    let mut cache = DynamicMeshProfileCache::default();
    let mut registry = VegetationMeshPrototypeRegistry::default();
    let id = VegetationPrototypeId(9);
    let old = cache.lease(key(1), id);
    install(&mut cache, &mut registry, key(1), Handle::default(), Handle::default());
    let current = cache.lease(key(2), id);
    let revision = install(&mut cache, &mut registry, key(2), Handle::default(), Handle::default());
    assert!(!cache.entries[&key(1)].ready(&registry));
    assert!(cache.entries[&key(2)].ready(&registry));
    drop(old);
    for _ in 0..3 { cache.collect_one(&mut registry); }
    assert_eq!(registry.revision(id), Some(revision), "旧目录租约释放不能删除新目录同ID原型");
    registry.register(VegetationMeshPrototype { id, lods: vec![] });
    let external_revision = registry.revision(id).unwrap();
    assert!(!cache.entries[&key(2)].ready(&registry), "同ID替换不能冒用旧登记版本的ready信用");
    cache.entries.get_mut(&key(2)).unwrap().borrowed_revision = Some(external_revision);
    assert!(cache.entries[&key(2)].ready(&registry), "借用的新版本精确匹配后不应再被旧保活版本挡住");
    drop(current);
    for _ in 0..3 { cache.collect_one(&mut registry); }
    assert!(cache.entries.is_empty());
    assert_eq!(registry.revision(id), Some(external_revision), "外部正式所有者的替换版本不归缓存删除");
}

#[test]
fn exact_profile_readiness_waits_for_new_catalog_registration_and_rejects_unacknowledged_replacement() {
    let source: VegetationPresetCatalogAsset = serde_json::from_str(crate::test_fixtures::VEGETATION_CATALOG_JSON).unwrap();
    let mut preset = source.presets.iter().find(|preset| preset.resolved_prototype_id().is_some()).unwrap().clone();
    preset.id = "shared-tree".into();
    preset.aliases = vec!["old-project-tree".into()];
    preset.prototype_id = Some(9);
    let mut presets = VegetationPresetRegistry { catalog: Handle::default(), revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&VegetationPresetCatalogAsset { schema_version: source.schema_version, presets: vec![preset] }).unwrap(), last_error: None };
    let mut cache = DynamicMeshProfileCache::default();
    let mut registry = VegetationMeshPrototypeRegistry::default();
    let id = VegetationPrototypeId(9);
    let _old_lease = cache.lease(key(1), id);
    let old_revision = install(&mut cache, &mut registry, key(1), Handle::default(), Handle::default());
    assert_eq!(cache.ready_revision("old-project-tree", id, &presets, &registry), Some(old_revision));
    assert_eq!(cache.ready_revision("old-project-tree", VegetationPrototypeId(999), &presets, &registry), None);
    presets.revision = 2;
    let _new_lease = cache.lease(key(2), id);
    assert!(registry.get(id).is_some(), "旧完整原型继续给可见结果保活");
    assert_eq!(cache.ready_revision("shared-tree", id, &presets, &registry), None, "新目录不能借旧 ID 的现存原型");
    let new_revision = install(&mut cache, &mut registry, key(2), Handle::default(), Handle::default());
    assert_ne!(old_revision, new_revision);
    assert_eq!(cache.ready_revision("shared-tree", id, &presets, &registry), Some(new_revision));
    registry.register(VegetationMeshPrototype { id, lods: vec![] });
    assert_eq!(cache.ready_revision("shared-tree", id, &presets, &registry), None, "同目录的外部替换也须重新取得该登记版本信用");
}

#[test]
fn shared_mesh_profile_cache_reclamation_keeps_real_render_handles_until_their_last_drop() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>().init_asset::<StandardMaterial>();
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(Mesh::from(Cuboid::default()));
    let material = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial::default());
    let (mesh_id, material_id) = (mesh.id(), material.id());
    let render = app.world_mut().spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()))).id();
    let mut cache = DynamicMeshProfileCache::default();
    let mut registry = VegetationMeshPrototypeRegistry::default();
    let lease = cache.lease(key(1), VegetationPrototypeId(9));
    install(&mut cache, &mut registry, key(1), mesh, material);
    drop(lease);
    cache.collect_one(&mut registry);
    app.update();
    assert!(registry.is_empty());
    assert!(app.world().resource::<Assets<Mesh>>().contains(mesh_id));
    assert!(app.world().resource::<Assets<StandardMaterial>>().contains(material_id),
        "热更旧完整渲染根仍在显示时，不能用Assets::remove强制抹去材质");
    app.world_mut().entity_mut(render).despawn();
    for _ in 0..3 { app.update(); }
    assert!(!app.world().resource::<Assets<Mesh>>().contains(mesh_id));
    assert!(!app.world().resource::<Assets<StandardMaterial>>().contains(material_id));
}

#[test]
fn shared_mesh_profile_reclamation_checks_only_one_cache_record_per_frame() {
    let mut cache = DynamicMeshProfileCache::default();
    let mut registry = VegetationMeshPrototypeRegistry::default();
    for revision in 1..=3 {
        let lease = cache.lease(key(revision), VegetationPrototypeId(revision));
        install(&mut cache, &mut registry, key(revision), Handle::default(), Handle::default());
        drop(lease);
    }
    cache.collect_one(&mut registry);
    assert_eq!(cache.entries.len(), 2, "换图不能在主帧一次回收整个预设集合");
    cache.collect_one(&mut registry);
    assert_eq!(cache.entries.len(), 1);
    cache.collect_one(&mut registry);
    assert!(cache.entries.is_empty());
    assert!(registry.is_empty());
}

fn drive_fixture() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>().init_asset::<StandardMaterial>().init_asset::<Image>()
        .init_asset::<TreeWindMaterial>().init_asset::<VegetationMeshBillboardMaterial>()
        .init_resource::<DynamicMeshProfileCache>().init_resource::<VegetationMeshPrototypeRegistry>()
        .init_instance_resource::<DynamicMeshProfileRequests>()
        .init_instance_resource::<DynamicMeshProfileState>()
        .init_instance_resource::<DynamicMeshProfileStatus>()
        .insert_resource(TreeWindNoiseTexture(Handle::default()));
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(
        crate::test_fixtures::VEGETATION_CATALOG_JSON).unwrap();
    app.insert_resource(VegetationPresetRegistry { catalog: Handle::default(), revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(), last_error: None });
    app.finish();
    app.cleanup();
    app
}

fn request_scope(world: &mut World) -> Entity {
    let scope = world.spawn_empty().id();
    initialize_instance_context(world, scope);
    world.resource_mut::<InstanceResourceTable<DynamicMeshProfileRequests>>()
        .values.get_mut(&scope).unwrap().request("hn01_tree").unwrap();
    scope
}

fn drive_scope(world: &mut World, scope: Entity) {
    world.resource_mut::<MapRuntimeScope>().instance = Some(scope);
    world.run_system_once(systems::drive_dynamic_mesh_profiles).unwrap();
    *world.resource_mut::<MapRuntimeScope>() = MapRuntimeScope::default();
}

#[test]
fn shared_mesh_profile_sibling_retry_recovers_failed_scope_without_reopening_stable_failed_io() {
    let mut app = drive_fixture();
    let first = request_scope(app.world_mut());
    let sibling = request_scope(app.world_mut());
    drive_scope(app.world_mut(), first);
    drive_scope(app.world_mut(), sibling);
    let key = DynamicMeshProfileKey { catalog_revision: 1, preset_id: "hn01_tree".to_owned() };
    app.world_mut().resource_mut::<DynamicMeshProfileCache>().entries.get_mut(&key).unwrap()
        .failure = Some("已完成的真实加载失败回执".to_owned());
    for frame in 1..=3 {
        app.world_mut().resource_mut::<FrameCount>().0 = frame;
        drive_scope(app.world_mut(), first);
        drive_scope(app.world_mut(), sibling);
        assert!(app.world().resource::<DynamicMeshProfileCache>().loading_profile().is_none());
        assert!(app.world().resource::<DynamicMeshProfileCache>().entries[&key].failure.is_some());
        assert_eq!(app.world().resource::<InstanceResourceTable<DynamicMeshProfileStatus>>()
            .values[&first].phase, DynamicMeshProfilePhase::Failed);
    }
    let first_request_revision = app.world().resource::<InstanceResourceTable<DynamicMeshProfileRequests>>()
        .values[&first].revision();
    app.world_mut().resource_mut::<InstanceResourceTable<DynamicMeshProfileRequests>>()
        .values.get_mut(&sibling).unwrap().request("hn01_tree").unwrap();
    drive_scope(app.world_mut(), sibling);
    assert!(app.world().resource::<DynamicMeshProfileCache>().entries[&key].failure.is_none());
    let mut cache = app.world_mut().remove_resource::<DynamicMeshProfileCache>().unwrap();
    install(&mut cache, &mut app.world_mut().resource_mut::<VegetationMeshPrototypeRegistry>(),
        key, Handle::default(), Handle::default()); // 成功装配归cache；真实GLTF由独立网格集成测试验证。
    app.insert_resource(cache);
    drive_scope(app.world_mut(), first);
    let world = app.world();
    assert_eq!(world.resource::<InstanceResourceTable<DynamicMeshProfileRequests>>()
        .values[&first].revision(), first_request_revision);
    assert!(world.resource::<InstanceResourceTable<DynamicMeshProfileState>>().values[&first]
        .ready_for("hn01_tree", world.resource::<VegetationPresetRegistry>(),
            world.resource::<VegetationMeshPrototypeRegistry>()));
}

#[test]
fn shared_mesh_profile_menu_reaper_finishes_real_abandoned_asset_request_without_new_drive() {
    let mut app = drive_fixture();
    app.insert_resource(State::new(feathrise_scene_core::state::ScenePhase::Idle))
        .add_systems(PostUpdate, systems::collect_dynamic_mesh_profiles);
    let preset = app.world().resource::<VegetationPresetRegistry>().resolve("hn01_tree").unwrap();
    let key = DynamicMeshProfileKey { catalog_revision: 1, preset_id: preset.id.clone() };
    let id = VegetationPrototypeId(preset.resolved_prototype_id().unwrap());
    // 不安装GLTF loader，真实AssetServer会明确失败；离场后只有收尾器，绝不再次调用drive。
    let load = request_generic_mesh_profile(app.world().resource::<AssetServer>(), preset, 1).unwrap();
    let mut cache = app.world_mut().resource_mut::<DynamicMeshProfileCache>();
    let lease = cache.lease(key, id);
    cache.current_generic = Some(load);
    drop(lease);
    drop(cache);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.world().resource::<DynamicMeshProfileCache>().loading_profile().is_some() {
        app.update();
        assert!(std::time::Instant::now() < deadline, "菜单必须结清已无租约的真实加载槽");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(app.world().resource::<DynamicMeshProfileCache>().entries.is_empty());
    assert!(app.world().resource::<VegetationMeshPrototypeRegistry>().is_empty());
}
