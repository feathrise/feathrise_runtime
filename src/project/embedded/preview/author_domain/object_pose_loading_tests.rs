//! 来源发布回执早于loader/活动根时保留同一条姿态命令，定义接回允许先于桥观察。
use super::*;
use super::super::super::{Prepared as DomainPrepared, State as DomainState, Value};
use feathrise_scene::{SubMapActive, SubMapInstance};

#[test]
fn object_pose_loading_ack_waits_for_new_terrain_and_submap_roots_in_same_world() {
    for submap in [false, true] {
        let mut f = fixture();
        let original = f.app.world().resource::<Assets<MapManifestAsset>>().get(&f.manifest).unwrap();
        let original_terrains = original.terrains.clone();
        let original_submaps = original.sub_maps.clone();
        let batch = f.app.world().get::<SubMapInstance>(f.root).unwrap().batch;
        f.app.world_mut().entity_mut(batch).remove::<SubMapActive>();
        {
            let mut assets = f.app.world_mut().resource_mut::<Assets<MapManifestAsset>>();
            let mut old = assets.get_mut(&f.manifest).unwrap();
            old.terrains = Some(Arc::from([]));
            old.sub_maps = Arc::from([]);
        }
        let target = if submap {
            Target::SubMap { id: MapInstanceId::parse("child").unwrap(),
                pose: Some(MapInstanceTransform { translation: [8., 0., 0.], ..default() }) }
        } else {
            Target::Terrain { id: f.terrain.clone(),
                pose: Some(TerrainObjectTransform { translation: [8., 0., 0.], ..default() }) }
        };
        let mut state = DomainState::default();
        state.receive(f.app.world_mut(), DomainPrepared { value: Value::ObjectPose(Prepared {
            map: f.map.clone(), revision: 1, active: true, target,
        }) }).unwrap();
        for _ in 0..3 {
            assert!(!state.advance(f.app.world_mut(), false, false).unwrap());
            assert!(state.pending.is_some(), "发布ACK不能使旧Assets被当成新正式清单");
        }
        {
            let mut assets = f.app.world_mut().resource_mut::<Assets<MapManifestAsset>>();
            let mut adopted = assets.get_mut(&f.manifest).unwrap();
            adopted.terrains = original_terrains;
            adopted.sub_maps = original_submaps;
        }
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(!state.advance(f.app.world_mut(), true, true).unwrap());
            if state.object_pose.ready.is_some() { break; }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(f.app.world().resource::<MapInstanceAuthorPreview>().map_id.is_none());
        assert!(f.app.world().resource::<TerrainObjectTransformPreview>().pose.is_none());
        // 仅激活原已安装批次，姿态仍由桥和原生产消费者采用。
        f.app.world_mut().entity_mut(batch).insert(SubMapActive);
        loop {
            let done = state.advance(f.app.world_mut(), true, true).unwrap();
            apply_terrain_pose_preview(f.app.world_mut());
            if done { break; }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(f.app.world().get_entity(f.root).is_ok(), "等待期间保留同一World原根");
        if submap {
            assert_eq!(f.app.world().resource::<MapInstanceAuthorPreview>().instances[0].transform.translation[0], 8.);
        } else {
            assert_eq!(f.app.world().get::<Transform>(f.root).unwrap().translation.x, 8.);
        }
    }
}

#[test]
fn object_pose_loading_failure_and_invalid_id_report_errors_after_source_settles() {
    for failed in [false, true] {
        let mut f = fixture();
        let mut state = DomainState::default();
        state.receive(f.app.world_mut(), DomainPrepared { value: Value::ObjectPose(Prepared {
            map: f.map.clone(), revision: 1, active: true,
            target: Target::Terrain { id: TerrainId::parse("missing").unwrap(), pose: Some(default()) },
        }) }).unwrap();
        assert!(!state.advance(f.app.world_mut(), false, false).unwrap());
        let until = Instant::now() + Duration::from_secs(5);
        let error = loop {
            if let Err(error) = state.advance(f.app.world_mut(), true, !failed) { break error; }
            assert!(Instant::now() < until, "已就绪清单的非法目标不能永久等待");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(error.contains(if failed { "加载失败" } else { "不存在" }), "{error}");
        assert!(f.app.world().resource::<TerrainObjectTransformPreview>().pose.is_none());
    }
}

#[test]
fn object_pose_definition_adopted_before_manifest_observation_releases_old_override() {
    let mut f = fixture();
    let mut state = State::default();
    let pose = TerrainObjectTransform { translation: [9., 0., 0.], ..default() };
    send(&mut f, &mut state, pose, true, 1);
    send(&mut f, &mut state, pose, false, 2);
    let mut next = f.app.world().get::<TerrainObjectInstance>(f.root).unwrap().definition.as_ref().clone();
    next.transform.translation[0] = 3.;
    f.app.world_mut().resource_mut::<Assets<MapManifestAsset>>().get_mut(&f.manifest).unwrap().terrains = Some(Arc::from([next.clone()]));
    // 新定义已先采用；桥本次观察不能把它再次记为“尚待替换的旧定义”。
    f.app.world_mut().get_mut::<TerrainObjectInstance>(f.root).unwrap().definition = Arc::new(next);
    settle(&mut f, &mut state);
    assert!(f.app.world().resource::<TerrainObjectTransformPreview>().pose.is_none());
    assert_eq!(f.app.world().get::<Transform>(f.root).unwrap().translation.x, 3.);
}
