//! 制作总量超过旧门、空间换入/卸载、共享边、持久历史及损坏/写入失败回归。
use super::*;
use super::{store::Store, types::*};
use crate::{
    TerrainMeshInput,
    volume::{VolumeBrush, VolumeBrushKind, sparse::*},
};
use bevy::math::{Rect, Vec2};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static ID: AtomicU64 = AtomicU64::new(0);
#[path = "long_stroke_tests.rs"]
mod long_stroke_tests;
pub(super) struct Temp(pub(super) PathBuf);
impl Temp {
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rpg-volume-stream-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn brush(point: [f32; 3]) -> VolumeBrush {
    VolumeBrush {
        kind: VolumeBrushKind::Dig,
        from: point,
        to: point,
        radius: 2.,
        strength: 1.,
        ..Default::default()
    }
}
fn bounds() -> Rect {
    Rect::from_corners(Vec2::splat(-1024.), Vec2::splat(1024.))
}
#[test]
fn tall_rule_brush_reads_vertical_blocks_and_old_persistent_roots_remain_unchanged() {
    use crate::volume::VolumeBrushShape;
    let temp = Temp::new();
    let empty = PagedVolume::new(0.25).unwrap();
    let fill = VolumeBrush {
        kind: VolumeBrushKind::Fill,
        shape: VolumeBrushShape::Cylinder,
        from: [4., 4., 4.],
        to: [4., 4., 4.],
        radius: 0.75,
        half_height: 6.5,
        ..Default::default()
    };
    let before = load_edit_window(&temp.0, &empty, fill, Arc::default()).unwrap();
    let filled = apply_sparse_brush(&before, fill, bounds(), |_| Ok(0.07)).unwrap();
    assert!(
        filled.value_at([16, 36, 16]).unwrap() < 0.,
        "高圆柱应填到上方体积块"
    );
    let old = write_delta(
        &temp.0,
        &empty,
        prepare_delta(Some(&before), &filled).unwrap(),
    )
    .unwrap();
    let loaded = load_edit_window(&temp.0, &old, fill, Arc::default()).unwrap();
    assert!(
        loaded.value_at([16, 36, 16]).unwrap() < 0.,
        "预取不能仍按横向半径漏掉高处密度"
    );
    let dig = VolumeBrush {
        kind: VolumeBrushKind::Dig,
        ..fill
    };
    let dug = apply_sparse_brush(&loaded, dig, bounds(), |_| Ok(0.07)).unwrap();
    assert!(dug.value_at([16, 36, 16]).unwrap() > 0.);
    let next = write_delta(&temp.0, &old, prepare_delta(Some(&loaded), &dug).unwrap()).unwrap();
    let again = load_edit_window(&temp.0, &next, fill, Arc::default()).unwrap();
    assert!(again.value_at([16, 36, 16]).unwrap() > 0.);
    let restored = load_edit_window(&temp.0, &old, fill, Arc::default()).unwrap();
    assert!(
        restored.value_at([16, 36, 16]).unwrap() < 0.,
        "撤销根不能被新版本覆盖"
    );
}

pub(super) fn resident(temp: &Temp, v: &PagedVolume, focus: [f32; 3]) -> ResidentVolume {
    build_resident_volume(load_resident_volume(&temp.0, v, focus, 4).unwrap()).unwrap()
}
pub(super) fn edit(temp: &Temp, previous: &PagedVolume, p: [f32; 3]) -> PagedVolume {
    let b = brush(p);
    let before = load_edit_window(&temp.0, previous, b, Arc::default()).unwrap();
    let after = apply_sparse_brush(&before, b, bounds(), |_| Ok(0.07)).unwrap();
    write_delta(
        &temp.0,
        previous,
        prepare_delta(Some(&before), &after).unwrap(),
    )
    .unwrap()
}
fn grid([x, z]: [i32; 2]) -> TerrainMeshInput {
    let mut m = TerrainMeshInput {
        positions: Vec::new(),
        indices: Vec::new(),
    };
    for iz in 0..=32 {
        for ix in 0..=32 {
            m.positions.push([
                x as f32 * 8. + ix as f32 * 0.25,
                0.07,
                z as f32 * 8. + iz as f32 * 0.25,
            ]);
        }
    }
    for z in 0..32 {
        for x in 0..32 {
            let a = z * 33 + x;
            m.indices.extend([a, a + 33, a + 34, a, a + 34, a + 1]);
        }
    }
    m
}
fn mesh_bytes(m: &TerrainMeshInput) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend((m.positions.len() as u32).to_le_bytes());
    b.extend((m.indices.len() as u32).to_le_bytes());
    b.extend(m.positions.iter().flatten().flat_map(|v| v.to_le_bytes()));
    b.extend(m.indices.iter().flat_map(|v| v.to_le_bytes()));
    b
}

#[test]
fn volume_paged_fine_window_accepts_geometry_above_legacy_single_mesh_totals() {
    let temp = Temp::new();
    let mut v = PagedVolume::new(0.25).unwrap();
    for i in 0..59 {
        let p = [i % 8, i / 8];
        let positions = (0..=96)
            .flat_map(|z| {
                (0..=96).map(move |x| {
                    [
                        p[0] as f32 * 8. + x as f32 / 12.,
                        0.07,
                        p[1] as f32 * 8. + z as f32 / 12.,
                    ]
                })
            })
            .collect();
        let mut indices = Vec::new();
        for z in 0..96u32 {
            for x in 0..96u32 {
                let a = z * 97 + x;
                indices.extend([a, a + 97, a + 1, a + 1, a + 97, a + 98]);
            }
        }
        v = write_delta(
            &temp.0,
            &v,
            VolumeDelta {
                columns: vec![LeafWrite {
                    key: key([p[0], 0, p[1]]).unwrap(),
                    bytes: vec![0.07f32; 33 * 33]
                        .iter()
                        .flat_map(|v| v.to_le_bytes())
                        .collect(),
                }],
                density: Vec::new(),
                meshes: vec![LeafWrite {
                    key: key([p[0], 0, p[1]]).unwrap(),
                    bytes: mesh_bytes(&TerrainMeshInput { positions, indices }),
                }],
                bounds: Some([[0, 0], [8, 8]]),
            },
        )
        .unwrap();
    }
    let mesh = resident(&temp, &v, [32., 0., 32.]).mesh.unwrap();
    assert!(mesh.positions.len() > 262_144);
    assert_eq!(mesh.indices.len() / 3, 59 * 96 * 96 * 2);
}
#[test]
fn volume_paged_total_exceeds_512_blocks_256_columns_and_global_geometry_without_resident_growth() {
    let mut app = bevy::prelude::App::new();
    app.add_plugins(bevy::prelude::MinimalPlugins);
    let temp = Temp::new();
    let mut v = PagedVolume::new(0.25).unwrap();
    for batch in 0..30 {
        let mut delta = VolumeDelta {
            columns: Vec::new(),
            density: Vec::new(),
            meshes: Vec::new(),
            bounds: Some([[0, 0], [25, 24]]),
        };
        for i in batch * 20..(batch + 1) * 20 {
            let p = [i % 25, i / 25];
            delta.columns.push(LeafWrite {
                key: key([p[0], 0, p[1]]).unwrap(),
                bytes: vec![0.07f32; 33 * 33]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
            });
            let mut values = vec![-1i16; 32usize.pow(3)];
            values[0] = -(i as i16 + 1);
            delta.density.push(LeafWrite {
                key: key([p[0], 0, p[1]]).unwrap(),
                bytes: values.iter().flat_map(|v| v.to_le_bytes()).collect(),
            });
            delta.meshes.push(LeafWrite {
                key: key([p[0], 0, p[1]]).unwrap(),
                bytes: mesh_bytes(&grid(p)),
            });
        }
        v = write_delta(&temp.0, &v, delta).unwrap();
    }
    assert!(600 * DENSITY_BYTES > 32 * 1024 * 1024);
    assert!(600 * 2048 > 262_144);
    let bytes = encode_paged_volume(&v).unwrap();
    assert!(bytes.len() < 4096);
    let reopened = decode_paged_volume(&bytes).unwrap().unwrap();
    assert_eq!(v, reopened);
    let near = resident(&temp, &v, [32., 0., 32.]);
    let mesh = near.mesh.unwrap();
    assert_eq!(mesh.indices.len() / 3, 81 * 2048);
    assert!(mesh.positions.len() < 262_144);
    let away = resident(&temp, &v, [700., 0., 700.]);
    assert!(away.mesh.is_none() && away.footprint.is_empty());
    assert_eq!(
        mesh,
        resident(&temp, &reopened, [32., 0., 32.]).mesh.unwrap()
    );
    let loaded =
        load_edit_window(&temp.0, &reopened, brush([32., 0., 32.]), Arc::default()).unwrap();
    assert!(loaded.block_count() < 32 && loaded.column_count() < 32);
    // 源网格总量达1228800个三角；移走仅卸载精细数据，显示仍保留整片的粗代理。
    let far = bevy::tasks::block_on(load_display_volume_at_foci(
        &temp.0,
        &reopened,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap();
    assert_eq!(far.footprint, vec![[[0., 0.], [200., 192.]]]);
    let coarse = far.mesh.unwrap();
    assert!(coarse.indices.len() / 3 < 262_144);
    assert!(
        coarse
            .positions
            .iter()
            .any(|p| p[0] == 200. && p[2] == 192.)
    );
    let mixed = bevy::tasks::block_on(load_display_volume_at_foci(
        &temp.0,
        &reopened,
        &[[32., 0., 32.]],
        4,
    ))
    .unwrap();
    assert_eq!(mixed.footprint, far.footprint);
    let mixed = mixed.mesh.unwrap();
    assert!(mixed.indices.len() / 3 >= 81 * 2048);
    assert!(mixed.indices.len() / 3 < 262_144);
    assert!(mixed.positions.iter().any(|p| p[0] == 200. && p[2] == 192.));
}
#[test]
fn volume_paged_legacy_migration_keeps_distant_meshes_outside_the_last_brush() {
    let temp = Temp::new();
    let legacy = new_sparse_volume(0.25, Arc::default()).unwrap();
    let legacy = apply_sparse_brush(&legacy, brush([4., 0., 4.]), bounds(), |_| Ok(0.07)).unwrap();
    let legacy =
        apply_sparse_brush(&legacy, brush([164., 0., 164.]), bounds(), |_| Ok(0.07)).unwrap();
    let legacy = VolumeSource::Sparse(legacy);
    let original_mesh = build_author_volume_mesh(&legacy).unwrap();
    let original_file = encode_author_volume_glb(&legacy, &original_mesh).unwrap();
    let VolumeSource::Sparse(reopened) =
        decode_author_volume_glb(&original_file, Arc::default()).unwrap()
    else {
        panic!("旧分块源应保留v2精确重开");
    };
    let next =
        apply_sparse_brush(&reopened, brush([164., -1., 164.]), bounds(), |_| Ok(0.07)).unwrap();
    assert!(
        next.meshes
            .keys()
            .any(|p| !next.last_rebuilt_keys.contains(p))
    );
    let paged = write_delta(
        &temp.0,
        &PagedVolume::new(0.25).unwrap(),
        prepare_delta(None, &next).unwrap(),
    )
    .unwrap();
    let resident = build_resident_volume(
        load_resident_volume_at_foci(&temp.0, &paged, &[[4., 0., 4.], [164., 0., 164.]], 4)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(resident.footprint.len(), 2);
    assert_eq!(
        resident.mesh.unwrap(),
        build_author_volume_mesh(&VolumeSource::Sparse(next)).unwrap()
    );
    assert_eq!(build_author_volume_mesh(&legacy).unwrap(), original_mesh);
    assert_eq!(
        encode_author_volume_glb(&legacy, &original_mesh).unwrap(),
        original_file
    );
}

#[test]
fn volume_paged_real_edit_unload_restore_and_reedit_matches_unpaged_shared_source() {
    let temp = Temp::new();
    let empty = PagedVolume::new(0.25).unwrap();
    let first = edit(&temp, &empty, [7.75, 0., 7.75]);
    let before_mesh = resident(&temp, &first, [8., 0., 8.]).mesh.unwrap();
    let second = edit(&temp, &first, [9., -1., 8.]);
    let original = new_sparse_volume(0.25, Arc::default()).unwrap();
    let original =
        apply_sparse_brush(&original, brush([7.75, 0., 7.75]), bounds(), |_| Ok(0.07)).unwrap();
    let original =
        apply_sparse_brush(&original, brush([9., -1., 8.]), bounds(), |_| Ok(0.07)).unwrap();
    assert_eq!(
        build_author_volume_mesh(&VolumeSource::Sparse(original)).unwrap(),
        resident(&temp, &second, [8., 0., 8.]).mesh.unwrap()
    );
    let third = edit(&temp, &second, [160., 0., 160.]);
    assert!(resident(&temp, &third, [160., 0., 160.]).mesh.is_some());
    assert_eq!(
        before_mesh,
        resident(&temp, &first, [8., 0., 8.]).mesh.unwrap()
    ); // 撤销到已经卸载的旧根不复制整层。
    assert_eq!(
        resident(&temp, &second, [8., 0., 8.]).mesh,
        resident(&temp, &third, [8., 0., 8.]).mesh
    );
    assert!(resident(&temp, &first, [160., 0., 160.]).mesh.is_none());
    let dual = build_resident_volume(
        load_resident_volume_at_foci(&temp.0, &third, &[[8., 0., 8.], [160., 0., 160.]], 4)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(dual.footprint.len(), 2);
    assert!(dual.mesh.unwrap().positions.iter().any(|p| p[0] > 150.));
}
#[test]
fn volume_paged_deep_expansion_keeps_preexisting_surface_density_and_empty_mesh_deletions() {
    let temp = Temp::new();
    let first = edit(&temp, &PagedVolume::new(0.25).unwrap(), [7.75, 0., 7.75]);
    let before_mesh = resident(&temp, &first, [8., 0., 8.]).mesh.unwrap();
    let second = edit(&temp, &first, [16., -64., 8.]);
    let near = resident(&temp, &second, [8., 0., 8.]).mesh.unwrap();
    // 新地下列冻结共享地表时不能把旧洞口封回去；比较旧列内部的三角坐标集合。
    fn triangles(m: &TerrainMeshInput) -> std::collections::BTreeSet<[[u32; 3]; 3]> {
        m.indices
            .chunks_exact(3)
            .map(|t| std::array::from_fn(|i| m.positions[t[i] as usize].map(f32::to_bits)))
            .filter(|p| p.iter().all(|p| f32::from_bits(p[0]) < 8.))
            .collect()
    }
    assert_eq!(triangles(&before_mesh), triangles(&near));
    let mut store = Store::new(&temp.0);
    let old_mesh = store.lookup(first.meshes, key([0, 0, 0]).unwrap()).unwrap();
    assert!(old_mesh.is_some());
    let next = write_delta(
        &temp.0,
        &first,
        VolumeDelta {
            columns: Vec::new(),
            density: Vec::new(),
            meshes: vec![LeafWrite {
                key: key([0, 0, 0]).unwrap(),
                bytes: Vec::new(),
            }],
            bounds: None,
        },
    )
    .unwrap();
    let mut store = Store::new(&temp.0);
    assert!(
        store
            .lookup(next.meshes, key([0, 0, 0]).unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.lookup(first.meshes, key([0, 0, 0]).unwrap()).unwrap(),
        old_mesh
    );
}
#[test]
fn volume_paged_corrupt_or_missing_objects_unknown_recipe_and_failed_write_do_not_change_old_root()
{
    let temp = Temp::new();
    let first = edit(&temp, &PagedVolume::new(0.25).unwrap(), [4., 0., 4.]);
    let mut store = Store::new(&temp.0);
    let hash = store
        .lookup(first.meshes, key([0, 0, 0]).unwrap())
        .unwrap()
        .unwrap();
    let path = temp.0.join(format!("{}.vblk", super::store::hex(hash)));
    let old = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(
        load_resident_volume(&temp.0, &first, [4., 0., 4.], 4)
            .unwrap_err()
            .contains("摘要")
    );
    std::fs::write(&path, &old).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(load_resident_volume(&temp.0, &first, [4., 0., 4.], 4).is_err());
    std::fs::write(path, old).unwrap();
    let mut unknown = first.clone();
    unknown.mesh_recipe = 2;
    assert!(encode_paged_volume(&unknown).is_err());
    let blocked = temp.0.join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    let before = load_edit_window(&temp.0, &first, brush([4., -2., 4.]), Arc::default()).unwrap();
    let after = apply_sparse_brush(&before, brush([4., -2., 4.]), bounds(), |_| Ok(0.07)).unwrap();
    assert!(
        write_delta(
            &blocked,
            &first,
            prepare_delta(Some(&before), &after).unwrap()
        )
        .is_err()
    );
    assert!(resident(&temp, &first, [4., 0., 4.]).mesh.is_some());
    assert!(!is_residency_budget_error("体积块被截断或摘要不一致"));
}
