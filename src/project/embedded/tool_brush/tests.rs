//! 不同World只交换稳定来源；本地root/页票据替换、快照换代和同图兄弟隔离的合同回归。
use super::*;
use feathrise_editor_protocol::{BrushShape, ClipmapBrushSurface, Mode};

fn brush(map: &MapId, terrain: Option<&TerrainId>, revision: u64) -> BrushPreview {
    BrushPreview {
        spatial: false, center: [2.0, 4.0, 6.0], rotation: [0.0, 0.0, 0.0, 1.0], radius: 8.0,
        strength: Some(0.4), shape: BrushShape::Planar, half_height: 0.0, flatten: false,
        color: [0.2, 0.5, 0.7, 1.0], clipmap: Some(ClipmapBrushSurface {
            map_id: map.0.clone(), terrain_id: terrain.map(|id| id.as_str().to_owned()), height_path: "maps/fixture/heights.json".into(),
            authoring_height_revision: revision,
        }),
    }
}

#[test]
fn clipmap_binding_requires_exact_map_terrain_path_author_clock_and_snapshot() {
    let map = MapId::parse("MAP-brush-bindings").unwrap();
    let a = TerrainId::parse("a").unwrap();
    let b = TerrainId::parse("b").unwrap();
    let mut sources = HeightSourceBindings::new(map.clone(), 7);
    sources.insert(Some(a.clone()), "maps/fixture/heights.json".into(), 0, 513).unwrap();
    sources.insert(Some(b.clone()), "maps/fixture/other.json".into(), 0, 514).unwrap();
    let mut preview = brush(&map, Some(&a), 0);
    let active = ActiveDynamicMap { map_id: Some(map.clone()), ..default() };
    let mut state = EmbeddedProjectState { mode: Mode::Editing, revision: 7, world_id: 22, transitioning: false };
    assert_eq!(binding(&state, &active, &sources, &preview).unwrap().2, 513, "作者0修订映射为独立本地ticket");
    let surface = preview.clipmap.as_mut().unwrap();
    surface.terrain_id = Some(b.as_str().to_owned());
    assert!(binding(&state, &active, &sources, &preview).is_none(), "兄弟同修订但路径不符");
    preview.clipmap.as_mut().unwrap().terrain_id = Some(a.as_str().to_owned());
    preview.clipmap.as_mut().unwrap().authoring_height_revision = 513;
    assert!(binding(&state, &active, &sources, &preview).is_none(), "不能把本地票据当作者时钟");
    preview.clipmap.as_mut().unwrap().authoring_height_revision = 0;
    preview.clipmap.as_mut().unwrap().map_id = "MAP-other".into();
    assert!(binding(&state, &active, &sources, &preview).is_none());
    preview.clipmap.as_mut().unwrap().map_id = map.0.clone();
    state.revision = 8;
    assert!(binding(&state, &active, &sources, &preview).is_none(), "新快照不可复用旧来源映射");
    assert!(!sources.published_preview(&map, Some(&a), 8, 1, 515));
    assert_eq!(sources.resolve(&map, Some(&a), "maps/fixture/heights.json", 7, 0), Some(513));
}

#[test]
fn clipmap_rebinding_uses_only_local_gpu_root_revision_and_missing_pages() {
    let map = MapId::parse("MAP-brush-bindings").unwrap();
    let original = brush(&map, None, 0);
    let local_revision = [1_001, 2_002, 3_003];
    let local_missing = [UVec4::new(3, 4, 5, 6); 16];
    let local = local_brush(&original, local_revision, local_missing).unwrap();
    assert_eq!(local.revision, local_revision);
    assert_eq!(local.missing_pages, local_missing);
    assert_ne!(local.revision[1], original.clipmap.as_ref().unwrap().authoring_height_revision);
    assert_eq!(local.map_id, map);
    assert_eq!(local.center, Vec2::new(2.0, 6.0));
    assert_eq!(local.radius, 8.0);
    assert_eq!(local.strength, 0.4);
}

#[test]
fn declared_source_has_no_clock_until_a_real_height_publication_grants_it() {
    let map = MapId::parse("MAP-brush-bindings").unwrap();
    let mut sources = HeightSourceBindings::new(map.clone(), 7);
    let path = "maps/fixture/heights.json";
    sources.declare(None, path.into()).unwrap();
    assert!(sources.resolve(&map, None, path, 7, 0).is_none(), "清单声明不能伪装首次空live已经采用");
    assert!(!sources.published_preview(&map, None, 8, 0, 513));
    assert!(sources.resolve(&map, None, path, 7, 0).is_none());
    assert!(sources.published_preview(&map, None, 7, 0, 513));
    assert_eq!(sources.resolve(&map, None, path, 7, 0), Some(513));
    assert!(sources.resolve(&map, None, "maps/other/heights.json", 7, 0).is_none());
}
