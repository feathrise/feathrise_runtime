//! 作者高度快照只授信当前实际来源、已交接根及同代完整页，兄弟对象和旧GPU页不得借用。
use super::*;
use bevy::ecs::system::RunSystemOnce;
use crate::scenes::dynamic_scene::{
    clipmap::{DynamicClipmapGroundSurface, test_authoring_ground_snapshot},
    instances::resources::SubsceneRuntime,
    resources::DynamicSceneRuntimeState,
};
use feathrise_scene_content::SceneInstancePath;
use feathrise_scene_core::residency::InstanceResourceTable;
use std::collections::BTreeMap;

type Grounds = InstanceResourceTable<DynamicClipmapGroundSurface>;
type Sources = InstanceResourceTable<DynamicSceneRuntimeState>;

#[derive(Resource)]
struct Request {
    scene_id: SceneId,
    terrain: TerrainId,
    path: String,
    revision: u64,
    accepted: bool,
}
fn read(source: TerrainObjectClipmapGround, mut request: ResMut<Request>) {
    request.accepted = source.authoring_snapshot(&request.scene_id, Some(&request.terrain),
        &request.path, request.revision).is_some();
}
fn check(world: &mut World) -> bool {
    world.run_system_once(read).unwrap();
    world.resource::<Request>().accepted
}
fn fixture() -> (World, Entity, Entity) {
    let mut world = World::new();
    let root = world.spawn_empty().id();
    let first = world.spawn_empty().id();
    let second = world.spawn_empty().id();
    let map = SceneId::parse("MAP-ground-test").unwrap();
    let a = TerrainId::parse("source-a").unwrap();
    let b = TerrainId::parse("source-b").unwrap();
    world.insert_resource(SubsceneRuntime::test_active(root, BTreeMap::from([
        (SceneInstancePath::root(map.clone()).terrain(a.clone()), first),
        (SceneInstancePath::root(map.clone()).terrain(b.clone()), second),
    ])));
    world.init_resource::<DynamicClipmapGroundSurface>();
    world.init_resource::<Grounds>();
    world.init_resource::<Sources>();
    for (entity, id) in [(first, &a), (second, &b)] {
        let snapshot = test_authoring_ground_snapshot(map.clone(), entity, Some(9));
        world.resource_mut::<Grounds>().values.entry(entity).or_default().snapshot = Some(snapshot);
        world.resource_mut::<Sources>().values.insert(entity, DynamicSceneRuntimeState {
            active_scene_id: Some(map.clone()),
            sparse_height_path: Some(format!("scenes/{map}/terrains/{}/height_tiles.scene_height_manifest.json", id.as_str())),
            root: Some(entity),
            ..default()
        });
    }
    world.insert_resource(TerrainObjectAuthoringTarget {
        scene_id: Some(map.clone()), terrain: Some(a.clone()), ..default()
    });
    sync_preview_owner(&mut world);
    world.insert_resource(Request {
        path: format!("scenes/{map}/terrains/{}/height_tiles.scene_height_manifest.json", a.as_str()),
        scene_id: map, terrain: a, revision: 9, accepted: false,
    });
    (world, first, second)
}

#[test]
fn author_height_source_rejects_same_scene_sibling_and_unapplied_owner() {
    let (mut world, _, second) = fixture();
    assert!(check(&mut world));
    let original = world.resource::<Request>().path.clone();
    let sibling = world.resource::<Sources>().values[&second].sparse_height_path.clone().unwrap();
    world.resource_mut::<Request>().path = sibling.clone();
    assert!(!check(&mut world), "同图兄弟同修订仍不能借另一条源路径");
    let b = TerrainId::parse("source-b").unwrap();
    world.resource_mut::<Request>().terrain = b.clone();
    world.resource_mut::<TerrainObjectAuthoringTarget>().terrain = Some(b);
    assert!(!check(&mut world), "requested改变但作者根尚未交换时拒绝");
    sync_preview_owner(&mut world);
    assert!(check(&mut world));
    world.resource_mut::<Request>().path = original;
    assert!(!check(&mut world));
    world.resource_mut::<Request>().path = sibling;
    world.resource_mut::<Request>().scene_id = SceneId::parse("MAP-other-source").unwrap();
    assert!(!check(&mut world));
}

#[test]
fn author_height_source_waits_for_exact_live_gpu_revision_and_complete_pages() {
    let (mut world, first, _) = fixture();
    let map = world.resource::<Request>().scene_id.clone();
    assert!(check(&mut world));
    world.resource_mut::<Request>().revision = 10;
    assert!(!check(&mut world), "新live已发布但GPU仍为旧完整版本，必须等待");
    {
        let mut grounds = world.resource_mut::<Grounds>();
        let snapshot = grounds.values.get_mut(&first).unwrap().snapshot.insert(
            test_authoring_ground_snapshot(map.clone(), first, Some(10)));
        snapshot.ready = false;
    }
    assert!(!check(&mut world), "仅修订相同不代表全部页已采用");
    world.resource_mut::<Grounds>().values.get_mut(&first).unwrap().snapshot.as_mut().unwrap().ready = true;
    assert!(check(&mut world));
    world.resource_mut::<Request>().revision = 0;
    world.resource_mut::<Grounds>().values.get_mut(&first).unwrap().snapshot =
        Some(test_authoring_ground_snapshot(map.clone(), first, None));
    assert!(!check(&mut world), "没有作者覆盖不能冒充合法第0修订");
    world.resource_mut::<Grounds>().values.get_mut(&first).unwrap().snapshot =
        Some(test_authoring_ground_snapshot(map, first, Some(0)));
    assert!(check(&mut world));
    world.resource_mut::<Sources>().values.get_mut(&first).unwrap().root = Some(Entity::PLACEHOLDER);
    assert!(!check(&mut world), "同路径的旧实例根也没有新页信用");
}

#[test]
fn author_height_source_rejects_old_root_until_active_batch_handoff_finishes() {
    let (mut world, first, _) = fixture();
    let next = world.spawn_empty().id();
    let request = world.resource::<Request>();
    let path = SceneInstancePath::root(request.scene_id.clone()).terrain(request.terrain.clone());
    world.resource_mut::<SubsceneRuntime>().active.as_mut().unwrap().index.insert(path, next);
    let mut source = DynamicSceneRuntimeState::default();
    source.active_scene_id = world.resource::<Sources>().values[&first].active_scene_id.clone();
    source.sparse_height_path = world.resource::<Sources>().values[&first].sparse_height_path.clone();
    source.root = Some(next);
    world.resource_mut::<Sources>().values.insert(next, source);
    let snapshot = test_authoring_ground_snapshot(world.resource::<Request>().scene_id.clone(), next, Some(9));
    world.resource_mut::<Grounds>().values.entry(next).or_default().snapshot = Some(snapshot);
    assert!(!check(&mut world));
    sync_preview_owner(&mut world);
    assert!(check(&mut world));
}
