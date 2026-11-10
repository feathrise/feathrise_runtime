//! 用真实异步高度发布检查映射生效时机；完整快照换代后不得发布旧worker结果。
use super::*;
use feathrise_editor_protocol::Mode;
use feathrise_scene_content::ActiveDynamicScene;
use std::time::{Duration, Instant};

const PATH: &str = "scenes/fixture/height_tiles.scene_height_manifest.json";

fn fixture() -> (App, SceneId, u64) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let map = SceneId::parse("MAP-height-brush").unwrap();
    app.insert_resource(ActiveDynamicScene { scene_id: Some(map.clone()), ..default() });
    app.insert_resource(EmbeddedProjectState { mode: Mode::Editing, revision: 7, world_id: 33, transitioning: false });
    let ticket = systems::next_height_source_revision().unwrap();
    let mut source = SparseHeightLiveOverride::default();
    source.retarget(Some(map.clone()), ticket);
    app.insert_resource(source);
    let mut bindings = HeightSourceBindings::new(map.clone(), 7);
    bindings.insert(None, PATH.into(), 0, ticket).unwrap();
    app.insert_resource(bindings);
    (app, map, ticket)
}

fn finish(state: &mut State, world: &mut World) -> Result<Option<Option<SparseHeightLiveOverride>>, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match state.advance(world) {
            Ok(None) => {
                assert!(Instant::now() < deadline, "真实高度worker未在定向测试预算内完成");
                std::thread::sleep(Duration::from_millis(1));
            }
            result => return result,
        }
    }
}

#[test]
fn brush_ticket_moves_only_after_real_height_publication() {
    let (mut app, map, old_ticket) = fixture();
    let mut state = State::default();
    let author_revision = u64::MAX - 7; // 明确使用不同于本地分配器的作者时钟。
    state.start(app.world(), prepare(map.clone(), None, author_revision, false, vec![]).unwrap()).unwrap();
    assert_eq!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 7, 0), Some(old_ticket));
    assert!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 7, author_revision).is_none());
    let old = finish(&mut state, app.world_mut()).unwrap().unwrap().unwrap();
    assert_eq!(old.revision(), old_ticket);
    let actual = app.world().resource::<SparseHeightLiveOverride>().revision();
    assert!(actual > old_ticket);
    assert_ne!(actual, author_revision);
    assert_eq!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 7, author_revision), Some(actual));
    assert!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 7, 0).is_none());
}

#[test]
fn replacing_snapshot_rejects_late_height_before_source_or_brush_scene_is_changed() {
    let (mut app, map, old_ticket) = fixture();
    let mut state = State::default();
    state.start(app.world(), prepare(map.clone(), None, 18, true, vec![]).unwrap()).unwrap();
    app.world_mut().resource_mut::<EmbeddedProjectState>().revision = 8;
    let mut next = HeightSourceBindings::new(map.clone(), 8);
    next.insert(None, PATH.into(), 19, 901).unwrap();
    app.insert_resource(next);
    assert!(finish(&mut state, app.world_mut()).is_err());
    assert!(state.busy(), "迟到结果所有权必须保留，不能在帧内取出并析构整树");
    assert_eq!(app.world().resource::<SparseHeightLiveOverride>().revision(), old_ticket);
    assert_eq!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 8, 19), Some(901));
    assert!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 8, 18).is_none());
}

#[test]
fn first_empty_live_height_binds_a_previously_declared_source() {
    let (mut app, map, old_ticket) = fixture();
    let mut declared = HeightSourceBindings::new(map.clone(), 7);
    declared.declare(None, PATH.into()).unwrap();
    app.insert_resource(declared);
    app.insert_resource(SparseHeightLiveOverride::default());
    let mut state = State::default();
    state.start(app.world(), prepare(map.clone(), None, 0, true, vec![]).unwrap()).unwrap();
    assert!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 7, 0).is_none());
    finish(&mut state, app.world_mut()).unwrap().unwrap();
    let live = app.world().resource::<SparseHeightLiveOverride>();
    let actual = live.revision();
    assert!(actual > old_ticket);
    assert_eq!(live.scene_id(), Some(&map));
    assert_eq!(live.tile_count(), 0);
    assert_eq!(app.world().resource::<HeightSourceBindings>().resolve(&map, None, PATH, 7, 0), Some(actual));
}
