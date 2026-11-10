//! GPU叶片植被分页报告的纯逻辑回归测试。
//! 测试覆盖旧样本截断、正式窗口分母、三类叶片身份和按 profile 几何预算，不启动窗口或 RenderApp。

use super::*; // 复用父模块私有格式器、分页筛选和公开诊断值类型。
use feathrise_environment::vegetation::blade::{
    GpuBladeConfig, GpuBladeUltraLodBudget, GpuBladeVegetationTimingStats, GpuBladeViewLodConfig,
}; // 通过公开配置构造真实内容和八档局部LOD窗口。

const TEST_WHEAT_CONFIG: GpuBladeConfig =
    GpuBladeConfig::new(GpuBladeProfileSlot::SLOT_1, 1.35, 0.72, 1.18, 0.04); // 测试侧只锁定一号槽和既有几何预算。
const TEST_MIXED_GRAIN_CONFIG: GpuBladeConfig =
    GpuBladeConfig::new(GpuBladeProfileSlot::SLOT_3, 1.18, 0.74, 0.98, 0.04); // 三号槽用于验证分页隔离，不向公共库写入草种预设。

#[test] // 无当前内容窗口时必须丢弃跨场景残留GPU样本。
fn no_grass_windows_discard_stale_gpu_diagnostics() {
    let stale_gpu = GpuBladeGpuReadbackStats {
        sample_sequence: 42,    // 模拟上一场景已完成回读。
        rendered_instances: 99, // 模拟上一场景实绘残留。
        ..Default::default()    // 其余字段保持零。
    }; // 完成旧实例样本。
    let stale_timing = GpuBladeGpuTimingStats {
        sample_sequence: 24,  // 模拟上一场景已完成查询。
        draw_total_ms: 3.5,   // 模拟上一场景绘制耗时。
        ..Default::default()  // 其余字段保持零。
    }; // 完成旧耗时样本。
    let (gpu, timing) = current_gpu_diagnostics(
        false,                            // 当前分页没有对应内容窗口。
        GpuBladeStreamingPhase::Inactive, // 当前流送会话未启用。
        Some(&stale_gpu),                 // 提供故意残留样本。
        Some(&stale_timing),              // 提供故意残留耗时。
    ); // 执行生产样本归属判断。
    assert_eq!(gpu, GpuBladeGpuReadbackStats::default()); // 实例样本必须清零。
    assert_eq!(timing, GpuBladeGpuTimingStats::default()); // 耗时样本必须清零。
} // 完成离图样本截断验证。

#[test] // 当前内容窗口存在时保留最新异步GPU样本。
fn active_grass_windows_keep_latest_gpu_diagnostics() {
    let gpu = GpuBladeGpuReadbackStats {
        sample_sequence: 7,   // 模拟当前会话实例样本。
        ..Default::default()  // 其余字段保持零。
    }; // 完成实例样本。
    let timing = GpuBladeGpuTimingStats {
        sample_sequence: 8,   // 模拟当前会话计时样本。
        ..Default::default()  // 其余字段保持零。
    }; // 完成计时样本。
    assert_eq!(
        current_gpu_diagnostics(
            true,                             // 当前分页存在窗口。
            GpuBladeStreamingPhase::Inactive, // 独立样例允许无正式流送阶段。
            Some(&gpu),                       // 提供当前实例样本。
            Some(&timing),                    // 提供当前计时样本。
        ),
        (gpu, timing),
    ); // 当前样本必须原样保留。
} // 完成活跃内容样本保留验证。

#[test] // 重进场景首载等待阶段必须拒绝上一代异步样本。
fn reentered_scene_waiting_phase_rejects_previous_session_samples() {
    let stale_gpu = GpuBladeGpuReadbackStats {
        sample_sequence: 17,     // 模拟上一代实例样本。
        rendered_instances: 123, // 模拟上一代实绘量。
        ..Default::default()     // 其余字段保持零。
    }; // 完成旧实例样本。
    let stale_timing = GpuBladeGpuTimingStats {
        sample_sequence: 18,   // 模拟上一代查询样本。
        compute_total_ms: 2.0, // 模拟上一代Compute耗时。
        ..Default::default()   // 其余字段保持零。
    }; // 完成旧计时样本。
    let (gpu, timing) = current_gpu_diagnostics(
        true,                            // 新会话已经创建窗口。
        GpuBladeStreamingPhase::Waiting, // 但首份真实输出尚未完成。
        Some(&stale_gpu),                // 提供故意残留样本。
        Some(&stale_timing),             // 提供故意残留计时。
    ); // 执行生产代次保护。
    assert_eq!(gpu, GpuBladeGpuReadbackStats::default()); // 首载等待不得显示旧实绘量。
    assert_eq!(timing, GpuBladeGpuTimingStats::default()); // 首载等待不得显示旧GPU耗时。
} // 完成重进场景代次保护验证。

#[test] // 正式验收摘要必须使用当前分页独立窗口和性能证据。
fn stage6_report_records_current_profile_performance_evidence() {
    let streaming = GpuBladeStreamingStats {
        scene_code: "HN-01",                       // 模拟正式开封场景身份。
        content_label: "普通草+全图麦田+杂粮田", // 聚合资源保留场景完整内容名。
        expected_windows: 12,                    // 场景总计三种叶片内容各四窗。
        resident_assets: 14,                     // 模拟当前会话只含三类植被的全部共享常驻资产引用。
        expected_resident_assets: 14,            // 锁定三类内容稳定共享资产预算，不混入地表。
        resident_input_bytes: 36 * 1_048_576,    // 使用整 MiB 三类数值输入。
        smoothed_speed: 8.0,                     // 模拟当前玩家奔跑速度。
        peak_speed: 12.0,                        // 模拟本会话最坏移动速度。
        ..Default::default()                     // 其余流送字段保持零。
    }; // 完成阶段6场景遥测。
    let gpu = GpuBladeVegetationReadbackStats {
        sampled_windows: 3,       // 故意模拟普通草仍缺少超远窗，显示3/4。
        rendered_instances: 1234, // 模拟当前profile真实实绘量。
        compact_capacity: 65_536, // 模拟当前profile一MiB紧凑实例缓冲。
        ..Default::default()      // 其余回读字段保持零。
    }; // 完成当前profile实例与容量样本。
    let profile_timing = GpuBladeVegetationTimingStats {
        compute_total_ms: 0.25, // 模拟普通草Compute耗时。
        draw_total_ms: 0.75,    // 模拟普通草Draw耗时。
        ..Default::default()    // 其余分项保持零。
    }; // 完成普通草独立GPU耗时样本。
    let timing = GpuBladeGpuTimingStats {
        sample_sequence: 1,        // 标记首份时间戳样本已经完成。
        timestamp_supported: true, // 模拟支持通道内时间戳的正式后端。
        ..Default::default()       // 聚合数值不参与当前profile格式化。
    }; // 完成GPU查询能力样本。
    let report = format_stage6_acceptance_report(
        "普通草",   // 当前测试只验收普通草分页。
        &streaming, // 提供正式场景与共享资产。
        &Stage6AcceptanceInputs {
            actual_windows: 3,               // 当前主世界实际普通草窗口数。
            expected_windows: 4,             // 普通草固定四窗规格。
            profile_stats: &gpu,             // 提供普通草独立实绘与容量。
            profile_timing: &profile_timing, // 提供普通草独立GPU耗时。
            gpu_timing: &timing,             // 提供GPU查询能力。
            window_buffer_bytes: gpu.dedicated_window_buffer_bytes(), // 精确窗口缓冲。
            frame_ms: 11.0,                  // 模拟同口径CPU帧耗。
        },
    ); // 使用生产helper生成可截图验收摘要。
    assert!(report.contains("普通草正式接入 HN-01 | 窗口 3/4 | 共享资产 14/14")); // 单页分母保持四窗，共享资产反映完整十二窗植被会话。
    assert!(report.contains("普通草验收 实绘 1234 | CPU帧 11.00ms")); // 当前profile实绘和CPU帧耗必须同屏。
    assert!(report.contains("GPU计算/绘制 0.250/0.750ms")); // 当前profile两个GPU阶段必须使用真实时间戳。
    assert!(report.contains("速度 当前/峰 8.0/12.0m/s")); // 固定路线保留当前与最坏速度。
    assert!(report.contains("已知下界")); // 显存必须明确标记可证明下界而非进程总量。
} // 完成当前profile正式验收摘要验证。

#[test] // 正式分母与三类叶片密度增益格式必须保持确定。
fn f3_keeps_formal_draw_denominator_and_reports_three_content_gains() {
    let ordinary = GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 1).with_density_gain(10.2); // 模拟HN-01普通草正式增益。
    let wheat = GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 2)
        .with_density_gain(1.18)
        .with_vegetation_config(TEST_WHEAT_CONFIG); // 模拟麦田独立增益。
    let mixed_grain = GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 3)
        .with_density_gain(1.05)
        .with_vegetation_config(TEST_MIXED_GRAIN_CONFIG); // 模拟杂粮田独立增益。
    let ordinary_gain =
        format_density_gain_report("普通草", &[GpuBladeProfileSlot::SLOT_0], &[ordinary]); // 普通草分页只读取零号profile窗口。
    let wheat_gain = format_density_gain_report("麦田", &[GpuBladeProfileSlot::SLOT_1], &[wheat]); // 麦田分页只读取一号profile窗口。
    let mixed_grain_gain =
        format_density_gain_report("杂粮田", &[GpuBladeProfileSlot::SLOT_3], &[mixed_grain]); // 杂粮田分页只读取三号profile窗口。
    assert_eq!(ordinary_gain, "普通草10.20"); // 普通草增益保持自身真实值。
    assert_eq!(wheat_gain, "麦田1.18"); // 麦田增益不得被普通草覆盖。
    assert_eq!(mixed_grain_gain, "杂粮田1.05"); // 杂粮增益保持独立。
    assert_eq!(expected_draw_command_count(3, 4), 32); // 普通草缺一窗仍保持四乘八桶分母。
    assert_eq!(expected_draw_command_count(3, 0), 24); // 无场景规格的独立样例按实际三窗。
} // 完成分母与增益验证。

#[test] // 麦田四个窗口必须按自身近中远超远身份显示。
fn lod_report_labels_wheat_by_profile_instead_of_extra_window() {
    let wheat_near = GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 2)
        .with_lod_level(0)
        .with_vegetation_config(TEST_WHEAT_CONFIG); // 构造麦田近景窗。
    let wheat_middle = GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 3)
        .with_lod_level(1)
        .with_vegetation_config(TEST_WHEAT_CONFIG); // 构造麦田中景窗。
    let wheat_far = GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 4)
        .with_lod_level(2)
        .with_vegetation_config(TEST_WHEAT_CONFIG); // 构造麦田远景窗。
    let wheat_ultra = GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 5)
        .with_lod_level(3)
        .with_vegetation_config(TEST_WHEAT_CONFIG); // 构造麦田超远窗。
    let report = format_lod_report(
        "麦田",
        &[
            wheat_near,   // 麦田近景按自身LOD编号命名。
            wheat_middle, // 麦田中景按自身LOD编号命名。
            wheat_far,    // 麦田远景按自身LOD编号命名。
            wheat_ultra,  // 麦田超远景按自身LOD编号命名。
        ],
    ); // 直接验证格式器不把第四项以后称为额外。
    assert!(report.contains("麦田LOD近")); // 麦田第一窗必须从近景开始。
    assert!(report.contains("麦田LOD中")); // 麦田第二窗必须显示中景。
    assert!(report.contains("麦田LOD远")); // 麦田第三窗必须显示远景。
    assert!(report.contains("麦田LOD超远")); // 麦田第四窗必须显示超远景。
    assert!(!report.contains("额外")); // 任何profile不得退回含混额外标签。
} // 完成LOD内容身份验证。

#[test] // 普通草和麦田几何必须读取各自GPU记录预算。
fn profile_geometry_report_uses_gpu_recorded_grass_and_wheat_budgets() {
    let grass = GpuBladeVegetationReadbackStats {
        sampled_windows: 1,                                   // 模拟普通草近景窗完成回读。
        rendered_instances: 10,                               // 模拟十株普通草实绘。
        rendered_instances_by_lod: [10, 0, 0, 0, 0, 0, 0, 0], // 扩展档没有实绘，仍保留八档诊断布局。
        submitted_vertices_by_lod: [170, 0, 0, 0, 0, 0, 0, 0], // 模拟每株十七顶点。
        submitted_indices_by_lod: [390, 0, 0, 0, 0, 0, 0, 0], // 模拟每株三十九索引。
        ..Default::default()                                  // 其余字段保持零。
    }; // 完成普通草几何回读。
    let wheat = GpuBladeVegetationReadbackStats {
        sampled_windows: 1,                                     // 模拟麦田近景窗完成回读。
        rendered_instances: 10,                                 // 使用相同实例量排除数量差异。
        rendered_instances_by_lod: [10, 0, 0, 0, 0, 0, 0, 0],   // 十株全部属于麦田近景。
        submitted_vertices_by_lod: [480, 0, 0, 0, 0, 0, 0, 0],  // 麦田每株四十八顶点。
        submitted_indices_by_lod: [1_200, 0, 0, 0, 0, 0, 0, 0], // 麦田每株一百二十索引。
        ..Default::default()                                    // 其余字段保持零。
    }; // 完成麦田几何回读。
    let gpu = GpuBladeGpuReadbackStats {
        by_profile: [
            grass,              // 零号profile保存普通草回读。
            wheat,              // 一号profile保存麦田回读。
            Default::default(), // 二号芦苇当前没有窗口。
            Default::default(), // 三号杂粮田当前测试没有窗口。
        ],
        ..Default::default() // 聚合字段不参与分项格式化。
    }; // 完成草麦独立profile聚合资源。
    let grass_report = format_content_gpu_report(
        &[GpuBladeProfileSlot::SLOT_0], // 请求输出普通草诊断。
        "普通草",                       // 零号profile按当前页面显示普通草。
        4,
        &gpu,                               // 提供两类真实回读。
        &GpuBladeGpuTimingStats::default(), // 性能查询保持等待。
    ); // 普通草几何断言只依赖零号GPU回读预算。
    let wheat_report = format_content_gpu_report(
        &[GpuBladeProfileSlot::SLOT_1], // 请求输出麦田诊断。
        "麦田",                         // 一号profile按当前页面显示麦田。
        4,
        &gpu,                               // 提供两类真实回读。
        &GpuBladeGpuTimingStats::default(), // 性能查询保持等待。
    ); // 麦田几何断言只依赖一号GPU回读预算。
    assert!(grass_report.contains("普通草几何 实例近/中/远/超远 10/0/0/0 | 顶点 170 [170/0/0/0]")); // 普通草必须使用自己的十七顶点预算。
    assert!(wheat_report.contains("麦田几何 实例近/中/远/超远 10/0/0/0 | 顶点 480 [480/0/0/0]")); // 麦田必须使用自己的四十八顶点预算。
    assert!(wheat_report.contains("索引 1200 [1200/0/0/0]")); // 麦田索引量同样来自自身窗口。
} // 完成草麦几何分项验证。

#[test]
fn extended_lod_report_keeps_all_geometry_and_performance_slots() {
    let stats = GpuBladeVegetationReadbackStats {
        rendered_instances: 36,
        rendered_instances_by_lod: [1, 2, 3, 4, 5, 6, 7, 8],
        submitted_vertices_by_lod: [10, 20, 30, 40, 50, 60, 70, 80],
        submitted_indices_by_lod: [20, 40, 60, 80, 100, 120, 140, 160],
        ..Default::default()
    };
    let mut gpu = GpuBladeGpuReadbackStats::default();
    gpu.by_profile[0] = stats;
    let mut timing = GpuBladeGpuTimingStats {
        sample_sequence: 1,
        timestamp_supported: true,
        pipeline_statistics_supported: true,
        ..Default::default()
    };
    timing.by_profile[0] = GpuBladeVegetationTimingStats {
        draw_lod_ms: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
        vertex_shader_invocations_by_lod: [11, 22, 33, 44, 55, 66, 77, 88],
        fragment_shader_invocations_by_lod: [100, 200, 300, 400, 500, 600, 700, 800],
        ..Default::default()
    };
    let report =
        format_content_gpu_report(&[GpuBladeProfileSlot::SLOT_0], "普通草", 8, &gpu, &timing); // 扩展档参与汇总时，分项必须完整展示同一批八档数据。
    assert!(report.contains("实例LOD0/LOD1/LOD2/LOD3/LOD4/LOD5/LOD6/LOD7 1/2/3/4/5/6/7/8"));
    assert!(report.contains("顶点 360 [10/20/30/40/50/60/70/80]"));
    assert!(report.contains("索引 720 [20/40/60/80/100/120/140/160]"));
    assert!(report.contains("0.100/0.200/0.300/0.400/0.500/0.600/0.700/0.800"));
    assert!(report.contains("11/22/33/44/55/66/77/88"));
    assert!(report.contains("片元 100/200/300/400/500/600/700/800"));
    assert!(report.contains("片元/实绘 100.0"));
}

#[test]
fn extended_lod_window_labels_budget_only_on_actual_coarsest_level() {
    let pixels = (0..7)
        .map(|level| Vec2::new(80.0 / (level + 1) as f32, 90.0 / (level + 1) as f32))
        .collect::<Vec<_>>();
    let radii = (0..8)
        .map(|level| 40.0 * (level + 1) as f32)
        .collect::<Vec<_>>();
    let config = GpuBladeViewLodConfig::from_slice(
        &pixels,
        Vec2::new(0.2, 0.6),
        0.7,
        0.1,
        &radii,
        2.0,
        &[4.0; 7],
    )
    .unwrap();
    let patches = (0..8)
        .map(|level| {
            GpuBladePatch::square(Vec3::ZERO, 100.0, 10, 1)
                .with_lod_level(level)
                .with_ultra_lod_budget(GpuBladeUltraLodBudget::High)
                .with_view_lod_config(config)
        })
        .collect::<Vec<_>>();
    let report = format_lod_report("普通草", &patches);
    for (level, line) in report.lines().enumerate() {
        assert!(line.starts_with(&format!("普通草LOD{level} ")));
        assert_eq!(line.contains("固定预算 2×2"), level == 7); // 第四档已是中间层，只有真正的第八档采用超远预算。
    }
}

#[test] // F3必须区分离图截断与存在窗口但尚未正式绑定。
fn adoption_report_distinguishes_disabled_and_direct_density_windows() {
    let status = VegetationBladeAdoptionStatus::default(); // 默认后端快照表示世界中没有显式层绑定。
    assert_eq!(format_blade_adoption_report(false, Some(&status)), "未启用"); // 当前分页没有窗口时截断全局采用资源。
    assert_eq!(
        format_blade_adoption_report(true, Some(&status)),
        "未绑定（直接密度资产） | revision 0 | 层 0 | 采用窗 0/绑定窗 0"
    ); // HN-02等仍使用直接密度资产的页面得到明确可观察状态。
    assert_eq!(
        format_blade_adoption_report(true, None),
        "状态资源未安装（直接密度资产）"
    ); // 只安装旧绘制插件的独立样例不会伪报解析布局。
} // 完成采用状态安全回退格式验证。
