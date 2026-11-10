//! 不同格距、反向边、移动旋转和磁盘往返的接缝合同回归。
use super::*;
use crate::*;
use std::sync::Arc;

fn object(id: &str, spacing: f32, x: f32) -> SceneTerrainObject {
    let mut terrain = SceneTerrainDescriptor::empty();
    terrain.kind = SceneTerrainKind::SparseClipmap;
    terrain.height_path = Some(format!(
        "scenes/test/terrains/{id}/height.scene_sparse_height.json"
    ));
    terrain.resolution = [(64.0 / spacing) as u32 + 1; 2];
    terrain.collision_lods = default_collision_lods_for_resolution(terrain.resolution);
    SceneTerrainObject {
        clipmap_stitches: Default::default(),
        mesh_lod: None,
        id: TerrainId::parse(id).unwrap(),
        display_name: id.into(),
        enabled: true,
        transform: TerrainObjectTransform {
            translation: [x, 0., 0.],
            ..Default::default()
        },
        world_min: [0.; 2],
        world_max: [64.; 2],
        terrain,
        voxel: None,
        surface_path: None,
        vegetation_path: None,
        water_path: None,
        puddles: None,
    }
}
fn pair() -> (SceneTerrainObject, SceneTerrainObject) {
    let (mut a, mut b) = (object("a", 0.5, 0.), object("b", 2., 64.));
    let (al, bl) = clipmap_stitch_pair(
        &a,
        ClipmapEdge::East,
        &b,
        ClipmapEdge::West,
        "seam".into(),
        |d| Ok(d * 0.25),
    )
    .unwrap();
    a.clipmap_stitches = Arc::from([al]);
    b.clipmap_stitches = Arc::from([bl]);
    (a, b)
}
#[test]
fn stitch_different_spacing_shares_full_curve_and_source_data() {
    let (a, b) = pair();
    let objects = [a.clone(), b.clone()];
    let ap = resolve_clipmap_stitches(&a, &objects);
    let bp = resolve_clipmap_stitches(&b, &objects);
    assert_eq!(ap.len(), 1);
    assert_eq!(bp.len(), 1);
    for i in 0..=256 {
        let d = i as f32 * 0.25;
        assert!((ap[0].sample(d, 1) - bp[0].sample(d, 1)).abs() < 0.0001);
    }
    let json = serde_json::to_vec(&a).unwrap();
    let loaded: SceneTerrainObject = serde_json::from_slice(&json).unwrap();
    assert_eq!(loaded.clipmap_stitches, a.clipmap_stitches);
    assert_eq!(a.terrain.resolution, [129; 2]);
}
#[test]
fn stitch_move_rotate_suspend_and_restore_without_erasing_relation() {
    let (a, mut b) = pair();
    let saved = b.clone();
    b.transform.translation[0] += 1.;
    assert!(resolve_clipmap_stitches(&a, &[a.clone(), b.clone()]).is_empty());
    b = saved.clone();
    b.transform.yaw_degrees = 90.;
    assert!(resolve_clipmap_stitches(&a, &[a.clone(), b]).is_empty());
    assert_eq!(resolve_clipmap_stitches(&a, &[a.clone(), saved]).len(), 1);
    assert_eq!(a.clipmap_stitches.len(), 1);
}

#[test]
fn tilted_stitches_reject_creation_and_suspend_until_horizontal_without_erasing_links() {
    for pitch in [true, false] {
        let (a, mut b) = pair();
        let saved = b.clone();
        if pitch { b.transform.pitch_degrees = 25.0; } else { b.transform.roll_degrees = 20.0; }
        assert!(clipmap_stitch_pair(&a, ClipmapEdge::East, &b, ClipmapEdge::West,
            "tilted".into(), |_| Ok(0.0)).unwrap_err().contains("倾斜"));
        assert!(nearest_clipmap_stitch_pose(&b, &[a.clone()]).unwrap_err().contains("倾斜"));
        let tilted = [a.clone(), b.clone()];
        assert!(resolve_clipmap_stitches(&a, &tilted).is_empty());
        assert!(resolve_clipmap_stitches(&b, &tilted).is_empty());
        assert_eq!(b.clipmap_stitches, saved.clipmap_stitches);
        let restored = [a.clone(), saved.clone()];
        assert_eq!(resolve_clipmap_stitches(&a, &restored).len(), 1);
        assert_eq!(resolve_clipmap_stitches(&saved, &restored).len(), 1);
    }
}
#[test]
fn stitch_non_dyadic_spacing_and_overlapping_sides_are_rejected() {
    let a = object("a", 1., 0.);
    let mut b = object("b", 1., 64.);
    b.terrain.resolution = [44; 2];
    assert!(
        clipmap_stitch_pair(
            &a,
            ClipmapEdge::East,
            &b,
            ClipmapEdge::West,
            "s".into(),
            |_| Ok(0.)
        )
        .is_err()
    );
    let b = object("b", 1., 0.);
    assert!(
        clipmap_stitch_pair(
            &a,
            ClipmapEdge::East,
            &b,
            ClipmapEdge::East,
            "s".into(),
            |_| Ok(0.)
        )
        .is_err()
    );
}

#[test]
fn stitch_quarter_turn_reverses_curve_with_nonuniform_scale() {
    let mut a = object("a", 0.5, 0.);
    a.transform.translation[1] = 3.;
    a.transform.scale = [1., 2., 1.];
    let mut b = object("b", 2., 64.);
    b.transform.translation = [64., -4., 64.];
    b.transform.yaw_degrees = 90.;
    b.transform.scale = [1., 3., 2.];
    // Bevy正90度把+X转到-Z；B的South边与A的East边反向重合。
    let (al, bl) = clipmap_stitch_pair(
        &a,
        ClipmapEdge::East,
        &b,
        ClipmapEdge::South,
        "s".into(),
        |d| Ok(1. + d * d * 0.002),
    )
    .unwrap();
    a.clipmap_stitches = Arc::from([al]);
    b.clipmap_stitches = Arc::from([bl]);
    let resolved = resolve_clipmap_stitches(&a, &[a.clone(), b.clone()]);
    assert_eq!(resolved.len(), 1);
    for i in 0..=64 {
        let h = resolved[0].sample(i as f32, 1) * 2. + 3.;
        let peer = b.clipmap_stitches[0].profile.sample(64. - i as f32, 1) * 3. - 4.;
        assert!((h - peer).abs() < 0.0001, "{i}: {h} {peer}");
    }
    // 整组继续任意旋转和平移仍有效，关系不依赖世界坐标轴。
    let rotation = Quat::from_rotation_y(0.37);
    for o in [&mut a, &mut b] {
        let p = rotation * Vec3::from_array(o.transform.translation) + Vec3::new(12., 0., -8.);
        o.transform.translation = p.to_array();
        o.transform.yaw_degrees += 0.37_f32.to_degrees();
    }
    assert_eq!(resolve_clipmap_stitches(&a, &[a.clone(), b]).len(), 1);
}

#[test]
fn stitch_source_version_height_changes_and_corner_conflicts_are_guarded() {
    let (a, b) = pair();
    let mut changed = b.clone();
    changed.terrain.height_path = Some("scenes/test/replaced.scene_sparse_height.json".into());
    assert!(resolve_clipmap_stitches(&a, &[a.clone(), changed]).is_empty());
    let mut changed = b.clone();
    changed.transform.translation[1] += 1.;
    assert!(resolve_clipmap_stitches(&a, &[a.clone(), changed]).is_empty());
    let mut changed = b.clone();
    let links = Arc::make_mut(&mut changed.clipmap_stitches);
    Arc::make_mut(&mut links[0].profile.samples)[1] += 0.5;
    assert!(resolve_clipmap_stitches(&a, &[a.clone(), changed]).is_empty());
    let mut conflict = a.clone();
    let mut extra = a.clipmap_stitches[0].clone();
    extra.id = "north".into();
    extra.edge = ClipmapEdge::North;
    extra.profile.edge = ClipmapEdge::North;
    *Arc::make_mut(&mut extra.profile.samples)
        .last_mut()
        .unwrap() += 1.;
    conflict.clipmap_stitches = Arc::from([a.clipmap_stitches[0].clone(), extra]);
    assert!(validate_clipmap_stitches(&conflict).is_err());
}

#[test]
fn stitch_nearest_preview_is_bounded_and_preserves_other_links() {
    let a = object("a", 0.5, 0.);
    let mut b = object("b", 2., 65.5);
    let (_, ae, be, pose) = nearest_clipmap_stitch_pose(&b, &[a.clone()]).unwrap();
    assert_eq!((ae, be), (ClipmapEdge::West, ClipmapEdge::East));
    assert!((pose.translation[0] - 64.).abs() < 0.0001);
    b.transform.translation[0] = 66.1;
    assert!(nearest_clipmap_stitch_pose(&b, &[a]).is_err());
    let (a, b) = pair();
    assert!(nearest_clipmap_stitch_pose(&a, &[b]).is_err());
}

#[test]
fn stitch_long_non_power_of_two_edge_has_identical_endpoints_and_bounded_profile() {
    let mut a = object("a", 1., 0.);
    let mut b = object("b", 2., 64.);
    a.world_max[1] = 8190.;
    b.world_max[1] = 8190.;
    a.terrain.resolution[1] = 8191;
    b.terrain.resolution[1] = 4096;
    let (al, bl) = clipmap_stitch_pair(
        &a,
        ClipmapEdge::East,
        &b,
        ClipmapEdge::West,
        "long".into(),
        |d| Ok((d * 0.01).sin()),
    )
    .unwrap();
    assert_eq!(al.profile.samples.len(), 257);
    assert_eq!(al.profile.samples, bl.profile.samples);
    assert!((al.profile.step * 256. - 8190.).abs() < 0.0001);
    al.profile.validate().unwrap();
    a.clipmap_stitches = Arc::from([al]);
    b.clipmap_stitches = Arc::from([bl]);
    assert_eq!(resolve_clipmap_stitches(&a, &[a.clone(), b]).len(), 1);
}
