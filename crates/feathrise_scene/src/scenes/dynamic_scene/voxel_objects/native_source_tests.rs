//! 真实薄板源离线复现：原窗口、密度与提取三角面接住实窗相机射线，不以解析盒体代替提取结果。
use super::super::resources::{VoxelObjectRuntime, VoxelWorkBudget};
use super::*;
use crate::TerrainObjectRaySnapshot;
use feathrise_scene_core::residency::SceneSourceFrame;

fn verify(source: SceneVoxelTerrainSource) -> (Arc<SourceIndex>, crate::ClipmapGroundSnapshot) {
    assert_eq!(source.cell_size_meters, 0.25);
    assert_eq!(source.min, [-2., -0.05, -2.]);
    assert_eq!(source.max, [2., 0., 2.]);
    assert!(source.blocks.is_empty());
    // 与体积地形正式创建入口jobs::new_object相同的描述符；显示规划必须读取
    // descriptor.display_lods，不能因当前默认值相同而借用碰撞列表。
    let descriptor = feathrise_scene_content::SceneVoxelTerrainDescriptor {
        cell_size_meters: source.cell_size_meters,
        min: source.min,
        max: source.max,
        source_path: format!(
            "scenes/{}/terrains/{}/source.scene_voxel_source.json",
            source.scene_id,
            source.terrain_id.as_str()
        ),
        display_lods: feathrise_scene_content::default_collision_lods(),
        collision_lods: feathrise_scene_content::default_collision_lods(),
    };
    let index = Arc::new(SourceIndex::new(source).unwrap());
    let scale = Vec3::ONE;
    let anchor_extent = (index.source.cell_size_meters * 32.).max(4. / scale.min_element());
    let display_center = (Vec3::new(0., 66.99999237060547, 0.) / anchor_extent)
        .floor()
        .as_ivec3()
        .to_array();
    let identity = Identity {
        root: Entity::PLACEHOLDER,
        source: Handle::<SceneVoxelTerrainSource>::default().id(),
        revision: 0,
        descriptor,
        display_center,
        physics_centers: vec![display_center],
        player_center: display_center,
        anchor_extent,
        scale,
    };
    let foci =
        [(Vec3::from_array(display_center.map(|v| v as f32)) + Vec3::splat(0.5)) * anchor_extent];
    eprintln!("薄板显示正式规划输入：{identity:?},foci={foci:?}");
    let plan = window::plan_with_limit(
        &index,
        &foci,
        identity.scale,
        &identity.descriptor.display_lods,
        false,
        window::DISPLAY_CHUNKS,
    );
    assert_eq!(plan.chunks.len(), 1, "此小来源应完整使用单个正式提取块");
    let chunk = plan.chunks[0];
    assert_eq!(chunk.depth, 0);
    let mut reader = DensityReader::new(index.clone());
    let density = finish_lattice(
        reader
            .read(Path::new("."), plan_lattice(&index, chunk, &plan.chunks))
            .unwrap(),
    );
    let negatives = density.iter().filter(|value| **value < 0.).count();
    let mut mesh = build_density_mesh_fractional(window::CELLS, 1., density, Vec3::ZERO).unwrap();
    for point in &mut mesh.positions {
        *point = (index.domain.min
            + Vec3::from_array(*point) / window::CELLS as f32
                * (index.domain.max - index.domain.min))
            .to_array();
    }
    let raw_hits = verify_mesh("原提取未焊接", &index, &mesh, negatives);

    // 实窗使用build_level聚合后的共享拓扑，FIX_INTERNAL_EDGES受焊接索引影响；
    // 必须调用同一生产build_set→build_level，不能复制焊接或绕过显示窗口规划。
    AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let budget = VoxelWorkBudget::default();
    let lease = Arc::new(budget.acquire().expect("隔离测试必须获得一个正式工作席位"));
    let (production, limited) = bevy::tasks::block_on(build_set(
        Path::new("."),
        &index,
        &mut reader,
        &foci,
        &identity,
        false,
        &lease,
    ))
    .unwrap();
    assert!(!limited, "薄板不得触发正式显示预算降级");
    assert_eq!(
        production.indices.len(),
        mesh.indices.len(),
        "焊接只应重映射索引，不能删去真实三角面"
    );
    assert!(
        production.positions.len() <= mesh.positions.len(),
        "位型焊接不能增加顶点"
    );
    let production_hits = verify_mesh("生产build_level焊接后", &index, &production, negatives);
    assert_eq!(
        raw_hits, production_hits,
        "原提取与生产聚合必须接住同一实际表面"
    );

    // 地表表优先发布ground_snapshot，真实拾取通常落到Clipmap包装而非Mesh分支；
    // 因此还须调用正式运行时包装，证明实窗实际选用的查询路径也能接住原射线。
    let runtime = VoxelObjectRuntime {
        active_index: Some(index.clone()),
        applied: Some(identity),
        display_query: triangle_shape(&production).unwrap(),
        ..Default::default()
    };
    let ground = runtime
        .ground_snapshot(Entity::PLACEHOLDER)
        .expect("已采用体素必须发布正式地表快照");
    let query = TerrainObjectRaySnapshot::Clipmap(
        ground.clone(),
        SceneSourceFrame::from_transform(Transform::from_xyz(48., 0., -48.)),
    );
    eprintln!(
        "薄板正式ground_snapshot/Clipmap包装：bounds={:?}",
        query.mesh_bounds()
    );
    let ground_hits = verify_query("正式ground_snapshot/Clipmap包装", &index, &query);
    assert_eq!(
        production_hits, ground_hits,
        "实际发布包装与底层显示三角面必须接住同一表面"
    );
    (index, ground)
}

fn verify_mesh(
    label: &str,
    index: &SourceIndex,
    mesh: &TerrainMeshInput,
    negatives: usize,
) -> Vec<Vec3> {
    let shape = triangle_shape(mesh)
        .unwrap()
        .expect("薄板必须保留实际提取三角面");
    let frame = SceneSourceFrame::from_transform(Transform::from_xyz(48., 0., -48.));
    let query = TerrainObjectRaySnapshot::Mesh(shape, frame);
    eprintln!(
        "薄板{label}：domain={:?}..{:?},负密度={negatives},顶点={},三角={},bounds={:?}",
        index.domain.min,
        index.domain.max,
        mesh.positions.len(),
        mesh.indices.len() / 3,
        query.mesh_bounds()
    );
    verify_query(label, index, &query)
}

fn verify_query(label: &str, index: &SourceIndex, query: &TerrainObjectRaySnapshot) -> Vec<Vec3> {
    let mut hits = Vec::new();
    for (origin, direction, expected_y) in [
        // v7、v8失败时记录的是同一原相机射线；不能改到另一个容易命中的格点。
        (
            Vec3::new(47.500003814697266, 66.89998626708984, -47.99999237060547),
            Vec3::new(0., -1., -5.960465188081798e-8),
            0.,
        ),
        (
            Vec3::new(47.500003814697266, -66.89998626708984, -47.99999237060547),
            Vec3::new(0., 1., -5.960465188081798e-8),
            -0.05,
        ),
        (Vec3::new(47.5, 10., -48.), Vec3::NEG_Y, 0.),
        (Vec3::new(47.5, -10., -48.), Vec3::Y, -0.05),
    ] {
        let hit = query.ray(origin, direction);
        let normal = query.ray_surface(origin, direction, 1_000_000.);
        eprintln!(
            "薄板{label}射线：origin={origin:?},direction={direction:?},ray={hit:?},surface={normal:?}"
        );
        let hit = hit.expect("正式提取薄板必须接住原世界射线");
        assert!(query.ray_hit_is_three_dimensional(origin, direction, hit),
            "正式体素包装的上下真实表面都必须继续进入三维字段");
        assert!(
            (hit.y - expected_y).abs() < 0.005,
            "实际厚度偏差不能超过10%：{hit:?}"
        );
        let (surface, normal) = normal.expect("原射线还须能解析同一面的真实法线");
        assert_eq!(hit, surface, "只取位置与完整法线命中必须来自同一真实表面");
        assert!(
            normal.y * if expected_y == 0. { 1. } else { -1. } > 0.9,
            "上下两面的实际法线必须相反"
        );
        hits.push(hit);
    }
    let thickness = hits[0].y - hits[1].y;
    eprintln!(
        "薄板{label}原实窗上下命中：top={:?},bottom={:?},实际厚度={thickness}m，作者厚度={}m，作者格距={}m",
        hits[0],
        hits[1],
        index.source.max[1] - index.source.min[1],
        index.source.cell_size_meters
    );
    assert!(
        (hits[0].xz() - hits[1].xz()).length() < 0.001,
        "上下原射线必须瞄准同一处"
    );
    assert!(
        (thickness - 0.05).abs() < 0.005,
        "不得以零厚双面或加厚替代真实0.05m薄板"
    );
    hits
}

#[test]
fn native_thin_plate_mesh_accepts_actual_editor_camera_rays() {
    verify(SceneVoxelTerrainSource {
        schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
        scene_id: feathrise_scene_content::SceneId::parse("M1-SAMPLE").unwrap(),
        terrain_id: feathrise_scene_content::TerrainId::parse("native-thin-plate").unwrap(),
        cell_size_meters: 0.25,
        min: [-2., -0.05, -2.],
        max: [2., 0., 2.],
        blocks: Default::default(),
        surface_fields_path: None,
    });
}

#[test]
fn native_thin_plate_clearance_preserves_opposite_surface_samples_in_the_same_cell() {
    use feathrise_terrain::volume::sparse::paged::{VolumeFieldSample, VolumeFieldSnapshot};
    let (_, ground) = verify(SceneVoxelTerrainSource {
        schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
        scene_id: feathrise_scene_content::SceneId::parse("M1-SAMPLE").unwrap(),
        terrain_id: feathrise_scene_content::TerrainId::parse("native-thin-clearance").unwrap(),
        cell_size_meters: 0.25,
        min: [-2., -0.05, -2.], max: [2., 0., 2.], blocks: Default::default(),
        surface_fields_path: None,
    });
    let mut fields = VolumeFieldSnapshot::empty(0.25);
    let sample = |y, normal, id: &str| VolumeFieldSample {
        position: [-0.5, y, 0.], normal, surface: Vec::new(),
        vegetation: vec![(id.to_owned(), 255)], puddles: 0,
    };
    let cell = [-2, 0, 0]; // 0.05米真实薄板两面属于同一个0.25米作者格。
    fields.samples.insert(cell, sample(0., Vec3::Y.to_array(), "top"));
    fields.additional_samples.insert(cell, vec![sample(-0.05, Vec3::NEG_Y.to_array(), "underside")]);
    let fields = Arc::new(fields);
    let build = |clearances| crate::scenes::dynamic_scene::vegetation::native_fields_source_for_test(
        fields.clone(), ground.clone(), clearances).unwrap();
    // 每个图层为两张真实面保留承托记录；另一层的面概率为零，不等于增加种植点。
    assert_eq!(build(None), vec![("top".to_owned(), 2, 1), ("underside".to_owned(), 2, 1)]);
    for transform in [Transform::IDENTITY, Transform::from_xyz(48., 0., -48.),
        Transform::from_xyz(48., 3., -48.)
            .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.35, 0.6, -0.15))
            .with_scale(Vec3::new(2., 0.2, 0.5))] {
        let frame = SceneSourceFrame::from_transform(transform);
        let point = frame.to_world(Vec3::new(-0.5, 0., 0.));
        let center = point.xz();
        let clearances = crate::BuildingVegetationClearanceSnapshot {
            scene_id: ground.scene_id.clone(), revision: 1,
            regions: Arc::from([feathrise_scene_content::BuildingWorldQuad { corners: [
                center + Vec2::splat(-0.2), center + Vec2::new(0.2, -0.2),
                center + Vec2::splat(0.2), center + Vec2::new(-0.2, 0.2),
            ] }]), floor_heights: Arc::from([point.y]),
        };
        let cleared = build(Some((clearances, frame)));
        assert_eq!(cleared, vec![("top".to_owned(), 1, 0), ("underside".to_owned(), 1, 1)],
            "顶面真实建筑只清顶面；同格反面保留，且倾斜非均匀缩放后也不能串层");
    }
    assert_eq!(fields.iter_samples().count(), 2, "派生净空不能改写作者的任一面样本");
}

#[test]
#[ignore = "显式设置FEATHRISE_NATIVE_VOXEL_REPRO_SOURCE读取本轮自有实窗源"]
fn owned_native_voxel_source_replays_real_extracted_triangles_and_camera_rays() {
    let path = std::env::var_os("FEATHRISE_NATIVE_VOXEL_REPRO_SOURCE").expect("需要本轮源绝对路径");
    let path = std::path::PathBuf::from(path);
    assert!(path.is_absolute());
    let bytes = std::fs::read(&path).unwrap();
    let source: SceneVoxelTerrainSource = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(source.scene_id.as_str(), "M1-SAMPLE");
    eprintln!(
        "实际薄板源：{}，sha256={}",
        path.display(),
        feathrise_scene_content::authoring_io::bytes_hash(&bytes)
    );
    verify(source);
}

#[test]
#[ignore = "显式设置FEATHRISE_NATIVE_VOXEL_REPRO_SOURCE和FEATHRISE_NATIVE_VOXEL_REPRO_ROOT读取本轮自有实窗源"]
fn owned_native_voxel_fields_reach_complete_vegetation_source() {
    let path = std::path::PathBuf::from(std::env::var_os("FEATHRISE_NATIVE_VOXEL_REPRO_SOURCE").expect("需要本轮源"));
    let asset_root = std::path::PathBuf::from(std::env::var_os("FEATHRISE_NATIVE_VOXEL_REPRO_ROOT").expect("需要本轮资产根"));
    assert!(path.is_absolute() && asset_root.is_absolute());
    let bytes = std::fs::read(&path).unwrap();
    let source: SceneVoxelTerrainSource = serde_json::from_slice(&bytes).unwrap();
    assert!(source.surface_fields_path.is_some(), "端到端复现必须读取真实25个作者字段");
    eprintln!("实际三维植被来源：{}，sha256={}", path.display(), feathrise_scene_content::authoring_io::bytes_hash(&bytes));
    let (index, ground) = verify(source);
    let budget = VoxelWorkBudget::default();
    let lease = Arc::new(budget.acquire().unwrap());
    let (fields, limited) = bevy::tasks::block_on(load_fields(&asset_root, &index,
        &[Vec3::new(4., 4., 4.), Vec3::new(4., 68., 4.)], Vec3::ONE, &lease)).unwrap();
    assert!(!limited);
    let samples: Vec<_> = fields.0.iter_samples().collect();
    eprintln!("实际字段{}个，前8个={:?}", samples.len(), samples.iter().take(8).collect::<Vec<_>>());
    assert_eq!(samples.len(), 25);
    assert!(samples.iter().all(|sample| sample.position[1].abs() < 0.0001 && sample.normal[1] > 0.99),
        "原笔刷须保存真实薄板顶面，而不是半个作者格中心");
    let without = crate::scenes::dynamic_scene::vegetation::native_fields_source_for_test(
        fields.0.clone(), ground.clone(), None).unwrap();
    eprintln!("实际字段→正式三维源（无净空）：{without:?}");
    assert!(without.iter().any(|(_, _, positive)| *positive > 0), "真实提取与原样本必须生成非空正式源");
    let region = feathrise_scene_content::BuildingWorldQuad { corners: [
        Vec2::new(-2., -2.), Vec2::new(2., -2.), Vec2::new(2., 2.), Vec2::new(-2., 2.)] };
    let root_clearance = crate::BuildingVegetationClearanceSnapshot {
        scene_id: index.source.scene_id.clone(), revision: 1,
        regions: Arc::from([region]), floor_heights: Arc::from([0.]),
    };
    let with_root = crate::scenes::dynamic_scene::vegetation::native_fields_source_for_test(
        fields.0.clone(), ground.clone(), Some((root_clearance.clone(), SceneSourceFrame::default()))).unwrap();
    eprintln!("实际字段→正式三维源（错误借用全局原点建筑净空）：{with_root:?}");
    assert!(!with_root.iter().any(|(_, _, positive)| *positive > 0),
        "此诊断应准确复现v11局部样本被世界原点建筑完全误清的事实");
    let frame = SceneSourceFrame::from_transform(Transform::from_xyz(48., 0., -48.));
    let fixed = crate::scenes::dynamic_scene::vegetation::native_fields_source_for_test(
        fields.0.clone(), ground.clone(), Some((root_clearance.clone(), frame))).unwrap();
    eprintln!("实际字段→正式三维源（所属场景净空+正确相对矩阵）：{fixed:?}");
    assert_eq!(fixed, without, "位于远处的体素必须保留全部25个真实非零作者源");
    let mut overlapping = root_clearance;
    overlapping.regions = Arc::from([feathrise_scene_content::BuildingWorldQuad {
        corners: region.corners.map(|point| point + Vec2::new(48., -48.)),
    }]);
    let cleared = crate::scenes::dynamic_scene::vegetation::native_fields_source_for_test(
        fields.0.clone(), ground, Some((overlapping, frame))).unwrap();
    eprintln!("实际字段→正式三维源（真正覆盖变换后体素的建筑）：{cleared:?}");
    assert!(!cleared.iter().any(|(_, _, positive)| *positive > 0), "真正重叠的建筑仍须完成避让");
}
