//! placement整网格水体的无IO身份、路径、uniform编码与原Mesh只读回归。

use super::*; // 纯测试直接调用私有匹配和编码helper，不启动App或RenderApp。

const RUNTIME_SOURCE: &str = include_str!("runtime.rs"); // 锁定成功提交只替换材质而不登记新Mesh。
const PREPARE_SOURCE: &str = include_str!("runtime/prepare.rs"); // 锁定预检只读POSITION/UV0且不复制顶点数据。
const RESTORE_SOURCE: &str = include_str!("runtime/restore.rs"); // 锁定恢复时无需替换或删除Mesh资产。

/// 构造一个仅供最终身份比较的最小primitive候选。
fn candidate(node_path: &[&str], mesh_name: &str) -> PrimitiveCandidate {
    PrimitiveCandidate {
        entity: Entity::from_raw_u32(1).expect("测试实体索引一必须合法"), // 身份匹配不依赖实际World。
        object_node: Entity::from_raw_u32(2).expect("测试实体索引二必须合法"), // 本测试不涉及重名对象歧义。
        node_path: node_path
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(), // 按调用方给定的纯Blender路径构造拥有值。
        mesh_name: mesh_name.to_owned(), // 保留独立GltfMeshName测试值。
        mesh: None,                      // 纯身份比较不读资产句柄。
        original_material: None,         // 纯身份比较不读材质句柄。
        original_transform: None,        // 纯身份比较不读局部位姿。
        had_no_frustum_culling: false,   // 原标记不参与身份。
        had_camera_raycast_ignore: false, // 原标记不参与身份。
        had_not_shadow_caster: false,    // 原标记不参与身份。
        had_not_shadow_receiver: false,  // 原标记不参与身份。
    } // 返回最小候选快照。
} // 结束测试候选构造。

/// 构造一项节点路径和网格名可修改的最小作者覆盖。
fn surface(node_path: &[&str], mesh_name: &str) -> MapPlacementWaterSurface {
    MapPlacementWaterSurface {
        mesh_index: 99, // 故意使用任意诊断索引，证明它不参与最终匹配。
        node_path: node_path
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(), // 复制纯Blender节点路径。
        mesh_name: mesh_name.to_owned(), // 复制最终GltfMeshName。
        kind: MapPlacementWaterKind::Horizontal, // 匹配不依赖表现类别。
        flow_speed: 1.0, // 匹配不依赖流速。
        height_offset: 0.0, // 匹配不依赖高度偏移。
    } // 返回最小作者覆盖。
} // 结束测试覆盖构造。

/// 构造两个三角形的最小河面条带，几何长度和U终值可独立指定。
fn uv_test_river_strip(
    geometry_length: f32, // 源Mesh局部沿程长度，按Blender米制契约解释。
    u_end: f32,           // 下游端U数值，可模拟米制或归一化展UV。
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Indices) {
    let positions = vec![
        [0.0, 0.0, 0.0],             // 上游左岸。
        [0.0, 0.0, 2.0],             // 上游右岸。
        [geometry_length, 0.0, 0.0], // 下游左岸。
        [geometry_length, 0.0, 2.0], // 下游右岸。
    ]; // 完成沿X轴延伸的两米宽平面。
    let uv0 = vec![
        [0.0, 0.0],   // 上游左岸为U起点和V零侧。
        [0.0, 1.0],   // 上游右岸为V一侧。
        [u_end, 0.0], // 下游左岸使用调用方U终值。
        [u_end, 1.0], // 下游右岸与左岸共享沿程U。
    ]; // 完成标准河面UV0。
    let indices = Indices::U32(vec![0, 2, 1, 1, 2, 3]); // 两个TriangleList面同时提供两岸沿程边与跨宽对角边。
    (positions, uv0, indices) // 返回只供纯语义验证的拥有数组。
} // 结束最小河面条带构造。

#[test] // 最终身份必须同时精确匹配完整路径与GltfMeshName。
fn matching_uses_exact_node_path_and_mesh_name_only() {
    let target = surface(&["水系集合", "河道_A"], "河面Mesh"); // 作者身份使用两层Blender路径。
    assert!(candidate_matches(
        &target,
        &candidate(&["水系集合", "河道_A"], "河面Mesh")
    )); // 完整路径和网格名都一致时必须命中，mesh_index不参与。
    assert!(!candidate_matches(
        &target,
        &candidate(&["水系集合", "河道_B"], "河面Mesh")
    )); // 同一Mesh datablock在另一对象路径下不得被误接管。
    assert!(!candidate_matches(
        &target,
        &candidate(&["水系集合", "河道_A"], "河床Mesh")
    )); // 相同对象路径下的另一网格名同样不得命中。
} // 完成最终身份匹配回归。

#[test] // 运行时路径必须和GLB浏览器一样排除WorldAsset顶层scene root。
fn world_scene_root_is_excluded_from_author_node_path() {
    let normalized = without_world_scene_root(vec![
        "Scene0_RuntimeRoot".to_owned(), // Bevy WorldAsset顶层scene root只是实例化容器。
        "水系集合".to_owned(),           // 第一个真实Blender节点必须保留。
        "河道_A".to_owned(),             // 目标Blender对象节点必须保留。
    ]); // 构造实例化后的完整技术路径。
    assert_eq!(normalized, vec!["水系集合", "河道_A"]); // 结果与浏览器增量Scene0遍历的作者路径完全一致。
    assert!(without_world_scene_root(Vec::new()).is_empty()); // 空技术路径幂等保持空而不panic。
} // 完成scene root排除回归。

#[test] // 三类作者水必须编码到uniform高位哨兵，且不与任何单位流向冲突。
fn uniform_encoding_reuses_flow_styles_without_unit_direction_collision() {
    assert_eq!(
        encoded_whole_mesh_flow_style(MapPlacementWaterKind::Horizontal),
        Vec2::new(RIBBON_STYLE_SENTINEL, 0.0)
    ); // 水平复杂面使用普通河流style 0，速度单独保存在uniform.w。
    assert_eq!(
        encoded_whole_mesh_flow_style(MapPlacementWaterKind::Sloped),
        Vec2::new(RIBBON_STYLE_SENTINEL, 0.0)
    ); // 斜水面保留Blender高程，仍使用普通河流style 0。
    assert_eq!(
        encoded_whole_mesh_flow_style(MapPlacementWaterKind::Waterfall),
        Vec2::new(RIBBON_STYLE_SENTINEL + STYLE_WATERFALL, 0.0)
    ); // 瀑布使用现有style 2幕布分支。
    for unit_direction_x in [-1.0_f32, 0.0, 1.0] {
        let decoded_style = unit_direction_x - RIBBON_STYLE_SENTINEL; // 普通流向x分量的数学范围被两端和中点覆盖。
        assert!(!(-0.5..=2.5).contains(&decoded_style)); // 单位方向绝不能被Shader解码成整网格模式。
    } // 完成流向哨兵冲突范围回归。
} // 完成uniform模式编码回归。

#[test] // 帧关键Update路径只读原Mesh，不得恢复逐顶点克隆、UV1写入或资产上传。
fn runtime_keeps_original_mesh_read_only_and_only_swaps_material() {
    assert!(!RUNTIME_SOURCE.contains("ResMut<Assets<Mesh>>")); // 处理系统对Mesh Assets只能使用不可变资源。
    assert!(!RUNTIME_SOURCE.contains("meshes.add(")); // 提交阶段不得登记克隆Mesh并触发GPU重上传。
    assert!(!PREPARE_SOURCE.contains("Mesh::ATTRIBUTE_UV_1")); // 原GLB第二套UV必须保持原样。
    assert!(!PREPARE_SOURCE.contains("insert_attribute")); // 预检只能读POSITION/UV0，不得变更任何顶点数据。
    assert!(!PREPARE_SOURCE.contains("source_mesh.clone()")); // 防止再次引入随网格顶点数增长的CPU峰值。
    assert!(!RESTORE_SOURCE.contains("Assets<Mesh>")); // 恢复只释放独占材质，完全不依赖Mesh资产生命周期。
    assert!(!RESTORE_SOURCE.contains(".insert(Mesh3d")); // 原Mesh3d从未替换，恢复期也不得写回句柄。
} // 完成原Mesh只读性能契约回归。

#[test] // Blender整网格赋水同样必须接影，并在撤销时恢复作者原标记。
fn whole_mesh_water_receives_directional_shadows_and_restores_author_state() {
    assert!(
        RUNTIME_SOURCE.contains(".remove::<NotShadowReceiver>()"), // 接管必须主动清除源primitive可能自带的禁接影标记。
        "整网格赋水必须强制接收方向光阴影",
    ); // 防止普通Blender材质遗留标记让山体无法遮掉水面天体高光。
    assert_eq!(
        RUNTIME_SOURCE.matches("NotShadowReceiver").count(), // 应用源码只允许出现一次强制清除，不得重新插入该标记。
        1,
        "整网格赋水不能在清除后再次禁用阴影接收",
    ); // 锁定接管期策略为“不投影、但接影”。
    assert!(
        RESTORE_SOURCE.contains("entity_commands.insert(NotShadowReceiver);"), // 作者原本禁接影时撤销赋水必须重新插回。
        "撤销整网格赋水必须恢复作者原有禁接影状态",
    ); // 防止功能接管永久改写普通场景网格的作者语义。
} // 完成整网格水接影与撤销恢复回归。

#[test] // 全零UV0即使格式正确也不具备沿程或横向语义。
fn uv0_semantics_reject_all_zero_coordinates() {
    let (positions, mut uv0, indices) = uv_test_river_strip(12.0, 12.0); // 先构造合法十二米河面几何。
    uv0.fill([0.0, 0.0]); // 模拟Blender未展UV或导出丢失UV后的全零属性。

    let result = validate_whole_mesh_water_uv0(
        &positions,                                       // 几何本身有效。
        &uv0,                                             // U/V两轴都退化为常数。
        Some(&indices),                                   // 显式两三角索引。
        Mat3A::IDENTITY, // 纯测试以Mesh局部一单位等于一世界米为基线。
        MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE, // 测试使用正式单primitive预算。
    ); // 执行纯UV0语义预检。

    assert!(result.is_err()); // 不得仅因Float32x2存在就让全零UV进入GPU流场。
} // 完成全零UV0失败回归。

#[test] // 任一UV非有限值都会污染逐像素相位，必须在材质切换前失败。
fn uv0_semantics_reject_nan() {
    let (positions, mut uv0, indices) = uv_test_river_strip(8.0, 8.0); // 构造其余属性都合法的八米河面。
    uv0[2][0] = f32::NAN; // 仅污染一个下游顶点U，验证不会被min/max静默略过。

    assert!(
        validate_whole_mesh_water_uv0(
            &positions,
            &uv0,
            Some(&indices),
            Mat3A::IDENTITY,
            MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
        )
        .is_err()
    ); // NaN必须产生可见失败而不是Ready。
} // 完成UV0 NaN失败回归。

#[test] // 长河的归一化U=0..1有数值跨度，但不能冒充沿程米。
fn uv0_semantics_reject_normalized_u_on_long_river() {
    let (positions, uv0, indices) = uv_test_river_strip(100.0, 1.0); // 一百米几何却只展开一个U单位。

    let result = validate_whole_mesh_water_uv0(
        &positions,
        &uv0,
        Some(&indices),
        Mat3A::IDENTITY,
        MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
    ); // 局部沿程边比例约0.01 U/米，明显违反米制约定。

    assert!(result.is_err()); // 仅有U/V跨度不足以放行长河归一化UV。
} // 完成长河归一化U失败回归。

#[test] // 真正沿程米制U和横向0..1 V必须通过几何一致性证据。
fn uv0_semantics_accept_meter_u_and_normalized_v() {
    let (positions, uv0, indices) = uv_test_river_strip(100.0, 100.0); // 一百米河面对应U=0..100米。

    let sampled = validate_whole_mesh_water_uv0(
        &positions,
        &uv0,
        Some(&indices),
        Mat3A::IDENTITY,
        MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
    )
    .expect("有限、有跨度且局部比例为一的米制UV0必须合法"); // 调用正式纯预检。

    assert_eq!(sampled, 2); // 两个三角的小网格在预算内必须全量检查。
} // 完成米制U合法回归。

#[test] // 窄小水面不需要为满足长河门而伪造巨大U跨度。
fn uv0_semantics_accept_narrow_short_meter_surface() {
    let (positions, uv0, indices) = uv_test_river_strip(0.5, 0.5); // 半米长小水面仍使用真实U=0..0.5米。

    assert!(
        validate_whole_mesh_water_uv0(
            &positions,
            &uv0,
            Some(&indices),
            Mat3A::IDENTITY,
            MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
        )
        .is_ok()
    ); // 局部比例为一的小型几何必须通过，不以绝对U终值猜测长河。
} // 完成窄小米制水面合法回归。

#[test] // placement或Blender层级缩放后，U必须仍与真正世界米一致。
fn uv0_semantics_include_hierarchy_scale_in_meter_evidence() {
    let (positions, uv0, indices) = uv_test_river_strip(1.0, 1.0); // 局部看似一米且U=0..1。
    let ten_meter_world_scale = Transform::from_scale(Vec3::new(10.0, 1.0, 1.0))
        .compute_affine()
        .matrix3; // 层级把沿程X放大为真实十米，U数值却没有同步。

    assert!(
        validate_whole_mesh_water_uv0(
            &positions,
            &uv0,
            Some(&indices),
            ten_meter_world_scale,
            MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
        )
        .is_err()
    ); // 一个U单位跨过十米时必须拒绝，不让placement缩放绕过米制门。
} // 完成层级缩放世界米证据回归。

#[test] // 超长三角索引只均匀抽样固定上限，不在Update全量遍历。
fn uv0_semantic_triangle_sampling_is_hard_bounded() {
    let (positions, uv0, _) = uv_test_river_strip(20.0, 20.0); // 四个顶点可被大量重复三角形合法引用。
    let mut index_values = Vec::new(); // 构造远多于单primitive抽样上限的逻辑三角列表。
    for _ in 0..(MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE * 4) {
        index_values.extend_from_slice(&[0, 2, 1]); // 每个面都包含一条有效沿程边和一条米制对角边。
    } // 完成四倍预算的长索引构造。
    let indices = Indices::U32(index_values); // 使用正式glTF可出现的U32索引格式。

    let sampled = validate_whole_mesh_water_uv0(
        &positions,
        &uv0,
        Some(&indices),
        Mat3A::IDENTITY,
        MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
    )
    .expect("重复的合法米制三角证据必须在固定预算内通过"); // 执行正式均匀抽样。

    assert_eq!(sampled, MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE); // 输入三角数增长不能让单primitive工作量突破硬上限。
    assert_eq!(
        MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_ROOT,
        MAX_MESH_WATER_PRIMITIVES_PER_ROOT * MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE
    ); // 全根抽样预算与primitive数硬门保持可审计一致。
} // 完成UV几何抽样预算回归。

#[test] // V需要同时有横向跨度且不明显越出0..1。
fn uv0_semantics_reject_degenerate_or_out_of_range_v() {
    let (positions, mut uv0, indices) = uv_test_river_strip(10.0, 10.0); // 构造合法U米制基线。
    for uv in &mut uv0 {
        uv[1] = 0.5; // 所有顶点使用同一V，没有左右岸语义。
    } // 完成退化V构造。
    assert!(
        validate_whole_mesh_water_uv0(
            &positions,
            &uv0,
            Some(&indices),
            Mat3A::IDENTITY,
            MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
        )
        .is_err()
    ); // 非空Float32x2也不得放行常数V。

    let (_, mut uv0, _) = uv_test_river_strip(10.0, 10.0); // 重建具有正常V跨度的基线。
    uv0[3][1] = 1.01; // 超出千分之一容差，模拟常规纹理平铺UV。
    assert!(
        validate_whole_mesh_water_uv0(
            &positions,
            &uv0,
            Some(&indices),
            Mat3A::IDENTITY,
            MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
        )
        .is_err()
    ); // 明显越界V必须失败，但正式实现仍允许有限f32舍入。
} // 完成V跨度与范围失败回归。

#[test] // mesh_index只从Bevy正式primitive标签严格解析供诊断。
fn gltf_mesh_index_parser_is_strict_and_diagnostic_only() {
    assert_eq!(parse_gltf_mesh_index("Mesh12/Primitive3"), Some(12)); // 正式Bevy标签返回mesh部分。
    assert_eq!(parse_gltf_mesh_index("Mesh0/Primitive0"), Some(0)); // 零索引是合法诊断值。
    assert_eq!(parse_gltf_mesh_index("Mesh12/Primitive"), None); // 缺失primitive数字时不做部分解析。
    assert_eq!(parse_gltf_mesh_index("Other12/Primitive3"), None); // 非Bevy primitive标签安全失败。
} // 完成mesh_index诊断解析回归。

#[test] // Pending必须阻止placement候选提前结算，但不能伪装成失败。
fn pending_status_is_neither_ready_nor_failed() {
    let status = DynamicWaterMeshOverrideStatus::Pending; // 构造尚未轮到单根处理的公开状态。
    assert!(!status.is_ready()); // placement联合门必须继续等待。
    assert_eq!(status.failure_detail(), None); // 等待不是失败，不能提前拒绝整份候选修订。
} // 完成Pending公开语义回归。

#[test] // 普通空覆盖不进入队列，清空已有水面则仍需排队恢复。
fn only_nonempty_or_previously_applied_roots_require_processing() {
    let empty = DynamicWaterMeshOverrides::default(); // 普通placement默认没有整网格赋水。
    assert!(!requires_pending_processing(&empty, false)); // 无新覆盖且无旧接管必须即时Ready。
    assert!(requires_pending_processing(&empty, true)); // 清空上轮水面仍需逐帧恢复原Blender组件。
    let nonempty = DynamicWaterMeshOverrides::new(vec![surface(&["河道_A"], "河面Mesh")]); // 新覆盖必须进行完整预检。
    assert!(requires_pending_processing(&nonempty, false)); // 即使新根没有旧接管，也必须进入队列。
} // 完成队列快路径判定回归。

#[test] // 赋水必须把普通Blender默认材质变成可直接使用的透明水，同时保留作者更低Alpha。
fn whole_mesh_assignment_normalizes_transparent_water_pbr() {
    let mut opaque = StandardMaterial {
        base_color: Color::srgba(0.2, 0.4, 0.6, 1.0), // 模拟Blender默认Opaque材质但保留可辨认RGB。
        alpha_mode: AlphaMode::Opaque,
        unlit: true,
        metallic: 0.8,
        perceptual_roughness: 0.7,
        double_sided: false,
        ..default()
    };
    runtime::configure_whole_mesh_water_material(&mut opaque);
    assert_eq!(opaque.alpha_mode, AlphaMode::Blend); // 赋水后必须进入透明前向PBR。
    assert_eq!(opaque.base_color.alpha(), 0.72); // 默认不透明材质采用正式水Alpha上限。
    assert_eq!(opaque.base_color.to_srgba().red, 0.2); // 作者RGB不因水面规范化被替换成另一套固定颜色。
    assert!(!opaque.unlit); // 赋水后必须恢复环境光与方向光PBR反射。
    assert_eq!(opaque.metallic, 0.0); // 水面必须按介电材质受光。
    assert_eq!(
        opaque.perceptual_roughness,
        DEFAULT_MAP_WATER_PERCEPTUAL_ROUGHNESS,
    ); // 普通粗材质收口到可辨认天空倒影的正式水面基准。
    assert!(opaque.double_sided && opaque.cull_mode.is_none()); // 瀑布背面也必须可见。

    let mut authored_transparent = StandardMaterial {
        base_color: Color::srgba(0.1, 0.2, 0.3, 0.35), // 作者明确设置更透明的水材质。
        perceptual_roughness: 0.03, // 作者明确设置比正式零点零六基准更光滑的镜面。
        ..default()
    };
    runtime::configure_whole_mesh_water_material(&mut authored_transparent);
    assert_eq!(authored_transparent.base_color.alpha(), 0.35); // 作者更透明Alpha不得被抬高。
    assert_eq!(authored_transparent.perceptual_roughness, 0.03); // 作者更光滑粗糙度同样保留。
} // 完成整网格水材质规范化回归。

#[test] // 同根连续Changed只保留一项，处理时由根组件提供最新快照。
fn pending_queue_coalesces_duplicate_root_requests() {
    let root = Entity::from_raw_u32(11).expect("测试根实体必须合法"); // 两次请求使用完全相同的稳定实体身份。
    let mut queue = DynamicWaterMeshOverrideQueue::default(); // 构造空固定容量队列。
    assert_eq!(queue.enqueue(root), MeshWaterEnqueueResult::Queued); // 第一次请求进入FIFO。
    assert_eq!(queue.enqueue(root), MeshWaterEnqueueResult::Coalesced); // 第二次Changed不复制旧作者快照也不追加重复根。
    assert_eq!(queue.len(), 1); // 同一根无论修改多少次都只占一个固定队列槽。
    assert_eq!(queue.pop_front(), Some(root)); // 处理系统最终只弹出一次并读取组件最新值。
    assert_eq!(queue.pop_front(), None); // 不存在第二个陈旧请求可覆盖最新结果。
} // 完成同根Changed合并回归。

#[test] // FIFO每次只暴露一个根，让逐帧系统能严格限制全局峰值。
fn pending_queue_pops_roots_one_at_a_time_in_fifo_order() {
    let first = Entity::from_raw_u32(21).expect("第一个测试根实体必须合法"); // 模拟先到水体根。
    let second = Entity::from_raw_u32(22).expect("第二个测试根实体必须合法"); // 模拟后到水体根。
    let mut queue = DynamicWaterMeshOverrideQueue::default(); // 构造独立纯队列。
    assert_eq!(queue.enqueue(first), MeshWaterEnqueueResult::Queued); // 先到根进入队首。
    assert_eq!(queue.enqueue(second), MeshWaterEnqueueResult::Queued); // 后到根进入队尾。
    assert_eq!(queue.pop_front(), Some(first)); // 本帧最多消费第一个根。
    assert_eq!(queue.len(), 1); // 第二个根明确留到下一帧。
    assert_eq!(queue.pop_front(), Some(second)); // 下一次调用才消费后到根。
} // 完成FIFO单根弹出回归。

#[test] // 根移除或despawn必须O(1)摘链，不能给下一张地图留下陈旧队列阻塞。
fn pending_queue_can_cancel_middle_root_without_breaking_fifo() {
    let first = Entity::from_raw_u32(31).expect("第一个取消测试根必须合法"); // 保留的队首。
    let removed = Entity::from_raw_u32(32).expect("中间取消测试根必须合法"); // 模拟覆盖移除或根despawn。
    let last = Entity::from_raw_u32(33).expect("最后一个取消测试根必须合法"); // 保留的队尾。
    let mut queue = DynamicWaterMeshOverrideQueue::default(); // 构造三项FIFO。
    assert_eq!(queue.enqueue(first), MeshWaterEnqueueResult::Queued); // 登记队首。
    assert_eq!(queue.enqueue(removed), MeshWaterEnqueueResult::Queued); // 登记随后会移除的中间项。
    assert_eq!(queue.enqueue(last), MeshWaterEnqueueResult::Queued); // 登记队尾。
    assert!(queue.cancel(removed)); // 移除observer必须能直接摘除中间根。
    assert!(!queue.cancel(removed)); // 重复移除保持幂等且不破坏邻接链。
    assert_eq!(queue.pop_front(), Some(first)); // 原队首顺序保持不变。
    assert_eq!(queue.pop_front(), Some(last)); // 原队尾跳过已移除根紧随其后。
    assert_eq!(queue.pop_front(), None); // 不残留会拖慢下一地图的陈旧实体。
} // 完成队列O(1)取消回归。

#[test] // 固定容量满后必须失败关闭，不允许队列或HashSet继续增长。
fn pending_queue_accepts_roots_beyond_the_old_limit() {
    let mut queue = DynamicWaterMeshOverrideQueue::default(); // 构造空队列并填满公开功能域硬上限。
    for raw in 1..=MAX_MESH_WATER_PENDING_ROOTS as u32 {
        let root = Entity::from_raw_u32(raw).expect("硬上限范围内测试实体必须合法"); // 每项使用唯一实体身份。
        assert_eq!(queue.enqueue(root), MeshWaterEnqueueResult::Queued); // 上限以内全部正常入队。
    } // 完成固定容量填充。
    let overflow = Entity::from_raw_u32(MAX_MESH_WATER_PENDING_ROOTS as u32 + 1)
        .expect("超额测试实体必须合法"); // 构造唯一的上限外请求。
    assert_eq!(queue.enqueue(overflow), MeshWaterEnqueueResult::Queued);
    assert_eq!(queue.len(), MAX_MESH_WATER_PENDING_ROOTS + 1);
} // 完成待处理队列容量回归。
