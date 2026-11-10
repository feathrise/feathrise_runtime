//! 射线保留真实世界距离；绑定查询在Runtime暂时移出时仍按本批次隔离，并拒绝陈旧地址。
use super::terrain_queries::TerrainObjectRaySnapshot;
use bevy::prelude::*;
use avian3d::prelude::Collider;
use feathrise_scene_core::residency::SceneSourceFrame;
#[test]
fn scaled_mesh_ray_keeps_world_distance_and_source_yaw() {
    let pose = Transform::from_translation(Vec3::new(10.,7.,-4.))
        .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).with_scale(Vec3::new(2.,3.,4.));
    let query = TerrainObjectRaySnapshot::Mesh(Collider::cuboid(2.,2.,2.), SceneSourceFrame::from_transform(pose));
    let point = query.ray(Vec3::new(10.,30.,-4.), Vec3::NEG_Y).unwrap();
    assert!((point - Vec3::new(10.,10.,-4.)).length() < 0.001);
    let side = query.ray(Vec3::new(30.,7.,-4.), Vec3::NEG_X).unwrap();
    assert!((side.x - 14.).abs() < 0.001);
}

#[test]
fn object_surface_snap_preserves_layers_under_translation_rotation_and_nonuniform_scale() {
    use super::{clipmap::{ClipmapGroundHit, test_ground_snapshot}, terrain_topology::TerrainMeshQuery};
    let frame = SceneSourceFrame::from_transform(Transform::from_translation(Vec3::new(22.0, -4.0, 12.0))
        .with_rotation(Quat::from_rotation_y(0.6)).with_scale(Vec3::new(2.0, 3.0, 0.5)));
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for offset in [0.0, 0.25] {
        let base = vertices.len() as u32;
        vertices.extend([Vec3::new(0.0, offset, 0.0), Vec3::new(0.0, offset, 2.0),
            Vec3::new(2.0, 2.0 + offset, 2.0), Vec3::new(2.0, 2.0 + offset, 0.0)]);
        triangles.extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
    }
    let collider = Collider::trimesh(vertices, triangles);
    let mut ground = test_ground_snapshot();
    ground.terrain_meshes = Arc::from([TerrainMeshQuery::new(collider.clone(), Mat4::IDENTITY).unwrap()]);
    let queries = [TerrainObjectRaySnapshot::Mesh(collider, frame), TerrainObjectRaySnapshot::Clipmap(ground, frame)];
    for query in queries {
        for offset in [0.0, 0.25] {
            for (from, to) in [(0.2, 1.5), (1.5, 0.2)] {
                let start = frame.to_world(Vec3::new(from, from + offset, 1.0));
                let expected = frame.to_world(Vec3::new(to, to + offset, 1.0));
                let ClipmapGroundHit::Hit(actual) = query.sample_support_along_surface(start, expected.xz()) else {
                    panic!("实际对象的连续坡面应保持原承托层");
                };
                assert!(actual.distance(expected) < 0.0001, "实际 {actual:?}，预期 {expected:?}");
            }
        } // 平移、绕Y旋转及非均匀缩放后，世界上下坡超过0.5米仍不能改选相邻层。
        let start = frame.to_world(Vec3::new(1.8, 1.8, 1.0));
        let outside = frame.to_world(Vec3::new(3.0, 3.0, 1.0));
        assert_eq!(query.sample_support_along_surface(start, outside.xz()), ClipmapGroundHit::Miss);
    }
    // 纯基础Clipmap也必须按源坐标取得同代作者面；显示或基础页缺失均保持等待。
    let mut ground = test_ground_snapshot();
    let ClipmapGroundHit::Hit(start) = ground.sample_authoritative_surface(Vec2::new(20.0, 20.0)) else { panic!("作者页夹具应就绪"); };
    let ClipmapGroundHit::Hit(target) = ground.sample_authoritative_surface(Vec2::new(30.0, 25.0)) else { panic!("作者页夹具应就绪"); };
    let start = frame.to_world(start);
    let target = frame.to_world(target);
    let query = TerrainObjectRaySnapshot::Clipmap(ground.clone(), frame);
    let ClipmapGroundHit::Hit(actual) = query.sample_support_along_surface(start, target.xz()) else { panic!("纯基础高度应保留源坐标桥"); };
    assert!(actual.distance(target) < 0.0001);
    ground.missing_pages[0] = UVec4::new(1, 0, 0, 0);
    let query = TerrainObjectRaySnapshot::Clipmap(ground.clone(), frame);
    assert_eq!(query.sample_support_along_surface(start, target.xz()), ClipmapGroundHit::WaitingForHeight);
    ground.ready = false;
    let query = TerrainObjectRaySnapshot::Clipmap(ground, frame);
    assert_eq!(query.sample_support_along_surface(start, target.xz()), ClipmapGroundHit::WaitingForHeight);
}

use super::{terrain_queries::TerrainObjectQueries, instances::{SubsceneActive, SubsceneInstance,
    SubsceneRuntime, TerrainInstanceLookup, TerrainObjectInstance, register_lookup, remove_lookup, remove_empty_lookup, seal_lookup}};
use bevy::ecs::system::RunSystemOnce;
use feathrise_scene_content::{SceneId, SceneInstancePath, SceneTerrainBinding, SceneTerrainDescriptor, SceneTerrainKind,
    SceneTerrainObject, TerrainId, TerrainObjectTransform};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn legacy_clipmap_root_source_remains_available_before_a_primary_instance_exists() {
    use super::sparse_runtime::{SparseClipmapRuntime, DisplayBuildIdentity};
    use super::resources::DynamicSceneRuntimeState;
    let mut world = World::new();
    let owner = SceneId::parse("MAP-claim-primary").unwrap();
    let root = world.spawn_empty().id();
    let local = Transform::from_xyz(3.5, 0.0, -1.25);
    let mut meshes = Assets::<Mesh>::default();
    let mesh = meshes.add(Rectangle::new(64.0, 64.0));
    let display = world.spawn((Mesh3d(mesh.clone()), local)).id();
    let spec = feathrise_scene_content::SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.25, -8.0, 16.0).unwrap();
    let plan = feathrise_scene_content::compile_display_lod_plan(&spec,
        &feathrise_scene_content::default_display_lods_for_spec(&spec).unwrap()).unwrap();
    let mut clipmap = SparseClipmapRuntime::default();
    clipmap.display = Some(display);
    clipmap.applied = Some(DisplayBuildIdentity { scene_id: owner.clone(), root, plan });
    world.insert_resource(clipmap);
    world.insert_resource(DynamicSceneRuntimeState { active_scene_id: Some(owner.clone()), root: Some(root),
        sparse_height_path: Some("scenes/MAP-claim-primary/height_tiles.scene_height_manifest.json".into()), ..default() });
    // 认领只改变作者身份，未新增正式批次；对象地址查询应缺席，原root仍有真实几何。
    let map = owner.clone();
    assert!(world.run_system_once(move |queries: TerrainObjectQueries| queries.render_mesh(&map, &TerrainId::legacy())).unwrap().is_none());
    let map = owner.clone();
    let (actual, matrix, kind) = world.run_system_once(move |queries: TerrainObjectQueries| queries.root_render_mesh(&map)).unwrap().unwrap();
    assert_eq!(actual, mesh);
    assert_eq!(matrix, local.to_matrix());
    assert_eq!(kind, SceneTerrainKind::SparseClipmap);
    world.resource_mut::<DynamicSceneRuntimeState>().active_scene_id = Some(SceneId::parse("MAP-other").unwrap());
    assert!(world.run_system_once(move |queries: TerrainObjectQueries| queries.root_render_mesh(&owner)).unwrap().is_none());
}

#[test]
fn surface_render_lookup_selects_one_of_two_clipmap_objects_and_applies_pose_once() {
    use feathrise_scene_core::residency::InstanceResourceTable;
    use super::sparse_runtime::{SparseClipmapRuntime, DisplayBuildIdentity};
    use super::resources::DynamicSceneRuntimeState;
    let mut world = World::new();
    let owner = SceneId::parse("MAP-surface-lookup").unwrap();
    let batch = world.spawn(SubsceneActive).id();
    let mut addresses = BTreeMap::new();
    let mut scenes = InstanceResourceTable::<DynamicSceneRuntimeState>::default();
    let mut clipmaps = InstanceResourceTable::<SparseClipmapRuntime>::default();
    let mut meshes = Assets::<Mesh>::default();
    let spec = feathrise_scene_content::SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.25, -8.0, 16.0).unwrap();
    let plan = feathrise_scene_content::compile_display_lod_plan(&spec,
        &feathrise_scene_content::default_display_lods_for_spec(&spec).unwrap()).unwrap();
    let mut expected = Vec::new();
    for (name, offset) in [("paint-a", 7.0), ("paint-b", 70.0)] {
        let id = TerrainId::parse(name).unwrap();
        let pose = TerrainObjectTransform { translation: [offset, 2.0, -3.0], yaw_degrees: 35.0, scale: [2.0, 3.0, 0.5], ..default() };
        let frame = SceneSourceFrame::from_transform(pose.as_transform());
        let mut object = (*definition(id.clone(), pose)).clone();
        object.terrain.kind = SceneTerrainKind::SparseClipmap;
        object.world_min = spec.world_min;
        object.world_max = spec.world_max;
        object.terrain.resolution = [257; 2]; // 与64米、0.25米作者格距的256格计划保持一致。
        let path = SceneInstancePath::root(owner.clone()).terrain(id.clone());
        let entity = world.spawn((SubsceneInstance { path: path.clone(), source_scene_id: owner.clone(), generation: 1, batch },
            TerrainObjectInstance { definition: Arc::new(object) }, frame)).id();
        let root = world.spawn_empty().id();
        let local = Transform::from_xyz(1.25, 0.0, -0.5);
        let mesh = meshes.add(Rectangle::new(64.0, 64.0));
        let display = world.spawn((Mesh3d(mesh.clone()), local)).id();
        addresses.insert(path, entity);
        scenes.values.insert(entity, DynamicSceneRuntimeState { active_scene_id: Some(owner.clone()), root: Some(root), ..default() });
        let mut clipmap = SparseClipmapRuntime::default();
        clipmap.applied = Some(DisplayBuildIdentity { scene_id: owner.clone(), root, plan: plan.clone() });
        clipmap.display = Some(display);
        clipmaps.values.insert(entity, clipmap);
        expected.push((id, entity, display, mesh, frame.source_to_world * local.to_matrix()));
    }
    world.insert_resource(SubsceneRuntime::test_active(batch, addresses));
    world.insert_resource(scenes);
    world.insert_resource(clipmaps);
    for (id, _, _, mesh, matrix) in &expected {
        let map = owner.clone(); let id = id.clone();
        let (actual_mesh, actual, kind) = world.run_system_once(move |queries: TerrainObjectQueries|
            queries.render_mesh(&map, &id)).unwrap().unwrap();
        assert_eq!(&actual_mesh, mesh, "多对象必须按稳定路径返回对应几何句柄");
        assert_eq!(kind, SceneTerrainKind::SparseClipmap);
        assert_eq!(actual, *matrix, "显示局部移动与对象旋转/非均匀缩放应各应用一次");
    }
    // 旧根/隐藏候选不能凭同SceneId提供GPU预热源。
    let entity = expected[0].1;
    world.resource_mut::<InstanceResourceTable<DynamicSceneRuntimeState>>().values.get_mut(&entity).unwrap().root = Some(batch);
    let map = owner.clone(); let id = expected[0].0.clone();
    assert!(world.run_system_once(move |queries: TerrainObjectQueries| queries.render_mesh(&map, &id)).unwrap().is_none());
    world.entity_mut(batch).remove::<SubsceneActive>();
    let id = expected[1].0.clone();
    assert!(world.run_system_once(move |queries: TerrainObjectQueries| queries.render_mesh(&owner, &id)).unwrap().is_none());
}

fn definition(id: TerrainId, pose: TerrainObjectTransform) -> Arc<SceneTerrainObject> {
    Arc::new(SceneTerrainObject {clipmap_stitches: Default::default(),mesh_lod: None,id, display_name:"实例绑定回归".into(), enabled:true, transform:pose,
        world_min:[-4.;2],world_max:[4.;2],terrain:SceneTerrainDescriptor {resolved_stitches: Default::default(),kind:SceneTerrainKind::Gltf,
            glb_path:"gltf/test.glb".into(),resolution:[2;2],..SceneTerrainDescriptor::empty()},
        voxel:None,surface_path:None,vegetation_path:None,water_path:None,puddles:None})
}

fn terrain(world: &mut World, owner: &SceneId, batch: Entity, path: &SceneInstancePath,
    generation: u64, pose: TerrainObjectTransform) -> Entity {
    let object = definition(path.terrain_id.clone().unwrap(), pose);
    let entity = world.spawn((SubsceneInstance {path:path.clone(),source_scene_id:owner.clone(),generation,batch},
        TerrainObjectInstance {definition:object},SceneSourceFrame::from_transform(pose.as_transform()))).id();
    register_lookup(world,batch,path,entity);
    entity
}

fn resolve(world: &mut World, owner: &SceneId, scope: Option<Entity>, binding: &SceneTerrainBinding)
    -> Result<TerrainObjectTransform,String> {
    let owner = owner.clone();let binding = binding.clone();
    world.run_system_once(move |query:TerrainObjectQueries|query.binding_pose(&owner,scope,&binding)).unwrap()
}

struct BindingFixture {
    world:World,owner:SceneId,path:SceneInstancePath,binding:SceneTerrainBinding,
    active:Entity,candidate:Entity,active_scope:Entity,candidate_scope:Entity,
    active_pose:TerrainObjectTransform,candidate_pose:TerrainObjectTransform,candidate_terrain:Entity,
}

impl BindingFixture {
    fn new() -> Self {
        let mut world=World::new();world.init_resource::<TerrainInstanceLookup>();
        let owner=SceneId::parse("MAP-batch-binding").unwrap();let id=TerrainId::parse("island").unwrap();
        let parent=SceneInstancePath::root(owner.clone());let path=parent.terrain(id.clone());
        let active=world.spawn(SubsceneActive).id();let candidate=world.spawn_empty().id();
        let active_scope=world.spawn(SubsceneInstance {path:parent.clone(),source_scene_id:owner.clone(),generation:7,batch:active}).id();
        let candidate_scope=world.spawn(SubsceneInstance {path:parent,source_scene_id:owner.clone(),generation:8,batch:candidate}).id();
        let active_pose=TerrainObjectTransform {translation:[10.,2.,3.],yaw_degrees:0.,scale:[1.;3],..default()};
        let candidate_pose=TerrainObjectTransform {translation:[100.,20.,-5.],yaw_degrees:90.,scale:[2.,3.,4.],..default()};
        let active_terrain=terrain(&mut world,&owner,active,&path,7,active_pose);
        let candidate_terrain=terrain(&mut world,&owner,candidate,&path,8,candidate_pose);
        seal_lookup(&mut world,active);seal_lookup(&mut world,candidate);
        world.insert_resource(SubsceneRuntime::test_active(active,BTreeMap::from([(path.clone(),active_terrain)])));
        let binding=SceneTerrainBinding::capture(id,default(),TerrainObjectTransform {translation:[1.,2.,3.],yaw_degrees:15.,scale:[2.,1.,0.5],..default()}).unwrap();
        Self {world,owner,path,binding,active,candidate,active_scope,candidate_scope,active_pose,candidate_pose,candidate_terrain}
    }
}

#[test]
fn candidate_and_active_bindings_remain_in_their_batches_while_runtime_is_scoped_out() {
    let mut fixture=BindingFixture::new();
    let active=fixture.binding.resolve(fixture.active_pose).unwrap();
    let candidate=fixture.binding.resolve(fixture.candidate_pose).unwrap();
    let owner=fixture.owner.clone();let binding=fixture.binding.clone();
    let active_scope=fixture.active_scope;let candidate_scope=fixture.candidate_scope;
    fixture.world.resource_scope(|world,_runtime:Mut<SubsceneRuntime>| {
        assert!(!world.contains_resource::<SubsceneRuntime>()); // 重现driver真实scope，不通过保留Runtime绕开死锁。
        assert_eq!(resolve(world,&owner,Some(candidate_scope),&binding).unwrap(),candidate);
        assert_eq!(resolve(world,&owner,Some(active_scope),&binding).unwrap(),active);
        assert!(resolve(world,&owner,None,&binding).is_err()); // 玩法根绝不借同ID候选地址。
    });
    assert_eq!(resolve(&mut fixture.world,&owner,None,&binding).unwrap(),active);
}

#[test]
fn root_binding_requires_an_adopted_runtime_even_if_candidate_lookup_is_ready() {
    let mut fixture=BindingFixture::new();
    assert_eq!(resolve(&mut fixture.world,&fixture.owner,None,&fixture.binding).unwrap(),fixture.binding.resolve(fixture.active_pose).unwrap());
    fixture.world.insert_resource(SubsceneRuntime::default());
    assert!(resolve(&mut fixture.world,&fixture.owner,None,&fixture.binding).is_err());
    assert_eq!(resolve(&mut fixture.world,&fixture.owner,Some(fixture.candidate_scope),&fixture.binding).unwrap(),fixture.binding.resolve(fixture.candidate_pose).unwrap());
}

#[test]
fn active_terrain_parent_lookup_rejects_retired_missing_or_mismatched_instances() {
    // 地址表会晚于实体退役失效；编辑器绝不能把新预览挂到旧批次或已经消失的父级。
    for changed in 0..7 {
        let mut fixture = BindingFixture::new();
        let owner = fixture.owner.clone();
        let id = fixture.binding.terrain_id.clone();
        let query_owner = owner.clone();
        let query_id = id.clone();
        let entity = fixture.world.run_system_once(move |queries: TerrainObjectQueries|
            queries.instance(&query_owner, &query_id)).unwrap().unwrap();
        match changed {
            0 => { fixture.world.entity_mut(entity).despawn(); }
            1 => { fixture.world.entity_mut(entity).remove::<TerrainObjectInstance>(); }
            2 => { fixture.world.entity_mut(fixture.active).remove::<SubsceneActive>(); }
            3 => {
                fixture.world.get_mut::<SubsceneInstance>(entity).unwrap().source_scene_id =
                    SceneId::parse("MAP-invalid-parent").unwrap();
            }
            4 => {
                fixture.world.get_mut::<SubsceneInstance>(entity).unwrap().path =
                    SceneInstancePath::root(owner.clone()).terrain(TerrainId::parse("other-parent").unwrap());
            }
            5 | 6 => {
                let mut object = (*fixture.world.get::<TerrainObjectInstance>(entity).unwrap().definition).clone();
                if changed == 5 { object.enabled = false; }
                else { object.id = TerrainId::parse("other-parent").unwrap(); }
                fixture.world.entity_mut(entity).insert(TerrainObjectInstance { definition: Arc::new(object) });
            }
            _ => unreachable!(),
        }
        assert!(fixture.world.run_system_once(move |queries: TerrainObjectQueries|
            queries.instance(&owner, &id)).unwrap().is_none(), "失效场景 {changed} 不能返回旧父实体");
    }
}

#[test]
fn repeated_source_scene_uses_the_full_instance_path_inside_the_same_batch() {
    let mut world=World::new();world.init_resource::<TerrainInstanceLookup>();
    let root=SceneInstancePath::root(SceneId::parse("MAP-parent").unwrap());
    let owner=SceneId::parse("MAP-repeated-source").unwrap();let batch=world.spawn_empty().id();
    let id=TerrainId::parse("shared-island").unwrap();
    let binding=SceneTerrainBinding::capture(id.clone(),default(),TerrainObjectTransform {translation:[1.,2.,3.],..default()}).unwrap();
    let mut scopes=Vec::new();
    for (name,x) in [("left",10.),("right",100.)] {
        let path=root.child(feathrise_scene_content::SceneInstanceId::parse(name).unwrap());
        let scope=world.spawn(SubsceneInstance {path:path.clone(),source_scene_id:owner.clone(),generation:4,batch}).id();
        let pose=TerrainObjectTransform {translation:[x,5.,0.],..default()};
        terrain(&mut world,&owner,batch,&path.terrain(id.clone()),4,pose);
        scopes.push((scope,pose));
    }
    for (scope,pose) in scopes {
        assert_eq!(resolve(&mut world,&owner,Some(scope),&binding).unwrap(),binding.resolve(pose).unwrap());
    }
}

#[test]
fn scoped_binding_rejects_stale_batch_path_source_and_generation() {
    let mut fixture=BindingFixture::new();
    for changed in 0..4 {
        let original=fixture.world.get::<SubsceneInstance>(fixture.candidate_terrain).unwrap().clone();
        let mut invalid=original.clone();
        match changed {
            0=>invalid.batch=fixture.active,
            1=>invalid.path=invalid.path.terrain(TerrainId::parse("another-island").unwrap()),
            2=>invalid.source_scene_id=SceneId::parse("MAP-another-source").unwrap(),
            _=>invalid.generation+=1,
        }
        fixture.world.entity_mut(fixture.candidate_terrain).insert(invalid);
        assert!(resolve(&mut fixture.world,&fixture.owner,Some(fixture.candidate_scope),&fixture.binding).unwrap_err().contains("身份"));
        fixture.world.entity_mut(fixture.candidate_terrain).insert(original);
    }
}

#[test]
fn missing_target_waits_only_until_its_batch_contexts_are_complete() {
    let mut world=World::new();world.init_resource::<TerrainInstanceLookup>();
    let owner=SceneId::parse("MAP-missing-binding").unwrap();let batch=world.spawn_empty().id();
    let scope=world.spawn(SubsceneInstance {path:SceneInstancePath::root(owner.clone()),source_scene_id:owner.clone(),generation:1,batch}).id();
    let binding=SceneTerrainBinding::capture(TerrainId::parse("removed").unwrap(),default(),default()).unwrap();
    assert!(resolve(&mut world,&owner,Some(scope),&binding).unwrap_err().contains("仍在加载"));
    seal_lookup(&mut world,batch);
    let error=resolve(&mut world,&owner,Some(scope),&binding).unwrap_err();
    assert!(error.contains("removed")&&!error.contains("仍在加载"));
}

#[test]
fn retirement_removes_lookup_after_despawn_without_erasing_other_batches_or_replacements() {
    let mut fixture=BindingFixture::new();
    let replacement_pose=TerrainObjectTransform {translation:[200.,1.,0.],..default()};
    let replacement=terrain(&mut fixture.world,&fixture.owner,fixture.candidate,&fixture.path,8,replacement_pose);
    remove_lookup(&mut fixture.world,fixture.candidate,&fixture.path,fixture.candidate_terrain);
    assert_eq!(resolve(&mut fixture.world,&fixture.owner,Some(fixture.candidate_scope),&fixture.binding).unwrap(),fixture.binding.resolve(replacement_pose).unwrap()); // 迟到旧节点退役不能删除同路径的新地址。
    fixture.world.entity_mut(replacement).despawn();
    remove_lookup(&mut fixture.world,fixture.candidate,&fixture.path,replacement);
    assert!(fixture.world.resource::<TerrainInstanceLookup>().instance(fixture.candidate,&fixture.path).is_none());
    assert!(!fixture.world.resource::<TerrainInstanceLookup>().contexts_complete(fixture.candidate)); // 完成位随空行整体删除。
    assert!(resolve(&mut fixture.world,&fixture.owner,Some(fixture.candidate_scope),&fixture.binding).is_err());
    assert_eq!(resolve(&mut fixture.world,&fixture.owner,Some(fixture.active_scope),&fixture.binding).unwrap(),fixture.binding.resolve(fixture.active_pose).unwrap());
}

#[test]
fn empty_batch_retirement_releases_sealed_credit_and_keeps_populated_rows() {
    let mut fixture=BindingFixture::new();
    let empty=fixture.world.spawn_empty().id();
    seal_lookup(&mut fixture.world,empty);
    assert!(fixture.world.resource::<TerrainInstanceLookup>().contexts_complete(empty));
    remove_empty_lookup(&mut fixture.world,empty);
    assert!(!fixture.world.resource::<TerrainInstanceLookup>().contexts_complete(empty));
    remove_empty_lookup(&mut fixture.world,fixture.candidate);
    assert!(fixture.world.resource::<TerrainInstanceLookup>().contexts_complete(fixture.candidate));
    assert_eq!(fixture.world.resource::<TerrainInstanceLookup>().instance(fixture.candidate,&fixture.path),Some(fixture.candidate_terrain));
}
