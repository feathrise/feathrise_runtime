//! 重现大半径连续雕刻的几何门槛，并验证简化不拆开洞顶、共享边或旧文件身份。
use super::{brush::rebuild_all, types::MeshRecipe, *};
use crate::{
    TerrainMeshInput,
    volume::{VolumeBrush, VolumeBrushKind},
};
use bevy::math::{Rect, Vec2};
use std::{collections::HashMap, sync::Arc};

fn bounds() -> Rect {
    Rect::from_corners(Vec2::splat(-128.), Vec2::splat(128.))
}
fn brush(point: [f32; 3], radius: f32) -> VolumeBrush {
    VolumeBrush {
        kind: VolumeBrushKind::Dig,
        from: point,
        to: point,
        radius,
        strength: 1.,
        ..Default::default()
    }
}
fn compact(mut data: SparseVolume) -> SparseVolume {
    data.mesh_recipe = MeshRecipe::CompactV1;
    rebuild_all(&mut data).unwrap();
    data
}

#[test]
fn volume_mesh_compaction_reproduces_large_brush_budget_failure_and_keeps_headroom() {
    let mut legacy = new_sparse_volume(0.25, Arc::default()).unwrap();
    for x in [0., 16., 32., 48.] {
        legacy =
            apply_sparse_brush(&legacy, brush([x, 0.07, 0.], 8.), bounds(), |_| Ok(0.07)).unwrap();
    }
    // 有效编辑会升级旧配方；完成夹具后按旧配方重建，复现同一密度场的原预算门。
    legacy.mesh_recipe = MeshRecipe::Legacy;
    rebuild_all(&mut legacy).unwrap();
    let raw_triangles: usize = legacy.meshes.values().map(|m| m.indices.len() / 3).sum();
    assert!(
        raw_triangles > super::types::MAX_SPARSE_TRIANGLES,
        "夹具必须越过原预算：{raw_triangles}"
    );
    assert!(
        build_author_volume_mesh(&VolumeSource::Sparse(legacy.clone()))
            .unwrap_err()
            .contains("三角碰撞预算")
    );
    let compact = compact(legacy.clone());
    let mesh = build_author_volume_mesh(&VolumeSource::Sparse(compact.clone())).unwrap();
    assert!(mesh.indices.len() / 3 < raw_triangles / 4);
    assert!(mesh.indices.len() / 3 < super::types::MAX_SPARSE_TRIANGLES / 2);
    let footprint = compact.footprint()[0];
    assert_eq!(compact.footprint().len(), 1);
    super::tests::assert_welded(&mesh, footprint[0], footprint[1]);
    for x in [0, 64, 128, 192] {
        assert_eq!(
            legacy.value_at([x, -16, 0]).unwrap(),
            compact.value_at([x, -16, 0]).unwrap()
        );
        assert!(compact.value_at([x, -16, 0]).unwrap() > 0.);
    }
    let bytes = encode_author_volume_glb(&VolumeSource::Sparse(compact.clone()), &mesh).unwrap();
    let reopened = decode_author_volume_glb(&bytes, Arc::default()).unwrap();
    assert_eq!(reopened, VolumeSource::Sparse(compact));
    assert_eq!(build_author_volume_mesh(&reopened).unwrap(), mesh);
    eprintln!(
        "大半径雕刻：原{raw_triangles}三角，简化{}三角/{}顶点",
        mesh.indices.len() / 3,
        mesh.positions.len()
    );
}

fn open_edges(mesh: &TerrainMeshInput) -> std::collections::HashSet<[[u32; 3]; 2]> {
    let mut edges = HashMap::new();
    for tri in mesh.indices.chunks_exact(3) {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            *edges.entry([a.min(b), a.max(b)]).or_insert(0) += 1;
        }
    }
    edges
        .into_iter()
        .filter(|(_, n)| *n == 1)
        .map(|(edge, _)| {
            let mut edge = edge
                .map(|i| mesh.positions[i as usize].map(|v| if v == 0. { 0 } else { v.to_bits() }));
            edge.sort();
            edge
        })
        .collect()
}

#[test]
fn volume_mesh_compaction_preserves_boundaries_roofs_and_small_filled_components() {
    let mut legacy = new_sparse_volume(0.25, Arc::default()).unwrap();
    legacy =
        apply_sparse_brush(&legacy, brush([0., 0.07, 0.], 2.), bounds(), |_| Ok(0.07)).unwrap();
    legacy = apply_sparse_brush(
        &legacy,
        VolumeBrush {
            to: [8., -3., 0.],
            ..brush([0., -3., 0.], 2.)
        },
        bounds(),
        |_| Ok(0.07),
    )
    .unwrap();
    // 洞内的小填充组件也必须保留，不能为降低数量顺便删掉小岩块。
    legacy = apply_sparse_brush(
        &legacy,
        VolumeBrush {
            kind: VolumeBrushKind::Fill,
            ..brush([4., -3., 0.], 0.5)
        },
        bounds(),
        |_| Ok(0.07),
    )
    .unwrap();
    legacy.mesh_recipe = MeshRecipe::Legacy;
    rebuild_all(&mut legacy).unwrap();
    let raw = build_author_volume_mesh(&VolumeSource::Sparse(legacy.clone())).unwrap();
    let source = compact(legacy);
    let mesh = build_author_volume_mesh(&VolumeSource::Sparse(source.clone())).unwrap();
    assert_eq!(
        open_edges(&raw),
        open_edges(&mesh),
        "外缘和所有分块接缝不移动、不删点"
    );
    let footprint = source.footprint()[0];
    super::tests::assert_welded(&mesh, footprint[0], footprint[1]);
    let raw_index = crate::build_terrain_editing_index(&raw).unwrap();
    let index = crate::build_terrain_editing_index(&mesh).unwrap();
    let mut rays = Vec::new();
    for x in -4..=12 {
        for z in -3..=3 {
            rays.push(([x as f32, 8., z as f32], [0., -1., 0.]));
        }
    }
    // 真实洞顶、洞底、小岩块与沿洞道的侧壁都保持原拾取几何；容差小于一个体素格距。
    rays.extend([
        ([6., -3., 0.], [0., 1., 0.]),
        ([6., -3., 0.], [0., -1., 0.]),
        ([6., -3., 0.], [0., 0., 1.]),
        ([4., -4., 0.], [0., 1., 0.]),
    ]);
    for (from, direction) in rays {
        let a = raw_index
            .ray_intersection(&raw.positions, &raw.indices, from, direction)
            .unwrap();
        let b = index
            .ray_intersection(&mesh.positions, &mesh.indices, from, direction)
            .unwrap();
        assert!(
            (a.position[0] - b.position[0])
                .abs()
                .max((a.position[1] - b.position[1]).abs())
                .max((a.position[2] - b.position[2]).abs())
                < 0.0625,
            "真实射线偏差过大：{a:?}/{b:?}"
        );
    }
}

#[test]
fn volume_mesh_recipe_reopens_legacy_v2_exactly_and_upgrades_only_on_valid_edit() {
    let initial = new_sparse_volume(0.25, Arc::default()).unwrap();
    let mut legacy =
        apply_sparse_brush(&initial, brush([0., 0.07, 0.], 2.), bounds(), |_| Ok(0.07)).unwrap();
    legacy.mesh_recipe = MeshRecipe::Legacy;
    rebuild_all(&mut legacy).unwrap();
    let raw = build_author_volume_mesh(&VolumeSource::Sparse(legacy.clone())).unwrap();
    let bytes = encode_author_volume_glb(&VolumeSource::Sparse(legacy.clone()), &raw).unwrap();
    let reopened = decode_author_volume_glb(&bytes, Arc::default()).unwrap();
    assert_eq!(build_author_volume_mesh(&reopened).unwrap(), raw);
    let VolumeSource::Sparse(reopened) = reopened else {
        panic!()
    };
    assert_eq!(reopened.mesh_recipe, MeshRecipe::Legacy);
    let unchanged = apply_sparse_brush(
        &reopened,
        VolumeBrush {
            kind: VolumeBrushKind::Smooth,
            ..brush([40., 0.07, 40.], 2.)
        },
        bounds(),
        |_| Ok(0.07),
    )
    .unwrap();
    assert_eq!(unchanged.mesh_recipe, MeshRecipe::Legacy);
    assert_eq!(
        build_author_volume_mesh(&VolumeSource::Sparse(unchanged)).unwrap(),
        raw
    );
    let next =
        apply_sparse_brush(&reopened, brush([0., -2., 0.], 2.), bounds(), |_| Ok(0.07)).unwrap();
    assert_eq!(next.mesh_recipe, MeshRecipe::CompactV1);
    let next = VolumeSource::Sparse(next);
    let mesh = build_author_volume_mesh(&next).unwrap();
    let next_bytes = encode_author_volume_glb(&next, &mesh).unwrap();
    assert_eq!(
        build_author_volume_mesh(&decode_author_volume_glb(&next_bytes, Arc::default()).unwrap())
            .unwrap(),
        mesh
    );
    assert_eq!(
        build_author_volume_mesh(&VolumeSource::Sparse(reopened)).unwrap(),
        raw,
        "上一版本仍可原样撤销"
    );
    let (mut document, binary) = crate::glb::parse_glb_chunks(&next_bytes).unwrap();
    document["asset"]["extras"]["rpg_editor_volume_v2"]["mesh_recipe"] = serde_json::json!(999);
    let damaged =
        crate::volume::codec::pack(document, binary, MAX_AUTHOR_VOLUME_GLB_BYTES).unwrap();
    let budget: Arc<VolumeMemoryBudget> = Arc::default();
    assert!(
        decode_author_volume_glb(&damaged, budget.clone())
            .unwrap_err()
            .contains("配方未知")
    );
    assert_eq!(budget.used_bytes(), 0);
}
