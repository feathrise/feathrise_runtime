//! 固定高度代理的几何与身份回归；实际 Solari 管线另由硬件烟雾验证。

use super::{
    constants::MAX_PROXY_CELLS,
    mesh::{build_mesh, choose_grid, triangle_at},
    types::*,
};
use bevy::{
    mesh::{Indices, VertexAttributeValues},
    prelude::*,
};
use feathrise_scene_content::{SceneId, SparseHeightSpec, SparseHeightTile, SparseHeightTileKey};
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn input(spec: SparseHeightSpec) -> ProxyCapture {
    let grid = choose_grid(&spec).unwrap();
    ProxyCapture {
        stitches:Default::default(),
        holes: Arc::default(),
        identity: ProxyIdentity {
            scene_id: SceneId::parse("rt_proxy_test").unwrap(),
            root: Entity::from_raw_u32(1).unwrap(),
            display: Entity::from_raw_u32(2).unwrap(),
            manifest: Handle::<feathrise_scene_content::SparseHeightManifestAsset>::default().id(),
            material: Handle::<feathrise_environment::terrain_surface::TerrainSurfaceMaterial>::default()
                .id(),
            content: [1, 2, 3, 4, 5],
            live_revision: Some(3),
            crater_revision: Some(4),
            topology_revision: 0,
            ticket: 7,
        },
        source: Handle::default(),
        manifest: Handle::default(),
        spec,
        grid,
        refs: Vec::new(),
        formal: BTreeMap::new(),
        live: BTreeMap::new(),
        live_cursor: 0,
        live_count: 0,
        craters: BTreeMap::new(),
        crater_cursor: 0,
        crater_count: 0,
    }
}

#[test]
fn stitch_proxy_inserts_complete_shared_curve_and_preserves_existing_payload_budget() {
    use feathrise_environment::vegetation::blade::GpuBladeClipmapStitches;
    let mut source=input(SparseHeightSpec::new([0.;2],[8190.;2],1.,0.,32.).unwrap());
    let mut constraints=GpuBladeClipmapStitches::default();constraints.edges[1]=Vec4::new(257.,8190./256.,64.,1.);
    for i in 0..=256 {constraints.heights[1][i/4][i%4]=3.+(i as f32*0.13).sin()*2.;}
    source.stitches=Arc::new(constraints);
    source.grid=super::mesh::choose_grid_for_holes(&source.spec,false,true).unwrap();
    let prepared=super::mesh::build_proxy(&source).unwrap();let vertices=positions(&prepared.mesh);
    for i in 0..=256 {
        let z=(i as f32*constraints.edges[1].y).min(8190.);
        let p=vertices.iter().find(|p|p[0]==8190.&&p[2]==z).expect("共同曲线折点不能由粗代理跨过");
        assert!((p[1]-constraints.heights[1][i/4][i%4]).abs()<0.0001);
    }
    let triangles=prepared.mesh.indices().unwrap().len()/3;
    assert!(vertices.len()<=513*513&&triangles<=524_288);
    assert!(vertices.len()*48+triangles*12<=20*1024*1024);
}

fn positions(mesh: &Mesh) -> &Vec<[f32; 3]> {
    match mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
        VertexAttributeValues::Float32x3(values) => values,
        _ => panic!("需要真实三维格点"),
    }
}

#[test]
fn full_scene_proxy_uses_finest_bounded_mip_and_true_short_edge() {
    let input =
        input(SparseHeightSpec::new([-31.0, 12.0], [482.0, 79.0], 1.0, -10.0, 20.0).unwrap());
    assert_eq!(input.grid.level, 1);
    assert_eq!(input.grid.cells, [257, 34]);
    let mesh = build_mesh(&input).unwrap();
    let vertices = positions(&mesh);
    assert_eq!(vertices.first().unwrap(), &[-31.0, 0.0, 12.0]);
    assert_eq!(vertices.last().unwrap(), &[482.0, 0.0, 79.0]);
    assert_eq!(vertices.len(), 258 * 35);
    assert_eq!(
        triangle_at(&input.spec, input.grid, Vec2::new(482.0, 79.0)),
        257 * 34 * 2 - 1
    );
    assert_eq!(
        triangle_at(&input.spec, input.grid, Vec2::new(-30.5, 12.5)),
        0
    );
    assert_eq!(
        triangle_at(&input.spec, input.grid, Vec2::new(-29.1, 13.9)),
        1
    );
    let Indices::U32(indices) = mesh.indices().unwrap() else {
        panic!("代理使用U32")
    };
    assert_eq!(&indices[..6], &[0, 258, 1, 1, 258, 259]);
}

#[test]
fn maximum_scene_never_builds_author_mip_zero_or_exceeds_solari_payload() {
    let input =
        input(SparseHeightSpec::new([0.0, 0.0], [8192.0, 8192.0], 0.125, -256.0, 512.0).unwrap());
    assert_eq!(input.grid.level, 7);
    assert_eq!(input.grid.cells, [MAX_PROXY_CELLS as u32; 2]);
    let mesh = build_mesh(&input).unwrap();
    let vertices = mesh.count_vertices();
    let triangles = mesh.indices().unwrap().len() / 3;
    assert_eq!((vertices, triangles), (263169, 524288));
    assert!(vertices * 48 + triangles * 12 <= 20 * 1024 * 1024);
    assert_eq!(
        triangle_at(&input.spec, input.grid, Vec2::splat(8192.0)),
        0x7ffff
    );
}

#[test]
fn formal_parent_samples_are_real_heights_and_live_flat_overrides_old_hill() {
    let mut input =
        input(SparseHeightSpec::new([10.0, 20.0], [523.0, 84.0], 1.0, -10.0, 20.0).unwrap());
    let key = SparseHeightTileKey { x: 0, z: 0 };
    let mut parent = SparseHeightTile::blank_at_level(&input.spec, input.grid.level, key).unwrap();
    parent
        .set([1, 0], input.spec.encode_height(4.0).unwrap())
        .unwrap();
    input.formal.insert(key, Arc::new(parent));
    let mesh = build_mesh(&input).unwrap();
    assert!((positions(&mesh)[1][1] - 4.0).abs() < 0.001);
    let mut live = SparseHeightTile::blank(&input.spec, key).unwrap();
    live.set([0, 0], input.spec.encode_height(3.0).unwrap())
        .unwrap();
    input.live.insert(key, Arc::new(live));
    let mesh = build_mesh(&input).unwrap();
    assert!((positions(&mesh)[0][1] - 3.0).abs() < 0.001);
    assert_eq!(positions(&mesh)[1][1], 0.0); // 显式平页也必须遮蔽磁盘父页旧凸起。
    assert_eq!(positions(&mesh).last().unwrap(), &[523.0, 0.0, 84.0]);
}

#[test]
fn unbounded_float_raytracing_reads_formal_and_live_meter_codes() {
    let mut input = input(SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -8.0, 16.0).unwrap());
    let key = SparseHeightTileKey { x: 0, z: 0 };
    let mut formal = SparseHeightTile::blank_at_level(&input.spec, input.grid.level, key).unwrap();
    formal.set([0, 0], input.spec.encode_height(512.25).unwrap()).unwrap();
    input.formal.insert(key, Arc::new(formal));
    assert_eq!(positions(&build_mesh(&input).unwrap())[0][1], 512.25);
    let mut live = SparseHeightTile::blank(&input.spec, key).unwrap();
    live.set([0, 0], input.spec.encode_height(-384.5).unwrap()).unwrap();
    input.live.insert(key, Arc::new(live));
    let mesh = build_mesh(&input).unwrap();
    assert_eq!(positions(&mesh)[0][1], -384.5);
}

#[test]
fn identity_rejects_every_source_change_and_late_ticket() {
    let input = input(SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -10.0, 20.0).unwrap());
    let original = input.identity;
    for i in 0..5 {
        let mut changed = original.clone();
        changed.content[i] += 1;
        assert!(!original.same_source(&changed));
    }
    let mut late = original.clone();
    late.ticket += 1;
    assert!(original.same_source(&late));
    assert_ne!(original, late); // 后台采用同时核对完整身份和票据。
    let mut moved = original.clone();
    moved.display = Entity::from_raw_u32(3).unwrap();
    assert!(!original.same_source(&moved));
    // 身份没有相机坐标、吸附或页窗口；视点变化不构成新输入。
}

#[test]
fn bad_live_page_is_rejected_before_sampling_mesh() {
    let input = input(SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -10.0, 20.0).unwrap());
    let other = SparseHeightSpec::new([0.0; 2], [128.0; 2], 1.0, -10.0, 20.0).unwrap();
    let page = SparseHeightTile::blank(&other, SparseHeightTileKey { x: 0, z: 0 }).unwrap();
    assert!(super::mesh::validate_live_shape(&input, &page).is_err());
}

#[test]
fn sub_cell_cutout_is_exact_and_triangle_slots_match_expanded_axes() {
    use super::mesh::build_proxy;
    let mut source = input(SparseHeightSpec::new([0.; 2], [1024.; 2], 1., -10., 20.).unwrap());
    source.grid = super::mesh::choose_grid_for_holes(&source.spec, true, false).unwrap();
    // 原代理步长4米，洞口只有0.5米；不能粗暴删除整格或继续封洞。
    source.holes = Arc::from([feathrise_scene_content::SceneTerrainCutout {
        min: [10.25, 10.25],
        max: [10.75, 10.75],
    }]);
    let prepared = build_proxy(&source).unwrap();
    let verts = positions(&prepared.mesh);
    let Indices::U32(indices) = prepared.mesh.indices().unwrap() else {
        panic!("索引格式错误");
    };
    let mut area = 0.;
    let mut removed = 0;
    for triangle in indices.chunks_exact(3) {
        let p = triangle
            .iter()
            .map(|i| Vec3::from_array(verts[*i as usize]))
            .collect::<Vec<_>>();
        let cross = (p[1] - p[0]).cross(p[2] - p[0]);
        if cross.length_squared() == 0. {
            removed += 1;
            continue;
        }
        area += cross.y * 0.5;
        let center = (p[0] + p[1] + p[2]) / 3.;
        assert!(!source.holes[0].contains(center.xz()));
        let locate = |axis: &[Vec4; 129], v: f32, count: u32| {
            (0..count)
                .find(|i| v < axis[(*i as usize + 1) / 4][(*i as usize + 1) % 4])
                .unwrap_or(count - 1)
        };
        let [nx, nz] = prepared.grid.cells;
        let x = locate(&prepared.axes.x, center.x, nx);
        let z = locate(&prepared.axes.z, center.z, nz);
        let read = |axis: &[Vec4; 129], i: u32| axis[i as usize / 4][i as usize % 4];
        let u = (center.x - read(&prepared.axes.x, x))
            / (read(&prepared.axes.x, x + 1) - read(&prepared.axes.x, x));
        let v = (center.z - read(&prepared.axes.z, z))
            / (read(&prepared.axes.z, z + 1) - read(&prepared.axes.z, z));
        let expected = (2 * (z * nx + x) + u32::from(u + v > 1.)) as usize;
        assert_eq!(&indices[expected * 3..expected * 3 + 3], triangle); // 同生产WGSL编号，洞口占位未压缩其他三角。
    }
    assert_eq!(removed, 2);
    assert!((area - (1024. * 1024. - 0.25)).abs() < 0.01);
    let mut changed = source.identity.clone();
    changed.topology_revision += 1;
    assert!(!source.identity.same_source(&changed));
}

#[test]
fn session_craters_are_included_without_changing_author_page() {
    let mut input = input(SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -10.0, 20.0).unwrap());
    let mut craters = feathrise_scene_content::SparseHeightSessionCrater::default();
    craters.clear(Some(input.identity.scene_id.clone()));
    craters
        .apply(
            &input.spec,
            Vec2::new(5.0, 5.0),
            2.0,
            1.0,
            crate::scenes::dynamic_scene::session_crater::soil_surface_for_test(&input.spec),
        )
        .unwrap();
    input.craters = craters.tiles().clone();
    let mesh = build_mesh(&input).unwrap();
    assert!(positions(&mesh)[5 * 65 + 5][1] < -0.99);
    assert_eq!(positions(&mesh)[0][1], 0.0);
    assert!(input.live.is_empty() && input.formal.is_empty());
}
