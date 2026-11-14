//! 三维材质/种植持久根、方向隔离、擦除和窗口读取的行为回归。
use super::super::tests::Temp;
use super::*;
fn brush(channel: VolumeFieldChannel, id: &str) -> VolumeFieldBrush {
    VolumeFieldBrush {
        channel,
        id: id.into(),
        from: [0.; 3],
        to: [0.; 3],
        normal: [0., 1., 0.],
        radius: 1.,
        strength: 1.,
        flow_exposure: 0.,
        erase: false,
        source_to_world: Mat3::IDENTITY,
    }
}

#[test]
fn one_spatial_cell_keeps_independent_parallel_and_opposite_surfaces_after_reload() {
    let temp=Temp::new(); let source=PagedVolume::new(0.5).unwrap();
    let mut top=brush(VolumeFieldChannel::Vegetation,"top-grass"); top.radius=0.1;
    let first=paint(&temp.0,&source,&top);
    let mut underside=brush(VolumeFieldChannel::Vegetation,"underside-moss");
    underside.radius=0.1; underside.from=[0.,-0.05,0.]; underside.to=underside.from; underside.normal=[0.,-1.,0.];
    let window=load_field_window(&temp.0,&first,&underside).unwrap();
    let delta=prepare_field_paint(window,&first,&underside,|p,d,maximum| {
        let t=(-0.05-p.y)/d.y;
        (t>=0. && t<=maximum).then_some((p+d*t,Vec3::NEG_Y))
    }).unwrap();
    let second=write_field_paint(&temp.0,&first,delta).unwrap();
    let read=load_surface_fields(&temp.0,&second,&[[0.;3]]).unwrap();
    assert_eq!(read.samples_at(&[0,0,0]).count(),2,"同格的反面不能覆盖原种植面");
    assert_eq!(read.sample(Vec3::ZERO,Vec3::Y).unwrap().vegetation,vec![("top-grass".into(),255)]);
    assert_eq!(read.sample(Vec3::new(0.,-0.05,0.),Vec3::NEG_Y).unwrap().vegetation,vec![("underside-moss".into(),255)]);
    let mut parallel=brush(VolumeFieldChannel::Vegetation,"low-grass");
    parallel.radius=0.1;parallel.from=[0.,0.05,0.];parallel.to=parallel.from;
    let window=load_field_window(&temp.0,&second,&parallel).unwrap();
    let delta=prepare_field_paint(window,&second,&parallel,|p,d,maximum| {
        let t=(0.05-p.y)/d.y;
        (t>=0. && t<=maximum).then_some((p+d*t,Vec3::Y))
    }).unwrap();
    let third=write_field_paint(&temp.0,&second,delta).unwrap();
    let read=load_surface_fields(&temp.0,&third,&[[0.;3]]).unwrap();
    assert_eq!(read.samples_at(&[0,0,0]).count(),3,"同向但不重合的楼板也拥有独立作者数据");
    assert_eq!(read.sample(Vec3::new(0.,0.05,0.),Vec3::Y).unwrap().vegetation,vec![("low-grass".into(),255)]);
    let restored=load_surface_fields(&temp.0,&first,&[[0.;3]]).unwrap();
    assert_eq!(restored.iter_samples().count(),1,"撤销仍复用原持久根，不反写附加表面");
}
fn plane(p: Vec3, n: Vec3, max: f32) -> Option<(Vec3, Vec3)> {
    let t = -p.y / n.y;
    (t >= 0. && t <= max).then_some((p + n * t, Vec3::Y))
}
fn paint(root: &Path, source: &PagedVolume, brush: &VolumeFieldBrush) -> PagedVolume {
    let input = load_field_window(root, source, brush).unwrap();
    let delta = prepare_field_paint(input, source, brush, plane).unwrap();
    write_field_paint(root, source, delta).unwrap()
}

#[test]
fn scaled_field_paint_stays_a_world_circle_and_keeps_source_lattice() {
    let temp=Temp::new(); let source=PagedVolume::new(0.25).unwrap();
    let mut dab=brush(VolumeFieldChannel::Surface,"stone");
    dab.source_to_world=Mat3::from_diagonal(Vec3::new(4.,2.,0.5));
    let next=paint(&temp.0,&source,&dab);
    let read=load_surface_fields(&temp.0,&next,&[[0.;3]]).unwrap();
    assert!(!read.samples.contains_key(&[2,0,0])); // 源X半米在世界已经是两米，超出一米笔刷。
    assert!(read.samples[&[0,0,6]].surface[0].1>0); // 源Z一米半在世界仍在圆内。
    assert_eq!(read.samples[&[0,0,6]].position,[0.,0.,1.5]);
    assert_eq!(next.cell_size,source.cell_size);
}

#[test]
fn queued_flow_preserves_radial_density_in_persisted_paint_and_erase() {
    let temp = Temp::new();
    let source = PagedVolume::new(0.25).unwrap();
    let mut dab = brush(VolumeFieldChannel::Vegetation, "grass");
    dab.strength = 0.6;
    dab.flow_exposure = 4.0; // 首次点按后累计一秒全强度，边缘应先衰减曝光再饱和。
    let next = paint(&temp.0, &source, &dab);
    let read = load_surface_fields(&temp.0, &next, &[[0.; 3]]).unwrap();
    let edge = read.samples[&[2, 0, 0]].vegetation[0].1;
    let expected = ((1.0_f32 - 0.7 * (-2.0_f32).exp()) * 255.0).round() as u8;
    assert_eq!(
        edge, expected,
        "半径中点不能把中心饱和强度简单减半而刷得偏淡"
    );
    dab.erase = true;
    dab.strength = 0.3;
    dab.flow_exposure = 1.0;
    let erased = paint(&temp.0, &next, &dab);
    let read = load_surface_fields(&temp.0, &erased, &[[0.; 3]]).unwrap();
    assert_eq!(
        read.samples[&[2, 0, 0]].vegetation[0].1,
        (edge as f32 * 0.85 * (-0.5_f32).exp()).round() as u8
    );
    dab.flow_exposure = f32::INFINITY;
    assert!(
        load_field_window(&temp.0, &erased, &dab).is_err(),
        "非法曝光不能进入IO与作者写入"
    );
}

#[test]
fn flow_response_does_not_depend_on_async_frame_partitioning() {
    for falloff in [0., 0.1, 0.5, 0.8, 1.] {
        let mut value = field_paint_response(0.6, 0., falloff);
        for _ in 0..60 {
            value += (1.0 - value) * field_paint_response(0., 4.0 / 60.0, falloff);
        }
        assert!(
            (value - field_paint_response(0.6, 4., falloff)).abs() < 1e-5,
            "合并后台等待帧不能改变笔刷中心或边缘的实际响应"
        );
    }
}

#[test]
fn independent_field_root_round_trips_and_undo_reuses_previous_root() {
    let temp = Temp::new();
    let before = PagedVolume::new(0.25).unwrap();
    let soil = paint(
        &temp.0,
        &before,
        &brush(VolumeFieldChannel::Surface, "rock"),
    );
    let planted = paint(
        &temp.0,
        &soil,
        &brush(VolumeFieldChannel::Vegetation, "cave-grass"),
    );
    assert_eq!(planted.density, before.density);
    assert_eq!(planted.meshes, before.meshes);
    assert_ne!(planted.fields, soil.fields);
    let bytes = super::super::encode_paged_volume(&planted).unwrap();
    let restored = super::super::decode_paged_volume(&bytes).unwrap().unwrap();
    assert_eq!(restored, planted);
    let read = load_surface_fields(&temp.0, &restored, &[[0.; 3]]).unwrap();
    let sample = read.sample(Vec3::ZERO, Vec3::Y).unwrap();
    assert_eq!(sample.surface[0], ("rock".into(), 255));
    assert_eq!(sample.vegetation[0], ("cave-grass".into(), 255));
    assert!(read.sample(Vec3::ZERO, -Vec3::Y).is_none());
    let undone = load_surface_fields(&temp.0, &soil, &[[0.; 3]]).unwrap();
    assert!(
        undone
            .sample(Vec3::ZERO, Vec3::Y)
            .unwrap()
            .vegetation
            .is_empty()
    );
    assert!(
        load_surface_fields(&temp.0, &before, &[[0.; 3]])
            .unwrap()
            .samples
            .is_empty()
    );
}
#[test]
fn erasing_surface_preserves_vegetation_and_empty_strokes_reuse_root() {
    let temp = Temp::new();
    let initial = PagedVolume::new(0.25).unwrap();
    let surface = paint(
        &temp.0,
        &initial,
        &brush(VolumeFieldChannel::Surface, "soil"),
    );
    let planted = paint(
        &temp.0,
        &surface,
        &brush(VolumeFieldChannel::Vegetation, "grass"),
    );
    let mut erase = brush(VolumeFieldChannel::Surface, "soil");
    erase.erase = true;
    let erased = paint(&temp.0, &planted, &erase);
    let read = load_surface_fields(&temp.0, &erased, &[[0.; 3]]).unwrap();
    let sample = read.sample(Vec3::ZERO, Vec3::Y).unwrap();
    assert!(sample.surface.is_empty());
    assert_eq!(sample.vegetation[0].0, "grass");
    let mut zero = erase;
    zero.strength = 0.;
    assert_eq!(paint(&temp.0, &erased, &zero), erased);
}
#[test]
fn flat_simplified_mesh_still_loads_interior_planting_cells() {
    let temp = Temp::new();
    let source = paint(
        &temp.0,
        &PagedVolume::new(0.25).unwrap(),
        &brush(VolumeFieldChannel::Vegetation, "grass"),
    );
    let plan = plan_surface_fields(&source, &[[24., 0., 24.]], &[[0.; 3]], 1);
    let read = decode_surface_fields(
        &source,
        read_surface_fields(&temp.0, &source, plan).unwrap(),
    )
    .unwrap();
    assert!(read.sample(Vec3::ZERO, Vec3::Y).is_some());
}
#[test]
fn far_proxy_triangle_interior_keeps_paint_and_plants_without_a_nearby_focus() {
    let temp = Temp::new();
    let base = PagedVolume::new(0.25).unwrap();
    let surface = paint(&temp.0, &base, &brush(VolumeFieldChannel::Surface, "soil"));
    let source = paint(
        &temp.0,
        &surface,
        &brush(VolumeFieldChannel::Vegetation, "grass"),
    );
    let positions = [
        [-24., 0., -24.],
        [24., 0., -24.],
        [-24., 0., 24.],
        [24., 0., 24.],
    ];
    let plan = plan_surface_fields(&source, &positions, &[[200., 0., 200.]], 0);
    let read = decode_surface_fields(
        &source,
        read_surface_fields(&temp.0, &source, plan).unwrap(),
    )
    .unwrap();
    let sample = read.sample(Vec3::ZERO, Vec3::Y).unwrap();
    assert_eq!(sample.surface[0].0, "soil");
    assert_eq!(sample.vegetation[0].0, "grass");
    let remote = plan_surface_fields(&source, &[[200., 0., 200.]], &[], 0);
    assert!(
        decode_surface_fields(
            &source,
            read_surface_fields(&temp.0, &source, remote).unwrap()
        )
        .unwrap()
        .samples
        .is_empty()
    );
}
#[test]
fn cave_floor_and_ceiling_keep_independent_channels_and_normals() {
    let temp = Temp::new();
    let base = PagedVolume::new(0.25).unwrap();
    let floor = paint(
        &temp.0,
        &base,
        &brush(VolumeFieldChannel::Vegetation, "floor-grass"),
    );
    let mut ceiling = brush(VolumeFieldChannel::Surface, "ceiling-rock");
    ceiling.from = [0., 2., 0.];
    ceiling.to = ceiling.from;
    ceiling.normal = [0., -1., 0.];
    let window = load_field_window(&temp.0, &floor, &ceiling).unwrap();
    let delta = prepare_field_paint(window, &floor, &ceiling, |p, d, max| {
        let t = (2. - p.y) / d.y;
        (t >= 0. && t <= max).then_some((p + d * t, -Vec3::Y))
    })
    .unwrap();
    let both = write_field_paint(&temp.0, &floor, delta).unwrap();
    let read = load_surface_fields(&temp.0, &both, &[[0.; 3], [0., 2., 0.]]).unwrap();
    let a = read.sample(Vec3::ZERO, Vec3::Y).unwrap();
    assert_eq!(a.vegetation[0].0, "floor-grass");
    assert!(a.surface.is_empty());
    let b = read.sample(Vec3::Y * 2., -Vec3::Y).unwrap();
    assert_eq!(b.surface[0].0, "ceiling-rock");
    assert!(b.vegetation.is_empty());
    assert!(read.sample(Vec3::Y * 2., Vec3::Y).is_none());
}
#[test]
fn layered_puddles_round_trip_and_erase_without_changing_material_or_plants() {
    let temp = Temp::new();
    let base = PagedVolume::new(0.25).unwrap();
    let surface = paint(&temp.0, &base, &brush(VolumeFieldChannel::Surface, "soil"));
    let planted = paint(
        &temp.0,
        &surface,
        &brush(VolumeFieldChannel::Vegetation, "grass"),
    );
    let wet = paint(
        &temp.0,
        &planted,
        &brush(VolumeFieldChannel::Puddles, "puddles"),
    );
    let encoded = super::super::encode_paged_volume(&wet).unwrap();
    let restored = super::super::decode_paged_volume(&encoded)
        .unwrap()
        .unwrap();
    let read = load_surface_fields(&temp.0, &restored, &[[0.; 3]]).unwrap();
    let sample = read.sample(Vec3::ZERO, Vec3::Y).unwrap();
    assert_eq!(sample.puddles, 255);
    assert!(read.sample(Vec3::ZERO, -Vec3::Y).is_none());
    let mut erase = brush(VolumeFieldChannel::Puddles, "puddles");
    erase.erase = true;
    let dry = paint(&temp.0, &restored, &erase);
    let read = load_surface_fields(&temp.0, &dry, &[[0.; 3]]).unwrap();
    let sample = read.sample(Vec3::ZERO, Vec3::Y).unwrap();
    assert_eq!(sample.puddles, 0);
    assert_eq!(sample.surface[0].0, "soil");
    assert_eq!(sample.vegetation[0].0, "grass");
    assert_eq!(dry.density, planted.density);
    let old: VolumeFieldSample = serde_json::from_str(
        r#"{"position":[0,0,0],"normal":[0,1,0],"surface":[],"vegetation":[]}"#,
    )
    .unwrap();
    assert_eq!(old.puddles, 0);
}

#[test]
fn curved_cave_brush_follows_the_surface_beyond_the_click_tangent_band() {
    let temp = Temp::new();
    let source = PagedVolume::new(0.25).unwrap();
    let mut brush = brush(VolumeFieldChannel::Vegetation, "grass");
    brush.radius = 2.;
    let window = load_field_window(&temp.0, &source, &brush).unwrap();
    let delta = prepare_field_paint(window, &source, &brush, |p, d, max| {
        let height = 0.5 * p.x * p.x;
        let t = (height - p.y) / d.y;
        (t >= 0. && t <= max).then_some((p + d * t, Vec3::new(-p.x, 1., 0.).normalize()))
    })
    .unwrap();
    let next = write_field_paint(&temp.0, &source, delta).unwrap();
    let read = load_surface_fields(&temp.0, &next, &[[0.; 3]]).unwrap();
    let point = Vec3::new(1.5, 1.125, 0.);
    assert!(
        read.sample(point, Vec3::new(-1.5, 1., 0.).normalize())
            .is_some_and(|s| !s.vegetation.is_empty()),
        "半径内连续曲面不能被点击点的薄切平面截断"
    );
}

#[test]
fn brush_does_not_cross_a_gap_to_a_second_same_facing_floor() {
    // 包含远小于作者格的间隙：下层仍完整刷出圆形，不能只剩点击点或串到上层。
    for gap in [0.05, 0.2, 0.5] {
    let temp = Temp::new();
    let source = PagedVolume::new(0.25).unwrap();
    let brush = brush(VolumeFieldChannel::Vegetation, "grass");
    let window = load_field_window(&temp.0, &source, &brush).unwrap();
    let delta = prepare_field_paint(window, &source, &brush, |p, d, max| {
        [0., gap]
            .into_iter()
            .filter_map(|y| {
                let t = (y - p.y) / d.y;
                (t >= 0. && t <= max).then_some(t)
            })
            .min_by(f32::total_cmp)
            .map(|t| (p + d * t, Vec3::Y))
    })
    .unwrap();
    let next = write_field_paint(&temp.0, &source, delta).unwrap();
    let read = load_surface_fields(&temp.0, &next, &[[0.; 3]]).unwrap();
    assert!(read.sample(Vec3::ZERO, Vec3::Y).is_some());
    assert!(
        !read.iter_samples().any(|s| s.position[1] > 0.00001),
        "拓宽曲面覆盖不能穿过空隙刷到另一层"
    );
    assert!(read.iter_samples().count() > 12, "同格近邻层不能抢走已点击下层的完整笔刷覆盖");
    }
}

#[test]
fn brush_smaller_than_an_author_cell_still_paints_the_actual_cursor_surface() {
    let temp = Temp::new();
    let source = PagedVolume::new(0.5).unwrap();
    let mut brush = brush(VolumeFieldChannel::Vegetation, "grass");
    brush.radius = 0.1;
    brush.from = [0.2, 0., 0.2];
    brush.to = brush.from;
    let next = paint(&temp.0, &source, &brush);
    let read = load_surface_fields(&temp.0, &next, &[[0.; 3]]).unwrap();
    assert!(
        read.sample(Vec3::from_array(brush.to), Vec3::Y)
            .is_some_and(|s| s.vegetation == vec![("grass".into(), 255)])
    );
}
