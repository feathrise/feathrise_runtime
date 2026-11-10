//! 以真实相机验证作者档位、跨来源观察点和实际拓扑，避免只改线框材质。
use super::*;
use bevy::{
    camera::{CameraProjection, ScalingMode, Viewport},
    ecs::system::SystemState,
};
use feathrise_scene_content::{SceneTerrainDisplayLod, compile_display_lod_plan, default_display_lods_for_spec};

fn source() -> (SparseHeightSpec, CompiledDisplayLodPlan) {
    let spec = SparseHeightSpec::new([-64.0; 2], [64.0; 2], 0.25, -64.0, 128.0).unwrap();
    let authored =
        compile_display_lod_plan(&spec, &default_display_lods_for_spec(&spec).unwrap()).unwrap();
    (spec, authored)
}

fn perspective(position: Vec3, target: Vec3, fov: f32, size: f32) -> ClipmapView {
    ClipmapView::new(
        &GlobalTransform::from(Transform::from_translation(position).looking_at(target, Vec3::Z)),
        &Projection::Perspective(PerspectiveProjection {
            fov,
            aspect_ratio: 1.0,
            ..default()
        }),
        Vec2::splat(size),
    )
    .unwrap()
}

#[test]
fn distance_changes_actual_topology_without_modifying_author_lods() {
    let (spec, authored) = source();
    let mut counts = Vec::new();
    let mut scales = Vec::new();
    for distance in [10.0, 24.0, 80.0] {
        let view = perspective(Vec3::Y * distance, Vec3::ZERO, 1.0, 1024.0);
        let (_, key) = view
            .choice(&spec, SceneSourceFrame::default(), [0.0; 2], None, &authored)
            .unwrap();
        let plan = compile_view_plan(&spec, &authored, key).unwrap();
        let mesh = super::super::clipmap::build_display_lod_mesh(&plan, Vec2::splat(0.25)).unwrap();
        assert_eq!(mesh.count_vertices() as u64, plan.vertex_count);
        assert_eq!(mesh.indices().unwrap().len() as u64, plan.index_count);
        assert_eq!(plan.authored_lods, authored.authored_lods);
        assert!(
            plan.sample_ranges
                .iter()
                .all(|range| range.mip_level <= spec.max_mip_level())
        );
        counts.push(plan.index_count);
        scales.push(key.minimum_multiplier);
    }
    assert!(
        counts.windows(2).all(|pair| pair[0] > pair[1]),
        "拉远必须减少真实三角形：{counts:?}"
    );
    assert!(
        scales.windows(2).all(|pair| pair[0] < pair[1]),
        "远近格距：{scales:?}"
    );
}

#[test]
fn distant_source_uses_same_world_camera_without_local_dense_center() {
    let (spec, authored) = source();
    let view = perspective(
        Vec3::new(0.0, 20.0, 30.0),
        Vec3::new(0.0, 0.0, -120.0),
        1.0,
        1024.0,
    );
    let near_frame = SceneSourceFrame::default();
    let far_frame = SceneSourceFrame::from_transform(Transform::from_xyz(0.0, 0.0, -400.0));
    let (near_focus, near) = view.choice(&spec, near_frame, [0.0; 2], None, &authored).unwrap();
    let (far_focus, far) = view.choice(&spec, far_frame, [0.0; 2], None, &authored).unwrap();
    assert_eq!(
        near_frame.to_world(near_focus.extend(0.0).xzy()),
        Vec3::new(0.0, 0.0, 30.0)
    );
    assert_eq!(far_focus, Vec2::ZERO, "全源末档仅调整均匀布局，不在远来源复制细环中心");
    assert!(
        far.minimum_multiplier > near.minimum_multiplier,
        "远对象不能保留近场精度：{near:?} / {far:?}"
    );
    let plan = compile_view_plan(&spec, &authored, far).unwrap();
    assert_eq!(plan.bands.len(), 1);
    assert_eq!(plan.bands[0].spacing_multiplier, authored.bands.last().unwrap().spacing_multiplier);
    let half = Vec2::from_array(*plan.actual_half_extents_meters.last().unwrap());
    for corner in [
        Vec2::from_array(spec.world_min),
        Vec2::from_array(spec.world_max),
    ] {
        assert!(
            (corner - far_focus).abs().cmple(half).all(),
            "共享中心仍须完整覆盖远网格"
        );
    }
}

#[test]
fn lens_viewport_and_orthographic_zoom_preserve_configured_lods() {
    let (spec, authored) = source();
    let key = |view: ClipmapView| {
        view.choice(&spec, SceneSourceFrame::default(), [0.0; 2], None, &authored)
            .unwrap()
            .1
            .minimum_multiplier
    };
    let normal = perspective(Vec3::Y * 100.0, Vec3::ZERO, 1.0, 1024.0);
    assert_eq!(key(perspective(Vec3::Y * 100.0, Vec3::ZERO, 0.25, 1024.0)), key(normal));
    assert_eq!(key(perspective(Vec3::Y * 100.0, Vec3::ZERO, 1.0, 4096.0)), key(normal));
    let transform =
        GlobalTransform::from(Transform::from_xyz(0.0, 100.0, 0.0).looking_at(Vec3::ZERO, Vec3::Z));
    let ortho = |span| {
        let mut projection = OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: span,
                height: span,
            },
            ..OrthographicProjection::default_3d()
        };
        projection.update(1024.0, 1024.0);
        ClipmapView::new(
            &transform,
            &Projection::Orthographic(projection),
            Vec2::splat(1024.0),
        )
        .unwrap()
    };
    assert_eq!(key(ortho(1024.0)), key(ortho(64.0)));
    assert!(ClipmapView::new(&transform, &Projection::default(), Vec2::splat(f32::NAN)).is_none());
}

#[test]
fn configured_height_thresholds_choose_only_the_authored_bands() {
    let (spec, authored) = source();
    let choose = |height, previous| {
        perspective(Vec3::Y * height, Vec3::ZERO, 1.0, 1024.0)
            .choice(&spec, SceneSourceFrame::default(), [0.0; 2], previous, &authored)
            .unwrap()
            .1
    };
    let mut previous = None;
    for (height, expected) in [(15.9, 1), (16.1, 2), (32.1, 4), (64.1, 8), (100_000.0, 8), (15.9, 1)] {
        let key = choose(height, previous);
        assert_eq!(key.minimum_multiplier, expected, "高度{height}必须按16/32/64米作者半径选档");
        previous = Some(key);
    }
}

#[test]
fn camera_selection_ignores_thumbnail_and_falls_back_when_editor_closes() {
    let mut world = World::new();
    let camera = |position| {
        (
            Camera3d::default(),
            Camera {
                viewport: Some(Viewport {
                    physical_size: UVec2::splat(1024),
                    ..default()
                }),
                ..default()
            },
            Projection::default(),
            GlobalTransform::from_translation(position),
        )
    };
    world.spawn(camera(Vec3::splat(999.0))); // 无关缩略图先创建，也不能被选中。
    let game = world.spawn(camera(Vec3::new(10.0, 20.0, 30.0))).id();
    let editor = world
        .spawn((
            camera(Vec3::new(40.0, 50.0, 60.0)),
            DynamicClipmapAuthoringView {
                focus_xz: Vec2::splat(-999.0),
                focus_y: 0.0,
            },
        ))
        .id();
    let mut state = SystemState::<ClipmapViews>::new(&mut world);
    assert_eq!(
        state
            .get(&world)
            .unwrap()
            .resolve(Some(game))
            .unwrap()
            .position,
        Vec3::new(40.0, 50.0, 60.0)
    );
    assert_eq!(
        state.get(&world).unwrap().resolve(None).unwrap().position,
        Vec3::new(40.0, 50.0, 60.0)
    );
    world.get_mut::<Camera>(editor).unwrap().is_active = false;
    assert_eq!(
        state
            .get(&world)
            .unwrap()
            .resolve(Some(game))
            .unwrap()
            .position,
        Vec3::new(10.0, 20.0, 30.0)
    );
    assert!(state.get(&world).unwrap().resolve(None).is_none());
}

#[test]
fn shared_camera_coverage_budget_cannot_invent_another_authored_lod() {
    let spec = SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.25, -8.0, 16.0).unwrap();
    let authored = compile_display_lod_plan(&spec, &[
        SceneTerrainDisplayLod { outer_half_extent_meters: Some(16.0), spacing_multiplier: 1 },
        SceneTerrainDisplayLod { outer_half_extent_meters: None, spacing_multiplier: 2 },
    ]).unwrap();
    // 111第二个网格的首载条件：共享中心扩张为512基础格，原投影档生成16818216字节Mesh。
    let key = ViewLodKey { minimum_multiplier: 2, coverage_cells: [512; 2] };
    assert!(compile_view_plan(&spec, &authored, key).unwrap_err().contains("16818216"));
    assert!(compile_view_plan(&spec, &authored, ViewLodKey { minimum_multiplier: 4, ..key }).is_err());
    // 极远来源已经完整处于作者末档，用固定均匀布局覆盖自身，不浪费源外整环的预算。
    let (focus, distant_key) = perspective(Vec3::new(100_000.0, 20.0, 0.0), Vec3::ZERO, 1.0, 1024.0)
        .choice(&spec, SceneSourceFrame::default(), [0.0; 2], None, &authored).unwrap();
    assert_eq!(focus, Vec2::ZERO);
    let distant = compile_view_plan(&spec, &authored, distant_key).unwrap();
    assert_eq!(distant.bands.len(), 1);
    assert_eq!(distant.bands[0].spacing_multiplier, 2);
    assert!(distant.mesh_bytes <= feathrise_scene_content::MAX_DISPLAY_LOD_MESH_BYTES);
    assert_eq!(distant.authored_lods, authored.authored_lods);
    let mesh = super::super::clipmap::build_display_lod_mesh(&distant, Vec2::splat(0.25)).unwrap();
    assert_eq!(mesh.indices().unwrap().len() as u64, distant.index_count);
    let returned = compile_view_plan(&spec, &authored, ViewLodKey { coverage_cells: [256; 2], ..key }).unwrap();
    assert_eq!(returned.bands[0].spacing_multiplier, 2);
}

#[test]
fn camera_coverage_overflow_returns_an_error_without_allocating_geometry() {
    let (spec, authored) = source();
    assert!(compile_view_plan(&spec, &authored,
        ViewLodKey { minimum_multiplier: 1, coverage_cells: [u64::MAX; 2] }).is_err());
}

#[test]
fn coverage_boundary_hysteresis_prevents_rebuild_churn_without_clipping() {
    let (spec, authored) = source();
    let choose = |x, previous| perspective(Vec3::new(x, 20.0, 0.0), Vec3::new(x, 0.0, 0.0), 1.0, 1024.0)
        .choice(&spec, SceneSourceFrame::default(), [0.0; 2], previous, &authored).unwrap();
    let initial = choose(0.0, None).1;
    let crossing = (1..2048).map(|step| step as f32 * 0.25)
        .find(|&x| choose(x, Some(initial)).1.coverage_cells != initial.coverage_cells).unwrap();
    let mut previous = None;
    let mut changes = 0;
    for frame in 0..64 {
        let x = if frame % 2 == 0 { crossing - 0.25 } else { crossing };
        let (focus, key) = choose(x, previous);
        if previous != Some(key) { changes += 1; }
        let radius = (focus - Vec2::from_array(spec.world_min)).abs()
            .max((focus - Vec2::from_array(spec.world_max)).abs());
        assert!(radius.x / 0.25 <= key.coverage_cells[0] as f32);
        previous = Some(key);
    }
    eprintln!("覆盖边界{x:?}的64次往返计划变更次数：{changes}", x = crossing);
    assert_eq!(changes, 2, "首次方案与一次向外扩展后，边界往返必须复用完整拓扑");
    assert_eq!(choose(0.0, previous).1.coverage_cells, initial.coverage_cells,
        "回到内侧后必须恢复较小包络，不能永久保持扩大状态");
}
