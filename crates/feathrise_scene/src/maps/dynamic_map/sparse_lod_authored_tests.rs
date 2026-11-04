//! 作者显示档位回归：真实镜头选择只能使用配置倍率，离开源矩形和改变高程不能制造无限LOD。

use super::*;
use bevy::mesh::VertexAttributeValues;
use feathrise_map_content::{
    MAX_DISPLAY_LOD_MESH_BYTES, MAX_DISPLAY_LOD_PAGE_BYTES, MapTerrainDisplayLod, compile_display_lod_plan,
    worst_case_display_page_upload_bytes,
};
use std::collections::BTreeSet;

fn source(radii: [f32; 3]) -> (SparseHeightSpec, CompiledDisplayLodPlan) {
    let spec = SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.125, -8.0, 16.0).unwrap();
    let authored = compile_display_lod_plan(&spec, &[
        MapTerrainDisplayLod { outer_half_extent_meters: Some(radii[0]), spacing_multiplier: 1 },
        MapTerrainDisplayLod { outer_half_extent_meters: Some(radii[1]), spacing_multiplier: 2 },
        MapTerrainDisplayLod { outer_half_extent_meters: Some(radii[2]), spacing_multiplier: 4 },
        MapTerrainDisplayLod { outer_half_extent_meters: None, spacing_multiplier: 8 },
    ]).unwrap();
    (spec, authored)
}

fn camera(position: Vec3, yaw: f32, pitch: f32, fov: f32, pixels: f32) -> ClipmapView {
    let transform = Transform::from_translation(position)
        .with_rotation(Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0));
    ClipmapView::new(&GlobalTransform::from(transform),
        &Projection::Perspective(PerspectiveProjection { fov, aspect_ratio: 1.5, ..default() }),
        Vec2::new(pixels * 1.5, pixels)).unwrap()
}

fn chosen(position: Vec3, bounds: [f32; 2], spec: &SparseHeightSpec,
    authored: &CompiledDisplayLodPlan) -> (Vec2, ViewLodKey) {
    camera(position, 0.0, -0.5, 1.0, 720.0)
        .choice(spec, MapSourceFrame::default(), bounds, None, authored).unwrap()
}

/// 同时审计作者行映射、真实顶点UV_1及最终上传载荷，避免只检查状态标签上的倍率。
fn mesh_for(spec: &SparseHeightSpec, authored: &CompiledDisplayLodPlan, key: ViewLodKey) -> Mesh {
    let plan = compile_view_plan(spec, authored, key).unwrap();
    assert_eq!(plan.authored_lods, authored.authored_lods);
    for band in &plan.bands {
        let row = band.source_lod_index.expect("四个相邻作者倍率无需伪造内部来源行");
        assert_eq!(band.spacing_multiplier, authored.authored_lods[row].spacing_multiplier,
            "剔除前缀后必须仍指向原作者行，不能重新从第0行编号");
        assert!([1, 2, 4, 8].contains(&band.spacing_multiplier));
    }
    assert_eq!(plan.bands.last().unwrap().source_lod_index, Some(3));
    assert!(plan.mesh_bytes <= MAX_DISPLAY_LOD_MESH_BYTES);
    assert!(worst_case_display_page_upload_bytes(spec, &plan).unwrap() <= MAX_DISPLAY_LOD_PAGE_BYTES);
    let spacing = Vec2::new(spec.actual_spacing_meters[0] as f32, spec.actual_spacing_meters[1] as f32);
    let mesh = super::super::clipmap::build_display_lod_mesh(&plan, spacing).unwrap();
    assert_eq!(mesh.count_vertices() as u64, plan.vertex_count);
    assert_eq!(mesh.indices().unwrap().len() as u64, plan.index_count);
    assert_eq!(mesh.count_vertices() as u64 * 40 + mesh.indices().unwrap().len() as u64 * 4, plan.mesh_bytes);
    let Some(VertexAttributeValues::Float32x2(levels)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1) else {
        panic!("正式Clipmap几何必须保存用于真实高度采样的UV_1");
    };
    let actual: BTreeSet<_> = levels.iter().map(|level| {
        assert!(level[0].is_finite() && level[0].fract() == 0.0 && (0.0..=3.0).contains(&level[0]));
        1_u32 << level[0] as u32
    }).collect();
    let expected: BTreeSet<_> = plan.bands.iter().map(|band| band.spacing_multiplier).collect();
    assert_eq!(actual, expected, "实际顶点不得出现配置外的16/32倍等隐藏档位");
    mesh
}

#[test]
fn authored_lod_four_bands_bound_extreme_distance_and_all_lens_directions() {
    let (spec, authored) = source([8.0, 16.0, 32.0]);
    for position in [Vec3::Y, Vec3::new(32.125, 1.0, 0.0),
        Vec3::new(1000.0, 1000.0, -1000.0), Vec3::new(1_000_000.0, 20.0, -1_000_000.0)] {
        let (focus, baseline) = chosen(position, [0.0; 2], &spec, &authored);
        for yaw in [0.0, 1.2, std::f32::consts::PI] {
            for pitch in [-1.4, 0.0, 1.4] {
                for fov in [0.3, 1.0, 2.4] {
                    for pixels in [360.0, 2160.0] {
                        let result = camera(position, yaw, pitch, fov, pixels)
                            .choice(&spec, MapSourceFrame::default(), [0.0; 2], Some(baseline), &authored).unwrap();
                        assert_eq!(result, (focus, baseline),
                            "相同机位转向/FOV/窗口变化不能虚构新档：{position:?}/{yaw}/{pitch}/{fov}/{pixels}");
                        assert!(result.1.minimum_multiplier <= 8);
                    }
                }
            }
        }
        mesh_for(&spec, &authored, baseline);
    }
}

#[test]
fn authored_lod_just_outside_each_map_edge_keeps_the_near_fine_band_when_looking_anywhere() {
    let (spec, authored) = source([8.0, 16.0, 32.0]);
    for position in [Vec3::new(32.125, 1.0, 0.0), Vec3::new(-32.125, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 32.125), Vec3::new(0.0, 1.0, -32.125)] {
        let (focus, key) = chosen(position, [0.0; 2], &spec, &authored);
        assert_eq!(key.minimum_multiplier, 1, "只跨一个基础格仍能看见近边细格，不能整块骤降到末档");
        assert_eq!(focus, position.xz(), "细层继续使用真实相机中心，不各自钳制到地图边缘");
        for yaw in [0.0, std::f32::consts::PI] {
            let (_, turned) = camera(position, yaw, 0.2, 1.2, 1080.0)
                .choice(&spec, MapSourceFrame::default(), [0.0; 2], Some(key), &authored).unwrap();
            assert_eq!(turned, key, "转向远离地面也不能改变本机位作者档位");
        }
        mesh_for(&spec, &authored, key);
    }
}

#[test]
fn authored_lod_uses_actual_high_mountain_and_negative_pit_bounds_instead_of_world_zero() {
    let (spec, authored) = source([8.0, 16.0, 32.0]);
    for (height, bounds) in [(1001.0, [0.0, 1000.0]), (-1001.0, [-1000.0, 0.0])] {
        let position = Vec3::new(4.0, height, -6.0);
        let (_, actual) = chosen(position, bounds, &spec, &authored);
        let (_, zero_plane) = chosen(position, [0.0; 2], &spec, &authored);
        assert_eq!(actual.minimum_multiplier, 1, "距真实山顶/坑底仅1米，不能拿离Y=0的千米距离放粗");
        assert_eq!(zero_plane.minimum_multiplier, 8, "对照证明输入的真实高度范围参与了档位选择");
        mesh_for(&spec, &authored, actual);
    }
    // 同样的源内机位随实例整体平移旋转，实际高程仍以来源坐标的同代范围解释。
    let frame = MapSourceFrame::from_transform(Transform::from_xyz(500.0, 200.0, -300.0)
        .with_rotation(Quat::from_rotation_y(0.7)));
    let local = Vec3::new(0.0, 1001.0, 0.0);
    let (_, key) = camera(frame.to_world(local), 2.0, 0.1, 0.6, 720.0)
        .choice(&spec, frame, [0.0, 1000.0], None, &authored).unwrap();
    assert_eq!(key.minimum_multiplier, 1);
}

#[test]
fn authored_lod_wholly_coarse_distant_views_share_a_small_stable_source_centered_mesh() {
    let (spec, authored) = source([8.0, 16.0, 32.0]);
    let mut baseline = None;
    let mut baseline_mesh = None;
    for position in [Vec3::new(1000.0, 1.0, 0.0), Vec3::new(-100_000.0, 50_000.0, 70_000.0),
        Vec3::new(1_000_000.0, 1_000_000.0, -1_000_000.0)] {
        let (focus, key) = camera(position, 0.5, -0.3, 1.0, 720.0)
            .choice(&spec, MapSourceFrame::default(), [0.0; 2], baseline, &authored).unwrap();
        assert_eq!(focus, Vec2::ZERO, "全源只用末档时布局应固定在来源中心，不能追到百万米镜头位置");
        assert_eq!(key.minimum_multiplier, 8);
        if let Some(previous) = baseline { assert_eq!(key, previous); }
        let plan = compile_view_plan(&spec, &authored, key).unwrap();
        assert_eq!(plan.bands.len(), 1);
        let mesh = mesh_for(&spec, &authored, key);
        let Some(VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
            panic!("正式组合Mesh必须保留源局部顶点");
        };
        assert!(positions.iter().all(|position| position.iter().all(|value| value.is_finite())
            && position[0].abs() <= 128.0 && position[2].abs() <= 128.0),
            "64米源只需要有界覆盖，极远镜头不能制造巨大图外几何");
        let fingerprint = (positions.clone(), mesh.indices().unwrap().iter().collect::<Vec<_>>());
        if let Some(previous) = &baseline_mesh { assert_eq!(&fingerprint, previous); }
        baseline_mesh = Some(fingerprint);
        baseline = Some(key);
    }
}

#[test]
fn authored_lod_custom_radii_change_selected_real_geometry_and_preserve_original_row_indices() {
    let (spec, compact) = source([8.0, 16.0, 32.0]);
    let (_, wide) = source([16.0, 24.0, 48.0]);
    let position = Vec3::new(0.0, 12.0, 0.0);
    let (_, compact_key) = chosen(position, [0.0; 2], &spec, &compact);
    let (_, wide_key) = chosen(position, [0.0; 2], &spec, &wide);
    assert_eq!(compact_key.minimum_multiplier, 2);
    assert_eq!(wide_key.minimum_multiplier, 1, "用户扩大第一档半径后，相同距离必须保留真实细档");
    let compact_plan = compile_view_plan(&spec, &compact, compact_key).unwrap();
    assert_eq!(compact_plan.bands[0].source_lod_index, Some(1));
    let wide_plan = compile_view_plan(&spec, &wide, wide_key).unwrap();
    assert_eq!(wide_plan.bands[0].source_lod_index, Some(0));
    let compact_mesh = mesh_for(&spec, &compact, compact_key);
    let wide_mesh = mesh_for(&spec, &wide, wide_key);
    assert!(wide_mesh.count_vertices() > compact_mesh.count_vertices());
    let mut previous_indices = None;
    for (height, multiplier, row) in [(1.0, 1, 0), (12.0, 2, 1), (24.0, 4, 2), (40.0, 8, 3)] {
        let (_, key) = chosen(Vec3::Y * height, [0.0; 2], &spec, &compact);
        assert_eq!(key.minimum_multiplier, multiplier);
        let plan = compile_view_plan(&spec, &compact, key).unwrap();
        assert_eq!(plan.bands[0].source_lod_index, Some(row));
        if let Some(count) = previous_indices { assert!(plan.index_count < count); }
        previous_indices = Some(plan.index_count);
    }
}

#[test]
fn authored_lod_budget_failure_never_replaces_the_last_configured_band_with_a_new_multiplier() {
    let (spec, authored) = source([8.0, 16.0, 32.0]);
    let snapshot = authored.clone();
    assert!(compile_view_plan(&spec, &authored,
        ViewLodKey { minimum_multiplier: 8, coverage_cells: [4096; 2] }).is_err(),
        "超预算须明确失败并保留既有完整版本，不能继续派生16/32倍绕开作者末档");
    for multiplier in [16, 32, 64] {
        assert!(compile_view_plan(&spec, &authored,
            ViewLodKey { minimum_multiplier: multiplier, coverage_cells: [512; 2] }).is_err());
    }
    assert_eq!(authored, snapshot, "预算拒绝不能改写作者半径、倍率或来源映射");
}

#[test]
fn authored_lod_near_outside_two_band_source_loads_without_coarsening_or_empty_ring_budget_inflation() {
    let spec = SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.25, -8.0, 16.0).unwrap();
    let authored = compile_display_lod_plan(&spec, &[
        MapTerrainDisplayLod { outer_half_extent_meters: Some(16.0), spacing_multiplier: 1 },
        MapTerrainDisplayLod { outer_half_extent_meters: None, spacing_multiplier: 2 },
    ]).unwrap();
    for position in [Vec3::new(40.0, 1.0, 0.0), Vec3::new(40.0, 1.0, 40.0)] {
        let (focus, key) = chosen(position, [0.0; 2], &spec, &authored);
        assert_eq!(focus, position.xz());
        assert_eq!(key.minimum_multiplier, 1, "出界8米仍在16米第一档内，不能为预算删掉近边细层");
        let plan = compile_view_plan(&spec, &authored, key).unwrap();
        assert_eq!(plan.authored_lods, authored.authored_lods);
        assert_eq!(plan.bands.iter().map(|band| band.spacing_multiplier).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(plan.actual_half_extents_meters[0], [16.0; 2]);
        assert!(plan.mesh_bytes <= MAX_DISPLAY_LOD_MESH_BYTES);
        assert!(worst_case_display_page_upload_bytes(&spec, &plan).unwrap() <= MAX_DISPLAY_LOD_PAGE_BYTES);
        let half = Vec2::from_array(*plan.actual_half_extents_meters.last().unwrap());
        for corner in [Vec2::splat(-32.0), Vec2::splat(32.0)] {
            assert!((corner - focus).abs().cmple(half).all(), "合法双网格首载必须完整覆盖来源");
        }
        let mesh = super::super::clipmap::build_display_lod_mesh(&plan, Vec2::splat(0.25)).unwrap();
        assert_eq!(mesh.indices().unwrap().len() as u64, plan.index_count);
    }
}
