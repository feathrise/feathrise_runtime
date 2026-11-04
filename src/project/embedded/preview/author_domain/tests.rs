use super::*;
use feathrise_map_content::{
    ActiveDynamicMap, MapPlacementAssetTarget, MapPlacementRecord,
    embedded_author_preview::{AuthorPreview, MeshPreview, encode},
};
use feathrise_scene::MapPlacementInstance;

fn setup() -> (App, MapId, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    super::super::placement::install(&mut app);
    install(&mut app);
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>();
    let map = MapId::parse("MAP-live-mesh").unwrap();
    app.insert_resource(ActiveDynamicMap {
        map_id: Some(map.clone()),
        ..default()
    });
    let record:MapPlacementRecord=serde_json::from_value(serde_json::json!({"id":"mesh","asset_path":"",
        "asset_target":MapPlacementAssetTarget::DEFAULT_CUBOID,"translation":[0.,0.,0.],"yaw_degrees":0.,"scale":[1.,1.,1.]})).unwrap();
    let root = app
        .world_mut()
        .spawn((
            MapPlacementInstance::new(&map, &record),
            Transform::IDENTITY,
            Visibility::Inherited,
        ))
        .id();
    (app, map, root)
}
#[test]
fn author_domain_mesh_real_geometry_cancel_restores_only_owned_root() {
    let (mut app, map, root) = setup();
    let mut state = State::default();
    let bytes = encode(&AuthorPreview::Mesh {
        map_id: map.clone(),
        revision: 1,
        committed: false,
        value: Some(MeshPreview {
            placement_id: "mesh".into(),
            target: MapPlacementAssetTarget::DEFAULT_CUBOID,
            local: Mat4::IDENTITY.to_cols_array(),
        }),
    })
    .unwrap();
    state
        .receive(
            app.world_mut(),
            prepare(AUTHOR_PREVIEW_FORMAT, &bytes, &map).unwrap(),
        )
        .unwrap();
    assert!(state.advance(app.world_mut(), true, true).unwrap());
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
    assert_eq!(
        app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
        1
    );
    let end = encode(&AuthorPreview::Mesh {
        map_id: map.clone(),
        revision: 2,
        committed: false,
        value: None,
    })
    .unwrap();
    state
        .receive(
            app.world_mut(),
            prepare(AUTHOR_PREVIEW_FORMAT, &end, &map).unwrap(),
        )
        .unwrap();
    assert!(state.advance(app.world_mut(), true, true).unwrap());
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Inherited)
    );
    assert_eq!(
        app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
        0
    );
    assert!(app.world().get::<MapPlacementInstance>(root).is_some());
}
#[test]
fn author_domain_rejects_wrong_map_and_invalid_mesh_before_any_runtime_write() {
    let (_, map, _) = setup();
    let wrong = MapId::parse("MAP-other").unwrap();
    let bytes = encode(&AuthorPreview::Mesh {
        map_id: wrong,
        revision: 1,
        value: None,
        committed: false,
    })
    .unwrap();
    assert!(prepare(AUTHOR_PREVIEW_FORMAT, &bytes, &map).is_err());
    let bytes = encode(&AuthorPreview::Mesh {
        map_id: map.clone(),
        revision: 1,
        committed: false,
        value: Some(MeshPreview {
            placement_id: "mesh".into(),
            target: MapPlacementAssetTarget::DEFAULT_CUBOID,
            local: [0.; 16],
        }),
    })
    .unwrap();
    assert!(prepare(AUTHOR_PREVIEW_FORMAT, &bytes, &map).is_err());
}
