//! 在真实ECS发布路径验证双方共同LOD、姿态预览暂停与取消恢复，不创建场景资产。
use super::*;
use feathrise_scene_content::*;
use std::collections::BTreeMap;

fn object(id: &str, spacing: f32, x: f32) -> SceneTerrainObject {
    let mut terrain = SceneTerrainDescriptor::empty();
    terrain.kind = SceneTerrainKind::SparseClipmap;
    terrain.height_path = Some(format!("scenes/test/{id}.scene_sparse_height.json"));
    terrain.resolution = [(64. / spacing) as u32 + 1; 2];
    SceneTerrainObject {
        id: TerrainId::parse(id).unwrap(),
        display_name: id.into(),
        enabled: true,
        world_min: [0.; 2],
        world_max: [64.; 2],
        transform: TerrainObjectTransform {
            translation: [x, 0., 0.],
            ..default()
        },
        terrain,
        mesh_lod: None,
        clipmap_stitches: Arc::default(),
        voxel: None,
        surface_path: None,
        vegetation_path: None,
        water_path: None,
        puddles: None,
    }
}
fn fixture() -> (App, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Followers>()
        .init_resource::<TerrainInstanceLookup>()
        .init_resource::<InstanceResourceTable<SparseHeightPageRuntime>>()
        .init_resource::<InstanceResourceTable<DynamicClipmapGroundSurface>>()
        .init_resource::<DynamicClipmapGroundSurface>()
        .init_resource::<Assets<ClipmapHeightMaterial>>()
        .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .add_systems(Update, publish);
    let (mut a, mut b) = (object("a", 0.5, 0.), object("b", 2., 64.));
    let (al, bl) = clipmap_stitch_pair(
        &a,
        ClipmapEdge::East,
        &b,
        ClipmapEdge::West,
        "s".into(),
        |d| Ok((d * std::f32::consts::PI / 64.).sin() * 3.),
    )
    .unwrap();
    a.clipmap_stitches = Arc::from([al]);
    b.clipmap_stitches = Arc::from([bl]);
    let objects = [a.clone(), b.clone()];
    a.terrain.resolved_stitches = resolve_clipmap_stitches(&a, &objects);
    b.terrain.resolved_stitches = resolve_clipmap_stitches(&b, &objects);
    let batch = app.world_mut().spawn_empty().id();
    let map = SceneId::parse("MAP-stitch-test").unwrap();
    let mut ids = Vec::new();
    let mut lookup = BTreeMap::new();
    for o in [a, b] {
        let path = SceneInstancePath::root(map.clone()).terrain(o.id.clone());
        let mut pages = SparseHeightPageRuntime::default();
        pages.seed_stitches(&o.terrain.resolved_stitches);
        let e = app
            .world_mut()
            .spawn((
                SceneSourceFrame::from_transform(o.transform.as_transform()),
                TerrainObjectInstance {
                    definition: Arc::new(o),
                },
                SubsceneInstance {
                    path: path.clone(),
                    source_scene_id: map.clone(),
                    batch,
                    generation: 1,
                },
                StitchLodStamp(Vec4::splat(4.)),
            ))
            .id();
        app.world_mut()
            .resource_mut::<InstanceResourceTable<SparseHeightPageRuntime>>()
            .values
            .insert(e, pages);
        let mut ground = DynamicClipmapGroundSurface::default();
        ground.snapshot = Some(super::super::test_ground_snapshot());
        app.world_mut()
            .resource_mut::<InstanceResourceTable<DynamicClipmapGroundSurface>>()
            .values
            .insert(e, ground);
        ids.push(e);
        lookup.insert(path, e);
    }
    app.world_mut()
        .resource_mut::<TerrainInstanceLookup>()
        .test_batch(batch, lookup);
    (app, ids[0], ids[1])
}
#[test]
fn stitch_publication_uses_shared_lod_and_freezes_local_shape_during_pose_preview() {
    let (mut app, a, b) = fixture();
    app.insert_resource(
        super::super::super::instances::TerrainObjectAuthoringTarget::test_applied(
            SceneId::parse("MAP-stitch-test").unwrap(),
            TerrainId::parse("a").unwrap(),
            a,
        ),
    );
    app.update();
    assert_eq!(app.world().get::<StitchStatus>(a).unwrap().active, 1);
    app.world_mut().entity_mut(b).remove::<StitchLodStamp>();
    app.world_mut()
        .entity_mut(a)
        .insert(StitchLodStamp(Vec4::splat(2.)));
    app.update();
    assert_eq!(encode(app.world(), a).edges[1].w, 32.);
    assert_eq!(encode(app.world(), b).edges[0].w, 32.);
    app.world_mut()
        .entity_mut(b)
        .insert(StitchLodStamp(Vec4::splat(4.)));
    app.update();
    app.world_mut()
        .entity_mut(a)
        .insert(StitchLodStamp(Vec4::splat(16.)));
    app.update();
    let read = |app: &App, e: Entity| {
        app.world()
            .resource::<InstanceResourceTable<SparseHeightPageRuntime>>()
            .values[&e]
            .stitches
            .clone()
    };
    let first = read(&app, a);
    let second = read(&app, b);
    for e in [a, b] {
        let table=app.world().resource::<InstanceResourceTable<DynamicClipmapGroundSurface>>();
        let snapshot=table.values[&e].snapshot.as_ref().unwrap();
        let (edge,x)=if e==a {(1,96.)}else{(0,0.)};
        let super::super::ClipmapGroundHit::Hit(point)=snapshot.sample_surface(Vec2::new(x,16.)) else{panic!("所选网格仍须从实例快照命中接缝");};
        assert!((point.y-read(&app,e).sample(edge,16.)).abs()<0.001);
    }
    assert!(
        app.world()
            .resource::<DynamicClipmapGroundSurface>()
            .snapshot
            .is_none()
    );
    assert_eq!(first.edges[1].w, 16.);
    assert_eq!(first.edges[1], second.edges[0]);
    for i in 0..=64 {
        assert!((first.sample(1, i as f32) - second.sample(0, i as f32)).abs() < 0.00001);
    }
    app.world_mut()
        .entity_mut(b)
        .insert(SceneSourceFrame::from_transform(Transform::from_xyz(
            65., 0., 0.,
        )));
    app.update();
    assert_eq!(app.world().get::<StitchStatus>(a).unwrap().active, 0);
    assert_eq!(*read(&app, a), *first);
    assert_eq!(*read(&app, b), *second);
    app.world_mut()
        .entity_mut(b)
        .insert(SceneSourceFrame::from_transform(Transform::from_xyz(
            64., 0., 0.,
        )));
    app.update();
    assert_eq!(app.world().get::<StitchStatus>(a).unwrap().active, 1);
    app.world_mut()
        .entity_mut(a)
        .insert(StitchLodStamp(Vec4::splat(2.)));
    app.update();
    assert_eq!(read(&app, a).edges[1].w, 4.);
    assert_eq!(read(&app, b).edges[0].w, 4.);
    // 端点不变的Y缩放仍必须检查中间曲线，暂停而不能拉开裂口。
    app.world_mut()
        .entity_mut(b)
        .insert(SceneSourceFrame::from_transform(
            Transform::from_xyz(64., 0., 0.).with_scale(Vec3::new(1., 2., 1.)),
        ));
    app.update();
    assert_eq!(app.world().get::<StitchStatus>(a).unwrap().active, 0);
}
#[test]
fn stitch_follower_reassignment_cleans_old_owner() {
    let mut index = Followers::default();
    let a = Entity::from_bits(1);
    let b = Entity::from_bits(2);
    let leaf = Entity::from_bits(3);
    index.assign(leaf, a);
    index.assign(leaf, b);
    assert!(!index.by_root.contains_key(&a));
    assert_eq!(index.by_root[&b], BTreeSet::from([leaf]));
    index.remove(leaf);
    assert!(index.by_entity.is_empty());
    assert!(index.by_root.is_empty());
}

#[test]
fn stitch_real_fine_and_coarse_mesh_edges_cover_the_same_knots_across_phases() {
    use super::super::ClipmapConfig;
    // 分别生成真正的0.5米和2米环带，而非用采样函数自身充当拓扑真值。
    for focus in [
        Vec2::new(31.5, 32.5),
        Vec2::new(32., 32.),
        Vec2::new(35.5, 29.5),
    ] {
        let mut outlines = Vec::new();
        for (spacing, levels, edge) in [(0.5, 5, 1_usize), (2., 3, 0_usize)] {
            let mesh = ClipmapConfig {
                finest_cell_size: Vec2::splat(spacing),
                half_cells: 8,
                levels,
            }
            .build_combined_mesh()
            .unwrap();
            let positions = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .unwrap()
                .as_float3()
                .unwrap();
            let bevy::mesh::VertexAttributeValues::Float32x2(lods) =
                mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
            else {
                panic!("真实LOD属性");
            };
            let mut constraints = GpuBladeClipmapStitches::default();
            constraints.edges[edge] = Vec4::new(33., 2., 8., 8.);
            let vertices: Vec<_> = positions
                .iter()
                .zip(lods)
                .map(|(p, l)| {
                    let step = spacing * 2_f32.powi(l[0] as i32);
                    let center = (focus / step + Vec2::splat(0.5)).floor() * step;
                    constraints.position(
                        Vec2::new(p[0], p[2]) + center,
                        Vec2::ZERO,
                        Vec2::splat(64.),
                    )
                })
                .collect();
            let mut segments = BTreeSet::new();
            let indices: Vec<_> = mesh.indices().unwrap().iter().collect();
            let bound = if edge == 0 { 0. } else { 64. };
            for ids in indices.chunks_exact(3) {
                let p = [vertices[ids[0]], vertices[ids[1]], vertices[ids[2]]];
                if (p[1] - p[0]).perp_dot(p[2] - p[0]).abs() < 0.00001 {
                    continue;
                }
                for i in 0..3 {
                    let (a, b) = (p[i], p[(i + 1) % 3]);
                    if a.x == bound && b.x == bound && a.y != b.y {
                        segments.insert((a.y.min(b.y) as u32, a.y.max(b.y) as u32));
                    }
                }
            }
            assert_eq!(
                segments,
                BTreeSet::from([(0, 16), (16, 32), (32, 48), (48, 64)]),
                "{spacing} {focus:?}"
            );
            outlines.push(segments);
        }
        assert_eq!(outlines[0], outlines[1]);
    }
}
