//! 真正移动到精细范围外后洞口仍可见；代理往返、版本换代和损坏拒收都用临时库。
use super::super::{
    encode_paged_volume,
    tests::{Temp, edit, resident},
};
use super::*;
use crate::TerrainMeshInput;
use bevy::{prelude::*, tasks::block_on};

fn top(mesh: &TerrainMeshInput, x: f32, z: f32) -> f32 {
    let p = Vec2::new(x, z);
    let mut top = f32::NEG_INFINITY;
    for tri in mesh.indices.chunks_exact(3) {
        let [a, b, c] =
            [tri[0], tri[1], tri[2]].map(|i| Vec3::from_array(mesh.positions[i as usize]));
        let determinant = (b.xz() - a.xz()).perp_dot(c.xz() - a.xz());
        if determinant.abs() < 1e-8 {
            continue;
        }
        let u = (p - a.xz()).perp_dot(c.xz() - a.xz()) / determinant;
        let v = (b.xz() - a.xz()).perp_dot(p - a.xz()) / determinant;
        if u >= -1e-5 && v >= -1e-5 && u + v <= 1. + 1e-5 {
            top = top.max(a.y + u * (b.y - a.y) + v * (c.y - a.y));
        }
    }
    top
}
#[test]
fn volume_display_unloaded_fine_chunks_keep_both_cave_openings_and_exact_gap() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let temp = Temp::new();
    let first = edit(&temp, &PagedVolume::new(0.25).unwrap(), [4., 0., 4.]);
    let source = edit(&temp, &first, [164., 0., 164.]);
    let bytes = encode_paged_volume(&source).unwrap();
    assert!(resident(&temp, &source, [700., 0., 700.]).mesh.is_none());
    let far = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap();
    assert_eq!(far.footprint.len(), 2);
    assert!(
        far.footprint
            .iter()
            .all(|r| r[1][0] <= 8. || r[0][0] >= 160.)
    );
    let mesh = far.mesh.unwrap();
    assert!(top(&mesh, 4., 4.) < -1.0, "远处洞口不能被还原成0.07m高度面");
    assert!(
        top(&mesh, 164., 164.) < -1.0,
        "另一处洞穴不能因精细块卸载而消失"
    );
    assert!(
        top(&mesh, 80., 80.).is_infinite(),
        "两个洞穴中间应继续显示原Clipmap"
    );
    let near = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[4., 0., 4.]],
        4,
    ))
    .unwrap();
    assert_eq!(near.footprint, far.footprint);
    assert!(
        (top(near.mesh.as_ref().unwrap(), 4., 4.)
            - top(
                &resident(&temp, &source, [4., 0., 4.]).mesh.unwrap(),
                4.,
                4.
            ))
        .abs()
            < 1e-5
    );
    let again = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap();
    assert_eq!(again.mesh.as_ref(), Some(&mesh));
    assert_eq!(encode_paged_volume(&source).unwrap(), bytes);
    let next = edit(&temp, &source, [4., -1., 4.]);
    let changed = block_on(load_display_volume_at_foci(
        &temp.0,
        &next,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap();
    assert!(top(changed.mesh.as_ref().unwrap(), 4., 4.) < top(&mesh, 4., 4.) - 0.5);
    assert_eq!(
        block_on(load_display_volume_at_foci(
            &temp.0,
            &source,
            &[[700., 0., 700.]],
            4
        ))
        .unwrap()
        .mesh,
        Some(mesh)
    );
}
#[test]
fn volume_display_cache_corruption_never_publishes_a_different_or_empty_source() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let temp = Temp::new();
    let source = edit(&temp, &PagedVolume::new(0.25).unwrap(), [4., 0., 4.]);
    let before = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap()
    .mesh;
    let key = cache::Key {
        source: source.meshes,
        depth: 0,
        prefix: 0,
        cell_size: source.cell_size,
    };
    let hash = cache::read_ref(&temp.0, key).unwrap().unwrap();
    let path = temp
        .0
        .join(format!("{}.vblk", super::super::store::hex(hash)));
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    let error = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap_err();
    assert!(error.contains("摘要") || error.contains("截断"));
    assert!(!is_residency_budget_error(&error));
    std::fs::write(path, bytes).unwrap();
    assert_eq!(
        block_on(load_display_volume_at_foci(
            &temp.0,
            &source,
            &[[700., 0., 700.]],
            4
        ))
        .unwrap()
        .mesh,
        before
    );
    // 摘要本身正确、但内容被换成另一份源身份，也必须在注册网格前拒收。
    let mut foreign = std::fs::read(
        temp.0
            .join(format!("{}.vblk", super::super::store::hex(hash))),
    )
    .unwrap();
    foreign[4] ^= 1;
    let foreign_hash = super::super::store::Store::new(&temp.0)
        .write(&foreign)
        .unwrap();
    let reference = std::fs::read_dir(&temp.0)
        .unwrap()
        .map(|p| p.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("lod1-0-")
        })
        .unwrap();
    std::fs::write(&reference, foreign_hash).unwrap();
    let error = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap_err();
    assert!(error.contains("源版本"));
    std::fs::write(reference, hash).unwrap();
}

#[test]
fn volume_display_mixed_resolution_cave_crossing_keeps_shared_edges_and_floor() {
    use std::collections::HashMap;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let temp = Temp::new();
    let source = edit(&temp, &PagedVolume::new(0.25).unwrap(), [15.75, 0., 4.]);
    let input = load_resident_volume_at_foci(&temp.0, &source, &[[4., 0., 4.]], 1).unwrap();
    assert!(input.columns.contains(&[1, 0]) && !input.columns.contains(&[2, 0]));
    let precise = resident(&temp, &source, [15.75, 0., 4.]);
    let mixed = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[4., 0., 4.]],
        1,
    ))
    .unwrap();
    assert_eq!(mixed.footprint, precise.footprint);
    let mesh = mixed.mesh.unwrap();
    assert!(top(&mesh, 15.25, 4.) < -1. && top(&mesh, 16.25, 4.) < -1.);
    let mut edges = HashMap::new();
    for t in mesh.indices.chunks_exact(3) {
        for [a, b] in [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]] {
            *edges.entry([a.min(b), a.max(b)]).or_insert(0) += 1;
        }
    }
    // x=16m是精细/粗代理交界，不是作者层的外缘；此处每条边都必须保持二面相邻。
    let seam: Vec<_> = edges
        .iter()
        .filter(|(edge, _)| {
            edge.iter().all(|i| {
                let p = mesh.positions[*i as usize];
                p[0] == 16. && p[2] > 1. && p[2] < 7.
            })
        })
        .collect();
    assert!(!seam.is_empty());
    assert!(
        seam.iter().all(|(_, n)| **n == 2),
        "精细/粗代理之间不能出现裂缝或重复面"
    );
}

#[test]
fn volume_display_small_cave_and_floating_fill_survive_unloading_and_coarsening() {
    use super::super::{load_edit_window, prepare_delta, write_delta};
    use crate::volume::{VolumeBrush, VolumeBrushKind, sparse::*};
    use std::sync::Arc;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let temp = Temp::new();
    let mut source = PagedVolume::new(0.25).unwrap();
    for (kind, point) in [
        (VolumeBrushKind::Dig, [4., 0., 4.]),
        (VolumeBrushKind::Fill, [7.5, 1., 4.]),
    ] {
        let brush = VolumeBrush {
            kind,
            from: point,
            to: point,
            radius: 0.5,
            strength: 1.,
            ..Default::default()
        };
        let before = load_edit_window(&temp.0, &source, brush, Arc::default()).unwrap();
        let after = apply_sparse_brush(
            &before,
            brush,
            Rect::from_corners(Vec2::splat(-64.), Vec2::splat(64.)),
            |_| Ok(0.07),
        )
        .unwrap();
        source = write_delta(
            &temp.0,
            &source,
            prepare_delta(Some(&before), &after).unwrap(),
        )
        .unwrap();
    }
    let far = block_on(load_display_volume_at_foci(
        &temp.0,
        &source,
        &[[700., 0., 700.]],
        4,
    ))
    .unwrap()
    .mesh
    .unwrap();
    let near = resident(&temp, &source, [4., 0., 4.]).mesh.unwrap();
    assert!(top(&far, 4., 4.) < -0.2, "小洞不能因远景减面被封回高度面");
    assert!(
        top(&far, 7.5, 4.) > 1.25,
        "孤立的小填充块也必须继续显示和拾取"
    );
    for [x, z] in [[4., 4.], [7.5, 4.]] {
        assert!((top(&far, x, z) - top(&near, x, z)).abs() <= 0.25);
    }
}
