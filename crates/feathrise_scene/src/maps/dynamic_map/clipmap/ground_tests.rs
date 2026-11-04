//! 精确笔刷查询对照正式组合Mesh，避免用查询自身的拓扑作为测试真值。
use super::super::ClipmapConfig;
use super::super::authority::{AuthorityHeightSnapshot, AuthorityIdentity, AuthorityPage};
use super::*; // 读取内部量化页夹具。 // 正式显示拓扑。
use feathrise_map_content::{SparseHeightSpec, SparseHeightTile, SparseHeightTileKey};
use std::collections::BTreeMap;

fn fixed_bands(half_cells: u32, levels: u32) -> [GpuBladeClipmapBand; 16] {
    // 旧固定模板只用于验证新可变描述的等价路径。
    let mut bands = [GpuBladeClipmapBand::default(); 16];
    for (level, band) in bands.iter_mut().enumerate().take(levels as usize) {
        *band = GpuBladeClipmapBand {
            inner_half_cells: if level == 0 {
                UVec2::ZERO
            } else {
                UVec2::splat(half_cells / 2)
            },
            outer_half_cells: UVec2::splat(half_cells),
            geometry_mip: level as u32,
        };
    }
    bands
}

fn fixture_height_code(x: f32, z: f32) -> u16 {
    let height = 12.0 + (x * 0.13).sin() * 4.0 + (z * 0.17).cos() * 3.0 + x * z * 0.0001;
    ((height / 32.0 * 65535.0).round()) as u16
}

pub(super) fn snapshot() -> ClipmapGroundSnapshot {
    // 非方形地图与非线性高度。
    let heights = (0..257 * 257)
        .flat_map(|i| {
            let x = (i % 257) as f32;
            let z = (i / 257) as f32;
            fixture_height_code(x, z).to_le_bytes()
        })
        .collect(); // R16坡面、鞍形和频率变化。
    let mut table = Vec::new(); // 五级各单页，复用同一层但采样位置随Mip变化。
    for _ in 0..5 {
        for word in [1_u32, 0.0_f32.to_bits(), 32.0_f32.to_bits(), 0] {
            table.extend_from_slice(&word.to_le_bytes());
        }
    } // 可辨认的每级表入口。
    let height = GpuBladeClipmapHeight {stitches: Default::default(),
        holes: [Vec4::ZERO; 32],
        hole_count: 0,
        atlas: Handle::default(),
        page_table: Handle::default(),
        world_min: Vec2::ZERO,
        world_max: Vec2::new(96.0, 64.0),
        cell_size: Vec2::ONE,
        logical_cells: UVec2::new(96, 64),
        max_mip: 4,
        fine_center: Vec2::new(47.0, 31.0),
        bands: fixed_bands(8, 5),
        band_count: 5,
        height_min: 0.0,
        height_max: 32.0,
        revision: [1, 2, 3],
        missing_pages: [UVec4::ZERO; 16],
        ready: true,
    }; // 故意奇数中心使相邻环带独立吸附。
    let content_revision = ClipmapGroundContentRevision {
        root: 1,
        manifest: 2,
        live: None,
        crater: None,
        topology: 3,
    };
    let spec = SparseHeightSpec::new([0.0, 0.0], [96.0, 64.0], 1.0, 0.0, 32.0).unwrap();
    let key = SparseHeightTileKey { x: 0, z: 0 };
    let mut authority_tile = SparseHeightTile::blank(&spec, key).unwrap();
    for z in 0..=64_u16 {
        for x in 0..=96_u16 {
            // 这份旧R16夹具的CPU/GPU必须仍表示同一源值；新encode本身已不量化。
            // 显式把GPU字节解回米制再存f32，避免无关水深回归混入两个不同精度的地形。
            let height = f32::from(fixture_height_code(f32::from(x), f32::from(z))) / 65535.0 * 32.0;
            authority_tile
                .set([x, z], spec.encode_height(height).unwrap())
                .unwrap();
        }
    }
    ClipmapGroundSnapshot {
        author_stitches:Default::default(),
        holes: Arc::default(),
        terrain_meshes: Arc::default(),
        hole_revision: 0,
        map_id: MapId::parse("MAP-ground-test").unwrap(),
        revision: height.revision,
        missing_pages: height.missing_pages,
        ready: true,
        content_revision,
        height,
        pages: Some(Arc::new(GroundPageBytes {
            heights,
            table,
            width: 1,
            height_bounds: [0.0, 32.0],
        })),
        authority: Some(Arc::new(AuthorityHeightSnapshot {
            identity: AuthorityIdentity {
                map_id: MapId::parse("MAP-ground-test").unwrap(),
                content: content_revision,
            },
            revision: 1,
            spec,
            pages: BTreeMap::from([(key, AuthorityPage::Tile(Arc::new(authority_tile)))]),
            craters: BTreeMap::new(),
        })),
        authority_requests: AuthorityRequestSink::default(),
    } // 查询只读输入。
} // 不创建运行时World。

#[test]
fn legacy_fixture_authority_and_gpu_base_points_share_identical_meter_values() {
    let ground = snapshot();
    let authority = ground.authority.as_ref().unwrap();
    for z in 0..=64 {
        for x in 0..=96 {
            let point = Vec2::new(x as f32, z as f32);
            let ClipmapGroundHit::Hit(author) = authority.sample_surface(point) else {
                panic!("完整夹具不能缺作者格点：{point:?}");
            };
            assert_eq!(ground.sample(point, 0), Some(author.y), "夹具CPU/GPU米制值必须一致：{point:?}");
        }
    }
}

#[test]
fn view_revision_changes_query_identity_without_changing_height_content() {
    let mut ground = snapshot();
    let content = ground.content_revision();
    let no_live_words = ground.content_revision_words();
    let query = ground.query_revision();
    ground.revision[2] = ground.revision[2].wrapping_add(1); // 模拟相机环带吸附中心换代。
    assert_eq!(ground.content_revision(), content); // Mesh常驻块不能随视角移动整片重建。
    assert_ne!(ground.query_revision(), query); // 缺页等待块仍须看到新窗口并重试。
    ground.content_revision.live = Some(0); // 第0作者修订是合法身份，不能与没有F12覆盖混同。
    assert_eq!(ground.authoring_height_revision(), Some(0));
    assert_ne!(ground.content_revision_words(), no_live_words); // presence位参与分帧摆放失效门。
}

#[test]
fn cave_opening_picks_real_floor_wall_and_overhang_then_restores_height_plane() {
    use crate::maps::dynamic_map::terrain_topology::TerrainMeshQuery;
    use avian3d::prelude::Collider;
    let mut ground = snapshot();
    let vertices = vec![
        Vec3::new(-2., 0., -2.),
        Vec3::new(-2., 0., 2.),
        Vec3::new(2., 0., 2.),
        Vec3::new(2., 0., -2.),
        Vec3::new(-2., 4., -2.),
        Vec3::new(-2., 4., 2.),
        Vec3::new(-1., 4., 2.),
        Vec3::new(-1., 4., -2.),
    ];
    // 地板、竖直壁和只覆盖左侧的倒扣；不能用单值高度代理替代该拓扑。
    let shape = Collider::trimesh(
        vertices,
        vec![
            [0, 1, 2],
            [0, 2, 3],
            [0, 4, 5],
            [0, 5, 1],
            [4, 5, 6],
            [4, 6, 7],
        ],
    );
    ground.holes = Arc::from([feathrise_map_content::MapTerrainCutout {
        min: [20., 10.],
        max: [24., 14.],
    }]);
    ground.terrain_meshes =
        Arc::from([
            TerrainMeshQuery::new(shape, Mat4::from_translation(Vec3::new(22., 0., 12.))).unwrap(),
        ]);
    assert_eq!(
        ground.sample_authoritative_surface(Vec2::new(22., 12.)),
        ClipmapGroundHit::Miss
    );
    assert_eq!(
        ground.height_ray_intersection(Vec3::new(22., 50., 12.), -Vec3::Y),
        ClipmapGroundHit::Miss
    );
    let expected = Vec3::new(22., 0., 12.);
    assert_eq!(
        ground.ray_intersection(Vec3::new(22., 50., 12.), -Vec3::Y),
        ClipmapGroundHit::Hit(expected)
    );
    assert_eq!(
        ground.sample_authoritative_hit(expected),
        ClipmapGroundHit::Hit(expected)
    );
    assert_eq!(
        ground.ray_intersection(Vec3::new(23., 2., 12.), -Vec3::X),
        ClipmapGroundHit::Hit(Vec3::new(20., 2., 12.))
    );
    assert_eq!(
        ground.ray_intersection(Vec3::new(20.5, 50., 12.), -Vec3::Y),
        ClipmapGroundHit::Hit(Vec3::new(20.5, 4., 12.))
    );
    // 保存撤销后的完整状态没有孤立洞或旧模型查询。
    ground.holes = Arc::default();
    ground.terrain_meshes = Arc::default();
    assert!(
        matches!(ground.ray_intersection(Vec3::new(22.,50.,12.),-Vec3::Y),ClipmapGroundHit::Hit(point) if point.y>5.)
    );
}

#[test]
fn author_platform_rejects_local_session_craters_and_stale_authority() {
    use feathrise_map_content::{SparseHeightSessionCrater, SurfaceCraterResponse};
    let mut ground = snapshot();
    let spec = ground.authority.as_ref().unwrap().spec.clone();
    let soil = Arc::new(
        SurfaceCraterResponse::new(
            Vec2::ZERO,
            Vec2::new(96., 64.),
            [1, 1],
            Arc::from([0; 16]),
            Arc::from([255, 0, 0, 0]),
            vec![1.],
        )
        .unwrap(),
    );
    let mut craters = SparseHeightSessionCrater::default();
    craters.clear(Some(ground.map_id.clone()));
    craters.apply(&spec, Vec2::splat(8.), 2., 1., soil).unwrap();
    let authority = Arc::get_mut(ground.authority.as_mut().unwrap()).unwrap();
    authority.craters = craters.tiles().clone(); // 真实会话笔印冻结，作者高度页保持不变。
    assert!(
        ground
            .session_crater_intersects_rect(Vec2::splat(7.), Vec2::splat(9.))
            .unwrap()
    );
    assert!(
        !ground
            .session_crater_intersects_rect(Vec2::splat(11.), Vec2::splat(12.))
            .unwrap()
    );
    assert!(
        ground
            .session_crater_intersects_rect(Vec2::splat(f32::NAN), Vec2::ONE)
            .is_err()
    );
    ground.content_revision.crater = Some(craters.revision()); // 精确源未同步时不能猜测没有坑。
    assert!(
        ground
            .session_crater_intersects_rect(Vec2::splat(7.), Vec2::splat(9.))
            .is_err()
    );
    ground.authority = None;
    assert!(
        ground
            .session_crater_intersects_rect(Vec2::ZERO, Vec2::ONE)
            .is_err()
    );
}

#[test]
fn authoritative_height_is_stable_across_display_center_and_lod_changes() {
    let mut ground = snapshot();
    let point = Vec2::new(54.375, 27.625); // 不落在基础格点上，能区分真实三角插值与粗LOD。
    let first = ground.sample_authoritative_surface(point);
    let first_content = ground.content_revision();

    ground.height.fine_center = Vec2::new(8.0, 56.0); // 模拟相机移到另一吸附中心。
    ground.height.bands[0].geometry_mip = 3; // 模拟同一点改由更粗的可见环带覆盖。
    ground.height.max_mip = 4;
    ground.height.revision[2] += 1;
    ground.revision = ground.height.revision;
    ground.pages = None; // 即使当前显示源变成合法平地，作者结果也不能随显示近似变化。

    assert_eq!(ground.content_revision(), first_content);
    assert_eq!(ground.sample_authoritative_surface(point), first); // 摆放与Mesh植被只认基础Mip 0。
}

#[test]
fn stitch_anisotropic_corner_queries_match_nearest_actual_mesh_triangle() {
    use feathrise_environment::vegetation::blade::{GpuBladeClipmapStitches,sample_clipmap_surface_for_validation};
    let mut ground=snapshot();ground.height.world_max=Vec2::new(509.,301.5);
    ground.height.cell_size=Vec2::new(1.,1.5);ground.height.logical_cells=UVec2::new(509,201);
    ground.height.bands=fixed_bands(4,4);ground.height.band_count=4;ground.height.max_mip=2;
    let mut constraints=GpuBladeClipmapStitches::default();
    for e in 0..4 {
        let axis=if e<2 {1}else{0};let normal=1-axis;let step=ground.height.world_max[axis]/16.;
        constraints.edges[e]=Vec4::new(17.,step,8.,if e<2 {2.}else{1.});
        for i in 0..=16 {
            let mut p=Vec2::ZERO;p[axis]=i as f32*step;p[normal]=if e%2==0 {0.}else{ground.height.world_max[normal]};
            constraints.heights[e][i/4][i%4]=5.+(p.x*0.03).sin()*2.+p.y*0.02;
        }
    }
    ground.height.stitches=Arc::new(constraints);
    for center in [Vec2::new(1.,1.5),Vec2::new(508.,300.)] {
        ground.height.fine_center=center;
        for i in 0..12 {
            let offset=Vec2::new(2.637+i as f32*0.17,0.943+i as f32*0.03);
            let p=if center.x<2. {offset}else{ground.height.world_max-offset};
            let hit=actual_mesh_hit(&ground,Vec3::new(p.x,60.,p.y),Vec3::NEG_Y).expect("角落实际覆盖");
            let ClipmapGroundHit::Hit(ray)=ground.height_ray_intersection(Vec3::new(p.x,60.,p.y),Vec3::NEG_Y) else{panic!("角落射线漏面 {p:?}");};
            assert!(ray.distance(hit)<0.002,"{p:?}: ray {ray:?} real {hit:?}");
            let ClipmapGroundHit::Hit(cpu)=ground.sample_surface(p) else{panic!("CPU角落漏面 {p:?}");};
            assert!((cpu.y-hit.y).abs()<0.002,"{center:?} {p:?}: CPU {cpu:?} real {hit:?}");
            let mirrored=sample_clipmap_surface_for_validation(&ground.height,p,|p,mip|ground.sample_base(p,mip).unwrap()).unwrap();
            assert!((mirrored-hit.y).abs()<0.002,"{center:?} {p:?}: grass {mirrored} real {hit:?}");
        }
    }
}

#[test]
fn stitch_authoritative_edge_keeps_full_curve_without_waiting_for_base_pages() {
    use feathrise_environment::vegetation::blade::GpuBladeClipmapStitches;
    let mut ground=snapshot();let mut constraints=GpuBladeClipmapStitches::default();
    constraints.edges[1]=Vec4::new(17.,4.,8.,1.);
    for i in 0..=16 {constraints.heights[1][i/4][i%4]=5.+(i as f32*0.4).sin()*2.;}
    ground.author_stitches=Arc::new(constraints);ground.authority=None;
    let p=Vec2::new(96.,7.25);let expected=constraints.sample(1,p.y);
    for stride in [1.,4.,16.] {
        let mut display=constraints;display.edges[1].w=stride;ground.height.stitches=Arc::new(display);
        assert_eq!(ground.sample_authoritative_surface(p),ClipmapGroundHit::Hit(Vec3::new(p.x,expected,p.y)));
    }
    assert_eq!(ground.sample_authoritative_surface(Vec2::new(90.,7.25)),ClipmapGroundHit::WaitingForHeight);
}

fn actual_mesh_hit(
    snapshot: &ClipmapGroundSnapshot,
    origin: Vec3,
    direction: Vec3,
) -> Option<Vec3> {
    // 直接遍历正式Mesh，只用于测试。
    let h = &snapshot.height; // 使用同一几何配置。
    let mesh = ClipmapConfig {
        finest_cell_size: h.cell_size,
        half_cells: h.bands[0].outer_half_cells.x,
        levels: h.band_count as u8,
    }
    .build_combined_mesh()
    .unwrap(); // 真正生产Mesh构建器。
    let positions = mesh
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .unwrap()
        .as_float3()
        .unwrap(); // 正式顶点。
    let bevy::mesh::VertexAttributeValues::Float32x2(lods) =
        mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
    else {
        panic!("uv1");
    }; // 正式Mip标记。
    let vertices: Vec<_> = positions
        .iter()
        .zip(lods)
        .map(|(p, l)| {
            let level = l[0] as u32;
            let xz = h.stitches.position(Vec2::new(p[0], p[2]) + snapshot.center(level),h.world_min,h.world_max);
            Vec3::new(xz.x, snapshot.sample(xz, level).unwrap(), xz.y)
        })
        .collect(); // UV_1.x 已直接记录每个顶点的几何Mip，与顶点着色器位移一致。
    let indices: Vec<_> = mesh.indices().unwrap().iter().collect(); // 正式索引三角形。
    indices
        .chunks_exact(3)
        .filter(|ids| {
            (vertices[ids[1]].xz() - vertices[ids[0]].xz())
                .perp_dot(vertices[ids[2]].xz() - vertices[ids[0]].xz())
                < -1e-9 // 正式地表背面剔除；边界收敛翻面的三角不能作为可见上表面真值。
        })
        .filter_map(|ids| {
            ray_triangle(
                origin,
                direction,
                [vertices[ids[0]], vertices[ids[1]], vertices[ids[2]]],
            )
        })
        .filter(|t| {
            let p = (origin + direction * *t).xz();
            p.cmpge(h.world_min).all() && p.cmple(h.world_max).all()
        })
        .min_by(f32::total_cmp)
        .map(|t| origin + direction * t) // 最近有地表面积的面，地图外同样裁剪。
} // Oracle不重建查询的单元/接缝逻辑。

#[test] // 覆盖真实五角接缝、矩形边缘和鞍形高度。
fn clipmap_ground_matches_actual_mesh_triangles() {
    // 三种吸附视图。
    let mut snapshot = snapshot(); // 非平测试页。
    for center in [
        Vec2::new(47.0, 31.0),
        Vec2::new(48.0, 32.0),
        Vec2::new(95.0, 63.0),
        Vec2::new(-24.0, 100.0), // 相机在对象外，查询仍须与真实粗层及接缝一致。
    ] {
        // 吸附边沿及地图边界。
        snapshot.height.fine_center = center; // 模拟合法视图换代。
        for point in [
            Vec2::new(47.1, 31.2),
            Vec2::new(54.8, 32.4),
            Vec2::new(38.5, 23.5),
            Vec2::new(62.2, 46.4),
            Vec2::new(0.1, 0.1),
            Vec2::new(95.9, 63.9),
        ] {
            // 中心、接缝、粗环、矩形末端。
            for direction in [Vec3::NEG_Y, Vec3::new(0.3, -1.0, 0.2).normalize()] {
                // 垂直与斜坡射线。
                let origin = Vec3::new(point.x, 60.0, point.y); // 高于整个量程。
                match (
                    snapshot.ray_intersection(origin, direction),
                    actual_mesh_hit(&snapshot, origin, direction),
                ) {
                    (ClipmapGroundHit::Hit(a), Some(b)) => {
                        assert!(a.distance(b) < 0.001, "{a:?} {b:?}")
                    }
                    (ClipmapGroundHit::Miss, None) => {}
                    other => panic!("不同实际网格命中：{other:?}"),
                } // 查询结果必须吻合真实Mesh。
            } // 下一方向。
        } // 下一地表位置。
    } // 下一视图。
} // 不以作者双线性插值作为期望值。

#[test]
fn stitch_ground_and_rays_match_real_mesh_at_coarse_boundaries_and_corners() {
    use feathrise_environment::vegetation::blade::GpuBladeClipmapStitches;
    let mut ground=snapshot();
    let mut stitches=GpuBladeClipmapStitches::default();
    for e in 0..4 {
        let axis=if e<2 {1}else{0};let normal=1-axis;
        let length=ground.height.world_max[axis];let step=length/16.;
        stitches.edges[e]=Vec4::new(17.,step,8.,4.);
        for i in 0..=16 {
            let mut p=Vec2::ZERO;p[axis]=i as f32*step;
            p[normal]=if e%2==0 {0.}else{ground.height.world_max[normal]};
            // 四角一致、边中间非线性，能发现只比较端点而漏掉曲线的错误。
            stitches.heights[e][i/4][i%4]=4.+(p.x*0.07).sin()*1.4+p.y*0.12;
        }
    }
    ground.height.stitches=Arc::new(stitches);
    for center in [Vec2::new(47.,31.),Vec2::new(1.,1.),Vec2::new(95.,63.),Vec2::new(-24.,100.)] {
        ground.height.fine_center=center;
        for e in 0..4 {
            let axis=if e<2 {1}else{0};let normal=1-axis;
            for i in 0..16 {
                let mut p=Vec2::ZERO;p[axis]=(i as f32+0.23)*ground.height.world_max[axis]/16.;
                for inward in [0.03,0.37,1.17,3.1] {
                    p[normal]=if e%2==0 {inward}else{ground.height.world_max[normal]-inward};
                    let origin=Vec3::new(p.x,60.,p.y);
                    let expected=actual_mesh_hit(&ground,origin,Vec3::NEG_Y);
                    let sampled=ground.sample_surface(p);
                    match (sampled,expected) {
                        (ClipmapGroundHit::Hit(a),Some(b))=>assert!(a.distance(b)<0.002,"sample {center:?} {p:?}: {a:?} {b:?}"),
                        (ClipmapGroundHit::Miss,None)=>{},
                        other=>panic!("stitch sample coverage {center:?} {p:?}: {other:?}"),
                    }
                    match (ground.ray_intersection(origin,Vec3::NEG_Y),expected) {
                        (ClipmapGroundHit::Hit(a),Some(b))=>assert!(a.distance(b)<0.002,"ray {center:?} {p:?}: {a:?} {b:?}"),
                        (ClipmapGroundHit::Miss,None)=>{},
                        other=>panic!("stitch ray coverage {center:?} {p:?}: {other:?}"),
                    }
                }
            }
        }
    }
}

#[test] // 缺页等待不能退回平面。
fn clipmap_ground_waits_for_missing_pages_and_content_revision() {
    // 查询正好命中中心非平页。
    let mut snapshot = snapshot();
    let origin = Vec3::new(47.0, 60.0, 31.0); // 当前中心。
    snapshot.height.missing_pages[0] = UVec4::new(1, 0, 0, 0); // 正式页(0,0)尚未采用。
    assert_eq!(
        snapshot.ray_intersection(origin, Vec3::NEG_Y),
        ClipmapGroundHit::WaitingForHeight
    ); // 不产生平地草。
    snapshot.height.missing_pages = [UVec4::ZERO; 16];
    snapshot.ready = false; // 新雕刻内容尚未采用。
    assert_eq!(
        snapshot.ray_intersection(origin, Vec3::NEG_Y),
        ClipmapGroundHit::WaitingForHeight
    ); // 内容代次也严格等待。
} // 局部与全局门各自明确。

#[test] // 合法空白仍可立即落笔，地图外无虚构地面。
fn clipmap_ground_flat_and_invalid_rays() {
    // 无CPU页即正式隐式平地。
    let mut snapshot = snapshot();
    snapshot.pages = None; // 空白高度不分配整图。
    assert_eq!(
        snapshot.ray_intersection(Vec3::new(47.0, 60.0, 31.0), Vec3::NEG_Y),
        ClipmapGroundHit::Hit(Vec3::new(47.0, 0.0, 31.0))
    ); // 零高度是合法已知地表。
    assert_eq!(
        snapshot.ray_intersection(Vec3::new(-1.0, 60.0, -1.0), Vec3::NEG_Y),
        ClipmapGroundHit::Miss
    ); // 地图外片元裁剪。
    assert_eq!(
        snapshot.ray_intersection(Vec3::ZERO, Vec3::ZERO),
        ClipmapGroundHit::Failed
    ); // 不产生NaN落点。
} // 不影响GLB拾取。

#[test] // 页边、每页独立min/span与半格R16解码由独立算术验证。
fn clipmap_ground_quantized_pages_have_exact_boundary_owner() {
    // 两个高度不同的分页。
    let mut snapshot = snapshot(); // 复用有效基本身份。
    snapshot.height.world_max = Vec2::new(512.0, 256.0);
    snapshot.height.logical_cells = UVec2::new(512, 256);
    snapshot.height.max_mip = 0; // 基础层两页。
    let mut heights = vec![0_u8; 257 * 257 * 4]; // 两个257² R16层。
    for z in 0..257 {
        for x in 0..257 {
            let offset = (z * 257 + x) * 2;
            let value = if x % 2 == 0 { 0_u16 } else { u16::MAX };
            heights[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
    } // 第零层交替0/1，半格应恰好0.5。
    let mut table = Vec::new(); // 独立页范围，允许页边出现可辨高度。
    for word in [
        1_u32,
        10.0_f32.to_bits(),
        20.0_f32.to_bits(),
        0,
        2,
        50.0_f32.to_bits(),
        4.0_f32.to_bits(),
        0,
    ] {
        table.extend_from_slice(&word.to_le_bytes());
    } // 第二层零样本恢复到50米。
    snapshot.pages = Some(Arc::new(GroundPageBytes {
        heights,
        table,
        width: 2,
        height_bounds: [10.0, 54.0],
    })); // 冻结已采用字节。
    assert_eq!(snapshot.sample(Vec2::new(255.0, 1.0), 0), Some(30.0)); // 第一页min+span。
    assert_eq!(snapshot.sample(Vec2::new(255.5, 1.5), 0), Some(20.0)); // R16半格双线性恢复。
    assert_eq!(snapshot.sample(Vec2::new(256.0, 1.0), 0), Some(50.0)); // 页边必须归右侧正式所有者。
    assert_eq!(snapshot.sample(Vec2::new(512.0, 256.0), 0), Some(50.0)); // 地图最远端归最后页而非越界。
    snapshot.height.missing_pages[0] = UVec4::new(2, 0, 0, 0); // 右侧页键(1,0)+1。
    assert_eq!(snapshot.sample(Vec2::new(256.0, 1.0), 0), None); // 新页不能误解释为空白。
    assert_eq!(snapshot.sample(Vec2::new(255.5, 1.0), 0), Some(20.0)); // 左侧完整共享边仍合法。
} // 不依赖查询自己的采样作为期望值。

#[test] // 确定性密集接缝和擦地射线防止只验证垂直中心点。
fn clipmap_ground_seams_and_shallow_rays_match_mesh() {
    // 不引入随机依赖。
    let snapshot = snapshot(); // 奇数吸附中心。
    for index in 0..96 {
        // 固定小测试预算。
        let x = 1.0 + ((index * 37) % 94) as f32 + 0.37;
        let z = 1.0 + ((index * 19) % 62) as f32 + 0.21; // 覆盖全矩形和各级接缝。
        let origin = Vec3::new(x, 18.0, z); // 高度落在地形量程内，可命中前后多个坡面。
        let direction = Vec3::new(if index % 2 == 0 { 1.0 } else { -1.0 }, -0.02, 0.15).normalize(); // 接近水平、含双方向。
        match (
            snapshot.ray_intersection(origin, direction),
            actual_mesh_hit(&snapshot, origin, direction),
        ) {
            (ClipmapGroundHit::Hit(a), Some(b)) => {
                assert!(a.distance(b) < 0.002, "{index}: {a:?} {b:?}")
            }
            (ClipmapGroundHit::Miss, None) => {}
            other => panic!("擦地射线与正式Mesh不同：{index}: {other:?}"),
        } // 要求最近交点一致。
    } // 全部测试射线完成。
} // 验证近水平不会回退水平面。

#[test] // 合并鼠标请求不能跨未查询缺页补画桥。
fn clipmap_ground_segment_rejects_missing_page_bridge() {
    // 两个端点自身可在已知区域。
    let mut snapshot = snapshot(); // 默认全部已知。
    snapshot.height.world_max = Vec2::new(2048.0, 512.0);
    snapshot.height.logical_cells = UVec2::new(2048, 512); // 全部测试端点处于合法地图内。
    let spec = SparseHeightSpec::new([0.0, 0.0], [2048.0, 512.0], 1.0, -32.0, 64.0).unwrap();
    snapshot.authority = Some(Arc::new(AuthorityHeightSnapshot {
        identity: AuthorityIdentity {
            map_id: snapshot.map_id.clone(),
            content: snapshot.content_revision,
        },
        revision: 2,
        spec,
        pages: (0..=4)
            .map(|x| (SparseHeightTileKey { x, z: 0 }, AuthorityPage::Flat))
            .collect(),
        craters: BTreeMap::new(),
    })); // 人工放大显示范围时同步提供相同范围的基础作者页。
    let from = Vec3::new(200.0, 10.0, 100.0);
    let to = Vec3::new(600.0, 20.0, 100.0); // X跨过基础页(1,0)。
    assert!(snapshot.segment_is_known(from, to)); // 没有缺页时保留原有连续笔画。
    snapshot.missing_pages[0] = UVec4::new(2, 0, 0, 0); // 中间世界X=256..512的页尚未采用。
    assert!(!snapshot.segment_is_known(from, to)); // 两个有效端点也不能越过未知中间区域。
    assert!(!snapshot.segment_is_known(to, from)); // 反向拖动使用相同裁剪规则。
    assert!(snapshot.segment_is_known(Vec3::new(20.0, 5.0, 20.0), Vec3::new(100.0, 8.0, 100.0))); // 远离缺页的笔画继续连接。
    assert!(!snapshot.segment_is_known(Vec3::new(255.5, 0.0, 20.0), Vec3::new(255.5, 0.0, 30.0))); // 扩一格覆盖临近三角面的顶点支持。
    assert!(snapshot.segment_is_known(Vec3::new(20.0, 0.0, 20.0), Vec3::new(20.0, 0.0, 20.0))); // 静止有效端点不会除零。
    snapshot.missing_pages = [UVec4::ZERO; 16];
    Arc::get_mut(snapshot.authority.as_mut().unwrap())
        .unwrap()
        .pages
        .remove(&SparseHeightTileKey { x: 1, z: 0 });
    assert!(!snapshot.segment_is_known(from, to)); // 可见环带完整也不能越过尚未取得的基础作者页。
    Arc::get_mut(snapshot.authority.as_mut().unwrap())
        .unwrap()
        .pages
        .insert(SparseHeightTileKey { x: 1, z: 0 }, AuthorityPage::Flat);
    assert!(snapshot.segment_is_known(from, to)); // 同代权威页恢复后允许从当前有效点重新连接。
    snapshot.missing_pages[1] = UVec4::new(2, 0, 0, 0); // 父页(1,0)覆盖X=512..1024。
    assert!(
        !snapshot.segment_is_known(Vec3::new(400.0, 0.0, 100.0), Vec3::new(1100.0, 0.0, 100.0))
    ); // 父页必须按层级世界格距放大。
    assert!(snapshot.segment_is_known(Vec3::new(20.0, 0.0, 20.0), Vec3::new(100.0, 0.0, 20.0))); // 其他层的远缺页不切断近处已知线段。
    snapshot.ready = false;
    assert!(!snapshot.segment_is_known(from, to)); // 内容换代暂停连接。
} // 固定最多64次矩形测试，不扫描全部高度。

impl ClipmapGroundSnapshot {
    /// 页worker回归使用真实产出的量化字节，避免另写一套高度编解码夹具冒充下游验证。
    pub(in crate::maps::dynamic_map::clipmap) fn adopted_pages_for_test(height: GpuBladeClipmapHeight, pages: Arc<GroundPageBytes>) -> Self {
        let mut snapshot = snapshot();
        snapshot.revision = height.revision;
        snapshot.missing_pages = height.missing_pages;
        snapshot.ready = height.ready;
        snapshot.height = height;
        snapshot.pages = Some(pages);
        snapshot.authority = None;
        snapshot
    }

    /// 地板朝上、洞顶朝下的两层真实网格，供水面和净空共同验证指定层查询。
    pub(crate) fn stacked_for_test() -> Self {
        let mut snapshot = snapshot();
        let shape = avian3d::prelude::Collider::trimesh(
            vec![
                Vec3::new(0., 2., 0.),
                Vec3::new(4., 2., 0.),
                Vec3::new(0., 2., 4.),
                Vec3::new(4., 2., 4.),
                Vec3::new(0., 8., 0.),
                Vec3::new(4., 8., 0.),
                Vec3::new(0., 8., 4.),
                Vec3::new(4., 8., 4.),
            ],
            vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6]],
        );
        snapshot.terrain_meshes = Arc::from(vec![
            crate::maps::dynamic_map::terrain_topology::TerrainMeshQuery::new(
                shape,
                Mat4::IDENTITY,
            )
            .unwrap(),
        ]);
        snapshot.holes = Arc::from(vec![feathrise_map_content::MapTerrainCutout {
            min: [0., 0.],
            max: [4., 4.],
        }]);
        snapshot
    }
}
#[test]
fn selected_height_keeps_stacked_cave_floors_separate() {
    let snapshot = ClipmapGroundSnapshot::stacked_for_test();
    assert_eq!(
        snapshot.sample_surface_below(Vec3::new(1., 5., 1.), 10.),
        ClipmapGroundHit::Hit(Vec3::new(1., 2., 1.))
    );
    assert_eq!(
        snapshot.sample_surface_below(Vec3::new(1., 10., 1.), 10.),
        ClipmapGroundHit::Hit(Vec3::new(1., 8., 1.))
    );
    let (_, normal) = snapshot
        .three_dimensional_hit(Vec3::new(1., 5., 1.), -Vec3::Y, 10.)
        .unwrap();
    assert!(normal.dot(Vec3::Y) > 0.99);
    assert!(
        snapshot
            .three_dimensional_hit(Vec3::new(1., 5., 1.), Vec3::NEG_Y * 2., 2.)
            .is_none()
    ); // 非单位方向不得把2米预算误放大到4米。
    // Parry会把背面命中法线翻向射线；业务查询必须恢复三角绕向，不能把洞顶误认为地板。
    let (_, ceiling) = snapshot
        .three_dimensional_hit(Vec3::new(1., 10., 1.), -Vec3::Y, 10.)
        .unwrap();
    assert!(ceiling.dot(-Vec3::Y) > 0.99);
    let (_, ceiling_inside) = snapshot
        .three_dimensional_hit(Vec3::new(1., 5., 1.), Vec3::Y, 10.)
        .unwrap();
    assert!(ceiling_inside.dot(-Vec3::Y) > 0.99);
    assert!(
        snapshot
            .three_dimensional_hit(Vec3::ZERO, Vec3::ZERO, 10.)
            .is_none()
    );
}
