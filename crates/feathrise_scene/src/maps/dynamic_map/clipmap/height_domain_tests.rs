//! 无固定高度域的真实页worker、CPU拾取和草根范围回归；旧版本等待仍严格隔离。
use super::super::ground::{ClipmapGroundHit, ClipmapGroundSnapshot};
use super::*;

fn source(
    height: f32,
) -> (
    SparseHeightManifest,
    SparseHeightLiveOverride,
    PageSetIdentity,
) {
    let manifest = SparseHeightManifest::empty(
        MapId::parse("MAP-float-height-domain").unwrap(),
        SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -8.0, 16.0).unwrap(),
    )
    .unwrap();
    let key = SparseHeightTileKey { x: 0, z: 0 };
    let mut tile = SparseHeightTile::blank(&manifest.spec, key).unwrap();
    let code = manifest.spec.encode_height(height).unwrap();
    for z in 0..tile.shape[1] {
        for x in 0..tile.shape[0] {
            tile.set([x, z], code).unwrap();
        }
    }
    let mut live = SparseHeightLiveOverride::default();
    live.retarget(Some(manifest.map_id.clone()), 1);
    live.publish_tiles(&manifest.map_id, 2, [Arc::new(tile)])
        .unwrap();
    let plan = feathrise_map_content::compile_display_lod_plan(
        &manifest.spec,
        &feathrise_map_content::default_display_lods_for_spec(&manifest.spec).unwrap(),
    )
    .unwrap();
    let identity = desired_identity(
        &manifest,
        Entity::from_bits(719),
        1,
        [32, 32],
        &plan,
        Some(&live),
        None,
    )
    .unwrap();
    (manifest, live, identity)
}

#[test]
fn unbounded_float_pages_ground_and_grass_share_adopted_meter_bounds() {
    for meters in [-384.5_f32, 512.25] {
        let (manifest, live, identity) = source(meters);
        assert_eq!(identity.height_bounds, [meters.min(0.0), meters.max(0.0)]);
        let built = build_page_set_with_live(
            identity.clone(),
            manifest.spec.clone(),
            BTreeMap::new(),
            live.snapshot(),
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(built.cpu.height_bounds, identity.height_bounds);
        assert!(built.height_bytes.len() + built.page_table_bytes.len() <= MAX_PAGE_UPLOAD_BYTES);
        let mut pages = SparseHeightPageRuntime::default();
        pages.map_id = Some(manifest.map_id.clone());
        pages.root = Some(identity.root);
        pages.spec = Some(manifest.spec.clone());
        pages.extension = Some(
            ClipmapHeightExtension::new_flat(
                ClipmapBounds::new(Vec2::ZERO, Vec2::splat(64.0)).unwrap(),
                UVec2::splat(64),
                &ClipmapFlatHeightPage {
                    heights: Handle::default(),
                    page_table: Handle::default(),
                },
            )
            .unwrap(),
        );
        pages.cpu = Some(Arc::clone(&built.cpu));
        pages.resident_identity = Some(identity.clone());
        pages.desired = Some(identity.clone());
        let height = pages
            .blade_height(&manifest.map_id, &identity.display_plan, Vec2::splat(32.0))
            .unwrap();
        assert!(height.ready);
        assert_eq!(
            [height.height_min, height.height_max],
            identity.height_bounds
        );
        let ground =
            ClipmapGroundSnapshot::adopted_pages_for_test(height.clone(), Arc::clone(&built.cpu));
        let ClipmapGroundHit::Hit(point) =
            ground.height_ray_intersection(Vec3::new(32.25, meters + 20.0, 32.25), Vec3::NEG_Y)
        else {
            panic!("超出旧±8米量程的真实页应可拾取：{meters}");
        };
        assert!((point.y - meters).abs() < 0.001);
        let (min, max) = ground.surface_bounds().unwrap();
        assert!(min.y <= meters && max.y >= meters);
        let desired = pages.desired.as_mut().unwrap();
        desired.live_revision = Some(3);
        desired.height_bounds = [-900.0, 1800.0];
        assert_eq!(
            pages.adopted_height_bounds(&manifest.map_id),
            Some(identity.height_bounds)
        );
        assert!(
            !pages
                .blade_height(&manifest.map_id, &identity.display_plan, Vec2::splat(32.0))
                .unwrap()
                .ready
        );
        let old_visual = pages
            .blade_render_height(&manifest.map_id, &identity.display_plan, Vec2::splat(32.0))
            .unwrap();
        assert!(old_visual.ready, "新候选等待不应提前关闭旧完整草地");
        assert_eq!(
            [old_visual.height_min, old_visual.height_max],
            identity.height_bounds
        );
        pages.map_id = Some(MapId::parse("MAP-float-other").unwrap());
        assert!(pages.adopted_height_bounds(&manifest.map_id).is_none());
    }
}

#[test]
fn unbounded_float_gpu_rejects_only_nonfinite_span_without_replacing_old_pages() {
    let (manifest, live, mut identity) = source(1024.0);
    identity.height_bounds = [-f32::MAX, f32::MAX];
    let error = build_page_set_with_live(
        identity,
        manifest.spec,
        BTreeMap::new(),
        live.snapshot(),
        BTreeMap::new(),
    )
    .err()
    .unwrap();
    assert!(
        error.contains("无法有限表达"),
        "有限f32作者数据不能被静默截回旧量程或编码NaN：{error}"
    );
}
