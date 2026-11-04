//! 观察LOD只裁已编译前缀；矩形真实格距不能在相机移动时被再次量化。
use super::*;

fn source() -> (SparseHeightSpec, CompiledDisplayLodPlan) {
    let spec = SparseHeightSpec::new([0.0; 2], [64.0, 65.0], 0.3, -8.0, 16.0).unwrap();
    let authored = compile_display_lod_plan(
        &spec,
        &[
            MapTerrainDisplayLod {
                outer_half_extent_meters: Some(8.0),
                spacing_multiplier: 1,
            },
            MapTerrainDisplayLod {
                outer_half_extent_meters: Some(16.0),
                spacing_multiplier: 2,
            },
            MapTerrainDisplayLod {
                outer_half_extent_meters: Some(32.0),
                spacing_multiplier: 4,
            },
            MapTerrainDisplayLod {
                outer_half_extent_meters: None,
                spacing_multiplier: 8,
            },
        ],
    )
    .unwrap();
    assert_ne!(spec.actual_spacing_meters[0], spec.actual_spacing_meters[1]);
    assert_ne!(
        authored.actual_half_extents_meters[0][0],
        authored.actual_half_extents_meters[0][1]
    );
    (spec, authored)
}

#[test]
fn compiled_lod_suffix_preserves_unequal_actual_spacing_and_each_finite_axis_boundary() {
    let (spec, authored) = source();
    let unchanged = compile_display_lod_plan_suffix(&spec, &authored, 1, spec.cells).unwrap();
    assert_eq!(
        unchanged, authored,
        "未剔除层时，计数、轴边界、页范围与作者身份必须原样保留"
    );
    for minimum in [2, 4, 8] {
        let first = authored
            .bands
            .iter()
            .position(|band| band.spacing_multiplier == minimum)
            .unwrap();
        let plan = compile_display_lod_plan_suffix(&spec, &authored, minimum, spec.cells).unwrap();
        assert_eq!(plan.authored_lods, authored.authored_lods);
        assert_eq!(
            plan.actual_half_extents_meters,
            authored.actual_half_extents_meters[first..]
        );
        for (index, (actual, old)) in plan.bands.iter().zip(&authored.bands[first..]).enumerate() {
            assert_eq!(actual.outer_half_cells, old.outer_half_cells);
            assert_eq!(actual.source_lod_index, old.source_lod_index);
            assert_eq!(actual.spacing_multiplier, old.spacing_multiplier);
            assert_eq!(
                actual.inner_half_cells,
                if index == 0 {
                    [0, 0]
                } else {
                    old.inner_half_cells
                }
            );
        }
        assert!(plan.mesh_bytes <= MAX_DISPLAY_LOD_MESH_BYTES);
        assert!(
            worst_case_display_page_upload_bytes(&spec, &plan).unwrap()
                <= MAX_DISPLAY_LOD_PAGE_BYTES
        );
    }
}

#[test]
fn compiled_lod_suffix_retains_internal_two_to_one_bands_and_original_author_rows() {
    let (spec, _) = source();
    let authored = compile_display_lod_plan(
        &spec,
        &[
            MapTerrainDisplayLod {
                outer_half_extent_meters: Some(8.0),
                spacing_multiplier: 1,
            },
            MapTerrainDisplayLod {
                outer_half_extent_meters: Some(32.0),
                spacing_multiplier: 8,
            },
            MapTerrainDisplayLod {
                outer_half_extent_meters: None,
                spacing_multiplier: 16,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        authored
            .bands
            .iter()
            .map(|band| band.source_lod_index)
            .collect::<Vec<_>>(),
        [Some(0), None, None, Some(1), Some(2)]
    );
    for minimum in [1, 2, 4, 8, 16] {
        let first = authored
            .bands
            .iter()
            .position(|band| band.spacing_multiplier == minimum)
            .unwrap();
        let plan = compile_display_lod_plan_suffix(&spec, &authored, minimum, spec.cells).unwrap();
        assert_eq!(plan.authored_lods, authored.authored_lods);
        assert_eq!(
            plan.actual_half_extents_meters,
            authored.actual_half_extents_meters[first..]
        );
        assert_eq!(
            plan.bands
                .iter()
                .map(|band| band.source_lod_index)
                .collect::<Vec<_>>(),
            authored.bands[first..]
                .iter()
                .map(|band| band.source_lod_index)
                .collect::<Vec<_>>()
        );
        for pair in plan.bands.windows(2) {
            assert_eq!(pair[1].spacing_multiplier, pair[0].spacing_multiplier * 2);
            for axis in 0..2 {
                assert_eq!(
                    u64::from(pair[0].outer_half_cells[axis])
                        * u64::from(pair[0].spacing_multiplier),
                    u64::from(pair[1].inner_half_cells[axis])
                        * u64::from(pair[1].spacing_multiplier)
                );
            }
        }
    }
}

#[test]
fn compiled_lod_suffix_adjusts_only_last_coverage_and_keeps_valid_one_axis_outer_ring() {
    let (spec, authored) = source();
    let original = authored.clone();
    let plan = compile_display_lod_plan_suffix(&spec, &authored, 2, [600, 0]).unwrap();
    assert_eq!(
        plan.bands[0].outer_half_cells,
        authored.bands[1].outer_half_cells
    );
    assert_eq!(
        plan.bands[1].outer_half_cells,
        authored.bands[2].outer_half_cells
    );
    let last = plan.bands.last().unwrap();
    assert_eq!(
        last.inner_half_cells,
        authored.bands.last().unwrap().inner_half_cells
    );
    assert_eq!(last.outer_half_cells, [75, last.inner_half_cells[1]]);
    assert_eq!(last.source_lod_index, Some(3));
    assert!(
        compile_display_lod_plan_suffix(&spec, &authored, 2, [0, 0]).is_err(),
        "末层两轴都缩入前层会退化，必须明确拒绝，不能更改有限层半径"
    );
    assert_eq!(authored, original);
}

#[test]
fn compiled_lod_suffix_rejects_unknown_multipliers_overflow_and_existing_mesh_budget_failure() {
    let (spec, authored) = source();
    let original = authored.clone();
    for multiplier in [0, 3, 16, 32768] {
        assert!(compile_display_lod_plan_suffix(&spec, &authored, multiplier, spec.cells).is_err());
    }
    assert!(compile_display_lod_plan_suffix(&spec, &authored, 8, [u64::MAX; 2]).is_err());
    let error = compile_display_lod_plan_suffix(&spec, &authored, 8, [4096; 2]).unwrap_err();
    assert!(
        error.contains("Mesh") && error.contains("预算"),
        "保留既有16MiB门：{error}"
    );
    assert_eq!(authored, original, "失败不改作者计划，不偷偷扩大末档倍率");
}

#[test]
fn compiled_lod_suffix_rejects_invalid_spacing_and_corrupt_compiled_band_identity() {
    let (spec, authored) = source();
    for spacing in [0.0, f64::NAN, f64::INFINITY] {
        let mut bad = spec.clone();
        bad.actual_spacing_meters[0] = spacing;
        assert!(compile_display_lod_plan_suffix(&bad, &authored, 1, spec.cells).is_err());
    }
    for corruption in 0..5 {
        let mut bad = authored.clone();
        match corruption {
            0 => bad.bands.last_mut().unwrap().spacing_multiplier = 0,
            1 => bad.bands[0].spacing_multiplier = 0,
            2 => bad.bands[1].spacing_multiplier = 4,
            3 => bad.bands[1].inner_half_cells[0] += 1,
            _ => bad.bands[0].source_lod_index = Some(999),
        }
        let minimum = bad.bands[0].spacing_multiplier;
        assert!(
            compile_display_lod_plan_suffix(&spec, &bad, minimum, spec.cells).is_err(),
            "公开helper不能把零倍率、断开的2:1共同边界或错误作者行传给Mesh：{corruption}"
        );
    }
}
