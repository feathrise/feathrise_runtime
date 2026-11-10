//! 绘制水面 R8/R16 联合核心、有限岸带和自由波输入回归。

use std::sync::Arc; // 测试按正式接口提供不可变R16快照。

use bevy::prelude::{Vec2, Vec3}; // 查询可见水面足迹并模拟玩家脚底三维位置。

use super::*; // 直接检查后台连续网格数据，测试不启动Bevy App或RenderApp。
use crate::shared::large_scene::water::components::{DynamicWaterSurface, WaterJumpCycle}; // 端到端回归直接使用正式接触状态组件。
use crate::shared::large_scene::water::constants::{
    PLAYER_RIPPLE_CAPACITY, PLAYER_RIPPLE_JUMP_AMPLITUDE,
}; // 锁定固定波源容量和浅滩起跳强度。
use crate::shared::large_scene::water::footprint::painted_water_footprint_contains; // 验证互动足迹与实际可见网格一致。
use crate::shared::large_scene::water::ripple::update_player_ripples as update_player_ripples_with_sequence; // 通过正式入水与跳跃状态机验证浅滩行为。

const BASE_HEIGHT: f32 = -10.0; // 测试R16零点。
const HEIGHT_SCALE: f32 = 20.0; // 测试R16完整米制跨度。

#[test]
fn independent_water_keeps_r8_footprint_and_free_waves_without_any_terrain() {
    for water_level in [-120., 0., 83.] {
        let mesh = PaintedWaterMeshData::from_independent_mask(
            &[255; 16], [4, 4], [-128., -64.], [128., 64.], water_level,
        ).unwrap();
        assert!(!mesh.is_empty(), "独立静水不能因没有河床或水位为零/负数而干涸");
        assert!(mesh.positions.iter().all(|position| position[1] == 0.));
        assert!(mesh.uvs.iter().all(|uv| uv[1] == 0.5), "沿用原动态材质的完整自由波输入");
        assert!(painted_water_footprint_contains(&mesh.footprint, Vec2::ZERO));
        assert!(!painted_water_footprint_contains(&mesh.footprint, Vec2::new(129., 0.)));
        let aabb = mesh.local_aabb.unwrap();
        assert_eq!(Vec3::from(aabb.half_extents), Vec3::new(128., 0., 64.));
    }
    assert!(PaintedWaterMeshData::from_independent_mask(&[0; 16], [4, 4], [-1.; 2], [1.; 2], 0.).unwrap().is_empty());
    assert!(PaintedWaterMeshData::from_independent_mask(&[255; 15], [4, 4], [-1.; 2], [1.; 2], 0.).is_err());
    assert!(PaintedWaterMeshData::from_independent_mask(&[255; 16], [4, 4], [1.; 2], [-1.; 2], 0.).is_err());
    assert!(PaintedWaterMeshData::from_independent_mask(&[255; 16], [4, 4], [-f32::MAX, 0.], [f32::MAX, 1.], 0.).is_err());
}

#[test] // 生产构建器与雨花查询必须引用完全相同的三角剖分。
fn splash_positions_follow_generated_render_triangles() {
    // 不使用手工网格假定生成器拓扑。
    let terrain = terrain_input([2, 2], vec![encode_height(-2.0); 4]); // 整块河床低于静水，形成完整水面核心。
    let mesh = build_painted_water_mesh(
        &[255; 256],
        [16, 16],
        [0.0, 0.0],
        [100.0, 100.0],
        &terrain,
        0.0,
    )
    .unwrap(); // 直接运行正式后台纯构建函数。
    let deform = |p: Vec3, alpha: f32| {
        p + Vec3::new(p.z.sin() * alpha, p.x.sin() * alpha, p.x.cos() * alpha)
    }; // 非线性变形区分顶点插值与错误的解析点采样。
    let mut checked = 0; // 确保至少检查一个真正可发射的片面。
    for triangle in mesh.indices.chunks_exact(3) {
        // 仅测试遍历实际渲染索引，运行时仍固定三点。
        let vertices: [Vec3; 3] = std::array::from_fn(|corner| {
            Vec3::from_array(mesh.positions[triangle[corner] as usize])
        }); // 读取生成器真正发布的位置。
        let point = (vertices[0] + vertices[1] + vertices[2]) / 3.0; // 使用实际片面的内部重心。
        let Some(sample) =
            mesh.footprint
                .sample_displaced_position(Vec2::new(point.x, point.z), 0.0, deform)
        else {
            continue;
        }; // 透明岸带本就不允许离水花。
        let expected = triangle
            .iter()
            .zip(vertices)
            .map(|(index, p)| deform(p, mesh.colors[*index as usize][3]))
            .sum::<Vec3>()
            / 3.0; // 直接从真实Mesh三角构造变形后重心作为独立证据。
        assert!((sample - expected).length() < 0.0001); // 同时验证位置、Alpha与两种对角半片的一致性。
        checked += 1; // 记录真实水面核心覆盖数量。
    } // 完成实际生成三角审计。
    assert!(checked > 2); // 空水面或全被过滤不能让测试空通过。
} // 结束生产几何链回归。

#[test] // 后台范围缓存必须覆盖全部三维顶点且不为零厚度轴伪造尺寸。
fn local_aabb_is_derived_once_from_completed_vertices() {
    let aabb = local_aabb_from_positions(&[
        [-4.0, 2.0, 8.0],   // 最小X与最大Z来自不同轴组合。
        [6.0, -2.0, -10.0], // 最大X、最小Y和最小Z来自第二点。
        [2.0, 4.0, 1.0],    // 最大Y来自第三点，锁定逐轴累计语义。
    ])
    .expect("非空顶点必须产生局部包围盒");
    assert_eq!(Vec3::from(aabb.center), Vec3::new(1.0, 1.0, -1.0)); // 中心取每轴极值中点。
    assert_eq!(Vec3::from(aabb.half_extents), Vec3::new(5.0, 3.0, 9.0)); // 半尺寸覆盖完整极值跨度。
    assert!(local_aabb_from_positions(&[]).is_none()); // 空水层继续明确不可定位。
}

/// 把测试世界高度编码为与正式资产相同的线性R16。
fn encode_height(height: f32) -> u16 {
    ((height - BASE_HEIGHT) / HEIGHT_SCALE * u16::MAX as f32)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16
}

/// 建立覆盖零至一百米方形场景的受检地形输入。
fn terrain_input(resolution: [u32; 2], heights: Vec<u16>) -> PaintedWaterTerrainInput {
    PaintedWaterTerrainInput::new(
        Arc::<[u16]>::from(heights), // 测试与正式运行时共享同一种Arc切片接口。
        resolution,                  // R16顶点分辨率。
        [0.0, 0.0],                  // 测试场景世界最小X/Z。
        [100.0, 100.0],              // 测试场景世界最大X/Z。
        BASE_HEIGHT,                 // R16零值对应高度。
        HEIGHT_SCALE,                // R16完整米制跨度。
    )
    .expect("测试地形输入必须有效")
}

/// 使用水面最近已消费的序号推进一帧，表示本帧没有新的Tnua实际起跳。
fn update_player_ripples(
    water_surface: &mut DynamicWaterSurface, // 接收绘制水测试中的完整运行时状态。
    player_local_foot_position: Option<Vec3>, // 传入当前玩家脚底局部位置或缺失状态。
    delta_seconds: f32,                      // 推进统一波源寿命与接触状态的帧长。
) -> Option<crate::shared::large_scene::water::types::PlayerWaterRipple> {
    let takeoff_sequence = water_surface.last_player_takeoff_sequence.unwrap_or(0); // 没有新ActionStarted时保持最近序号不变。
    update_player_ripples_with_sequence(
        water_surface,              // 调用正式纯逻辑更新同一测试状态。
        player_local_foot_position, // 传入当前可选脚底位置。
        Some(takeoff_sequence),     // 相同序号不能重复生成起跳波。
        delta_seconds,              // 使用调用方给出的真实帧长。
    ) // 返回本帧最后新增的统一波源。
}

/// 发布一个新的Tnua实际起跳序号并推进绘制水接触状态。
fn update_player_ripples_with_takeoff(
    water_surface: &mut DynamicWaterSurface, // 接收即将消费实际起跳身份的绘制水状态。
    player_local_foot_position: Option<Vec3>, // Tnua固定步完成后的玩家脚底局部位置。
    takeoff_sequence: u64,                   // 不同于上次已消费值的新ActionStarted序号。
    delta_seconds: f32,                      // 推进统一长期波的帧长。
) -> Option<crate::shared::large_scene::water::types::PlayerWaterRipple> {
    update_player_ripples_with_sequence(
        water_surface,              // 调用正式纯逻辑更新同一测试状态。
        player_local_foot_position, // 传递当前脚底位置。
        Some(takeoff_sequence),     // 发布不会被短按时序漏掉的权威起跳身份。
        delta_seconds,              // 使用调用方给出的真实帧长。
    ) // 返回本帧新增的完整起跳大波。
}

/// C类回归使用17²粗地形：整图平地，仅中央四乘四高度单元形成一米深河槽。
fn coarse_local_channel_terrain() -> PaintedWaterTerrainInput {
    let resolution = [17, 17]; // 一百米场景每格六点二五米，能锁定动态岸带宽度。
    let mut heights = vec![encode_height(0.0); 17 * 17]; // 远处平地与零水位共面。
    for z in 6..=10 {
        for x in 6..=10 {
            heights[z * 17 + x] = encode_height(-1.0); // 四角都低于水位的单元才成为稳定核心。
        }
    }
    terrain_input(resolution, heights)
}

#[test]
fn full_mask_with_local_channel_only_builds_local_water_and_exact_footprint() {
    let mask = vec![255u8; 16 * 16]; // C类坏输入：整张R8都宣称允许水面。
    let mesh = build_painted_water_mesh(
        &mask,
        [16, 16],
        [0.0, 0.0],
        [100.0, 100.0],
        &coarse_local_channel_terrain(),
        0.0,
    )
    .expect("局部具备稳定河床时应只构建局部水面");
    assert!(!mesh.is_empty()); // 中央真实深槽仍应正常显示水面。
    let used_positions = mesh
        .indices
        .iter()
        .map(|index| mesh.positions[*index as usize])
        .collect::<Vec<_>>(); // 只审计实际被三角索引引用的顶点，忽略透明保护边。
    let used_bounds = used_positions.iter().fold(
        [
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ],
        |mut bounds, position| {
            bounds[0] = bounds[0].min(position[0]);
            bounds[1] = bounds[1].max(position[0]);
            bounds[2] = bounds[2].min(position[2]);
            bounds[3] = bounds[3].max(position[2]);
            bounds
        },
    );
    assert!(
        used_bounds[0] >= 20.0
            && used_bounds[1] <= 80.0
            && used_bounds[2] >= 20.0
            && used_bounds[3] <= 80.0,
        "整图R8不得把局部河槽扩成远处平地三角形，实际范围为{used_bounds:?}"
    );
    assert!(painted_water_footprint_contains(
        &mesh.footprint,
        Vec2::new(50.0, 50.0),
    )); // 中央稳定核心所在的可见水面允许玩家触水。
    assert!(!painted_water_footprint_contains(
        &mesh.footprint,
        Vec2::new(5.0, 5.0),
    )); // 实际活跃水面外的远处共面平地不属于玩法水域。
}

#[test]
fn full_mask_on_flat_ground_is_a_safe_dry_layer() {
    let terrain = terrain_input([3, 3], vec![encode_height(0.0); 9]); // 全部地面与静止水位共面。
    let mesh = build_painted_water_mesh(
        &[255; 16],
        [4, 4],
        [0.0, 0.0],
        [100.0, 100.0],
        &terrain,
        0.0,
    )
    .expect("非空R8没有稳定河床时应成为合法干涸层");
    assert!(mesh.is_empty()); // 不能退回整图透明水面，也不能让水位拖动永久标记预览失败。
}

#[test]
fn high_resolution_mask_uses_only_the_overlapped_coarse_terrain_surface() {
    let terrain = terrain_input(
        [2, 2], // 单个一百米地形格刻意远粗于R8水像素。
        vec![
            encode_height(-1.0), // 西北角已经形成一米深河床。
            encode_height(0.0),  // 东北远端平地不能否决西北角附近的小水像素。
            encode_height(0.0),  // 西南远端平地同样不在首个水像素覆盖范围内。
            encode_height(0.0),  // 东南远端平地保持零水位共面。
        ],
    );
    let mut mask = vec![0u8; 100 * 100]; // 一米R8像素模拟细水笔刷与粗R16组合。
    mask[0] = 255; // 只覆盖西北一平方米，实际三角地形在该范围内仍接近一米深。
    let mesh = build_painted_water_mesh(
        &mask,          // 使用仅一个有效像素的高分辨率遮罩。
        [100, 100],     // 一个水像素远小于一百米地形格。
        [0.0, 0.0],     // 与测试地形共享世界最小角。
        [100.0, 100.0], // 与测试地形共享世界最大角。
        &terrain,       // 使用正式NE-SW三角剖分的粗R16输入。
        0.0,            // 静止水位为零，首像素内深度远超0.20米。
    )
    .expect("细R8水像素应按实际覆盖范围采样粗地形，而不是读取整格远端高顶点");
    assert!(!mesh.is_empty()); // 修复前会误报“没有满足最低稳定水深的R16河床”。
}

#[test]
fn terrain_rectangle_maximum_includes_the_internal_triangle_diagonal() {
    let terrain = terrain_input(
        [2, 2], // 单格地形便于把正式NE-SW对角线单独构造成山脊。
        vec![
            encode_height(0.0), // 西北低点。
            encode_height(1.0), // 东北高点。
            encode_height(1.0), // 西南高点，与东北连成正式对角线山脊。
            encode_height(0.0), // 东南低点。
        ],
    );
    let maximum = terrain
        .maximum_surface_height(
            [20.0, 70.0], // 局部坐标u=0.2、v=0.7，矩形四角都避开山脊峰值。
            [40.0, 90.0], // 对角线从矩形西边和北边内部穿过，交点高度应为一米。
        )
        .expect("地形内有限矩形应返回正式三角曲面最大值");
    assert!((maximum - 1.0).abs() < 0.001); // 只采矩形四角会得到约0.9米，必须包含内部对角线交点。
}

#[test]
fn empty_mask_is_a_valid_zero_water_layer() {
    let terrain = terrain_input([3, 3], vec![encode_height(0.0); 9]); // 空层不要求预先存在河床。
    let mesh =
        build_painted_water_mesh(&[0; 16], [4, 4], [0.0, 0.0], [100.0, 100.0], &terrain, 0.0)
            .expect("全零水层是合法作者输入");
    assert!(mesh.is_empty()); // 不创建顶点、索引、实体或GPU材质。
}

#[test]
fn painted_deep_water_keeps_free_wave_uv_and_zero_rest_y() {
    let terrain = terrain_input([3, 3], vec![encode_height(-1.0); 9]); // 整图都满足稳定最低水深。
    let mesh = build_painted_water_mesh(
        &[255; 16],
        [4, 4],
        [0.0, 0.0],
        [100.0, 100.0],
        &terrain,
        0.0,
    )
    .expect("完整深水河床应正常构建");
    assert!(mesh.positions.iter().all(|position| position[1] == 0.0)); // 水位仍只由实体Transform提供。
    assert!(mesh.uvs.iter().all(|uv| uv[1] == 0.5)); // 绘制水全部顶点都获得完整背景波与互动波振幅。
}

#[test]
fn coarse_terrain_shore_band_spans_two_height_cells() {
    let mask = vec![255u8; 16 * 16]; // 整图R8让测试只观察R16核心与派生岸带。
    let mesh = build_painted_water_mesh(
        &mask,
        [16, 16],
        [0.0, 0.0],
        [100.0, 100.0],
        &coarse_local_channel_terrain(),
        0.0,
    )
    .expect("粗地形局部河槽应构建完整作者岸坡宽度");
    let max_core_x = 62.5; // 深槽核心最东侧高度顶点是第十格。
    let farthest_used_x = mesh
        .indices
        .iter()
        .map(|index| mesh.positions[*index as usize][0])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(farthest_used_x >= max_core_x + 12.5 - 0.001); // 两个6.25米地形格内都保留视觉三角，alpha不在岸坡中途截断。
    assert!(farthest_used_x <= max_core_x + 12.5 + 0.001); // 有限岸带外绝不继续向远处平地扩张。
}

#[test]
fn painted_shallow_water_jump_emits_matching_waves_but_coplanar_land_never_does() {
    let row = [-1.0, -1.0, -0.05, -0.05, 0.0, 0.0, 0.0]; // 一行同时包含深水核心、五厘米浅滩和齐平陆地。
    let heights = (0..3)
        .flat_map(|_| row.into_iter().map(encode_height))
        .collect::<Vec<_>>(); // 三行相同高度形成六乘二个规则地形单元。
    let terrain = PaintedWaterTerrainInput::new(
        Arc::<[u16]>::from(heights), // 端到端测试仍通过正式共享高度切片入口。
        [7, 3],                      // 六乘二个一米地形单元。
        [0.0, 0.0],                  // 测试场景世界最小X/Z。
        [6.0, 2.0],                  // 一米格距让两米视觉岸带范围可精确断言。
        BASE_HEIGHT,                 // 使用测试统一R16零点。
        HEIGHT_SCALE,                // 使用测试统一R16米制跨度。
    )
    .expect("浅滩测试地形输入必须有效");
    let mesh = build_painted_water_mesh(
        &[255; 12], // 全覆盖R8让测试只观察深水核心派生的真实可见浅滩足迹。
        [6, 2],     // 水面源单元与测试高度单元一一对应。
        [0.0, 0.0], // 使用地形输入西北角开始的局部测试范围。
        [6.0, 2.0], // 水面与一米格距测试地形严格覆盖相同范围。
        &terrain,   // 使用同一不可变R16测试快照。
        0.0,        // 静止水位为零，五厘米浅滩脚底位于负五厘米。
    )
    .expect("深水核心外的可见浅滩应正常构建");
    assert!(painted_water_footprint_contains(
        &mesh.footprint,
        Vec2::new(2.5, 0.5),
    )); // 第二个浅水高度区间位于实际可见岸带中，不能再被稳定核心门拒绝。
    assert!(!painted_water_footprint_contains(
        &mesh.footprint,
        Vec2::new(5.5, 0.5),
    )); // 活跃水面网格之外即使R8全覆盖也不能授权玩家水波。

    let mut water_surface = DynamicWaterSurface {
        interaction_footprint: WaterInteractionFootprint::Painted(mesh.footprint), // 移交真实后台构建足迹，避免手工点亮掩盖回归。
        rest_water_level: 0.0,   // 绘制水实体局部静止水位固定为零。
        flow_direction: Vec2::X, // 纯互动测试保持静水默认流向。
        flow_speed: 1.0,         // 纯互动测试保持静水默认流速。
        player_ripples: Vec::with_capacity(PLAYER_RIPPLE_CAPACITY), // 与正式实体使用相同固定池。
        last_player_takeoff_sequence: None, // 首帧只建立Tnua实际起跳序号基线。
        previous_player_local_xz: None, // 首帧尚未建立水中移动基准。
        previous_player_local_y: None, // 首帧尚未建立起跳速度基准。
        distance_since_ripple: 0.0, // 首帧没有累计水中步距。
        player_was_in_water: false, // 首次浅水接触应生成一圈入水波。
        jump_cycle: WaterJumpCycle::Idle, // 首次接触尚未开始等待落水的跳跃周期。
    };
    let frame_seconds = 1.0 / 60.0; // 使用固定六十帧步长构造可重复竖直速度。
    let shallow_position = Vec3::new(2.5, -0.05, 0.5); // 脚底低于静止水位五厘米且水平位于可见浅滩。
    assert!(
        update_player_ripples(&mut water_surface, Some(shallow_position), frame_seconds).is_some()
    ); // 可见浅滩必须首先进入正式水接触状态。
    assert!(
        update_player_ripples(&mut water_surface, Some(shallow_position), frame_seconds).is_none()
    ); // 原地站定不能持续追加水波。
    let ripple_count_before_takeoff = water_surface.player_ripples.len(); // 锁定实际起跳前仍在传播的普通入水圈。
    let shallow_takeoff = update_player_ripples_with_takeoff(
        &mut water_surface,
        Some(shallow_position + Vec3::Y * 0.03), // 三厘米单帧上升超过起跳速度且脚底仍有两厘米水深。
        1,                                       // 模拟Tnua在固定步真正接受本次短按跳跃。
        frame_seconds,
    );
    assert_eq!(
        shallow_takeoff.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 绘制浅滩的起跳必须立即得到与落水相同的完整九厘米大波。
    assert_eq!(
        water_surface.player_ripples.len(),
        ripple_count_before_takeoff + 1
    ); // 起跳大波必须进入统一八槽，并与旧入水圈同时传播。
    let ripple_count_before_landing = water_surface.player_ripples.len(); // 锁定起跳前仍在传播的旧波数量。
    assert!(
        update_player_ripples(
            &mut water_surface,
            Some(shallow_position + Vec3::Y * 0.08), // 脚底穿出静止水位三厘米，进入同一浅滩上空。
            frame_seconds,
        )
        .is_none()
    ); // 离水帧继续播放既有起跳大波，但不向统一池重复追加第二圈。
    let shallow_landing = update_player_ripples(
        &mut water_surface,
        Some(shallow_position), // 回到五厘米浅水时结算本周期唯一冲击。
        frame_seconds,
    );
    assert_eq!(
        shallow_landing.map(|ripple| ripple.amplitude),
        Some(PLAYER_RIPPLE_JUMP_AMPLITUDE)
    ); // 可见浅滩必须在真实落水边沿追加与深水相同的一圈强波。
    assert_eq!(
        water_surface.player_ripples.len(),
        ripple_count_before_landing + 1
    ); // 旧入水圈和新落水圈同时保留，不能重置波源池。
    let ripple_count_after_landing = water_surface.player_ripples.len(); // 锁定完整跳跃结算后的固定池数量。

    let _reset = update_player_ripples(&mut water_surface, None, frame_seconds); // 模拟离开当前接触并清除跳跃历史。
    let coplanar_position = Vec3::new(2.5, 0.0, 0.5); // 使用同一可见水平足迹，只把脚底放到与静止水位齐平。
    assert!(
        update_player_ripples(&mut water_surface, Some(coplanar_position), frame_seconds,)
            .is_none()
    ); // 齐平陆地不能因水平处于岸带而进入水接触。
    assert!(
        update_player_ripples_with_takeoff(
            &mut water_surface,
            Some(coplanar_position + Vec3::Y * 0.03),
            2, // 即使Tnua真正接受陆地起跳，水深门控也必须拒绝波源。
            frame_seconds,
        )
        .is_none()
    ); // 齐平陆地上的跳跃同样不能产生水波。
    assert_eq!(
        water_surface.player_ripples.len(),
        ripple_count_after_landing
    ); // 陆地两帧不得污染既有波源池。
}
