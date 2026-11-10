//! GPU 草窗组件、公开输入和 ECS 注册契约回归测试。

use super::*; // 作为 components 私有子模块验证内部 uniform 字段与公开组件行为。
use crate::types::{GpuBladeConfig, GpuBladeProfileSlot, GpuBladeViewLodConfig}; // 锁定通用profile槽、内容级LOD标志及保守包围。

const COMPUTE_SHADER_SOURCE: &str = include_str!("shaders/gpu_grass_compute.wgsl"); // 直接验证随crate打包的Compute源码。
const TALL_SLOT_1_CONFIG: GpuBladeConfig =
    GpuBladeConfig::new(GpuBladeProfileSlot::SLOT_1, 1.35, 0.72, 1.18, 0.04); // 构造无草种语义的高秆二号槽测试规格。
const LOW_SLOT_3_CONFIG: GpuBladeConfig =
    GpuBladeConfig::new(GpuBladeProfileSlot::SLOT_3, 0.52, 0.95, 0.42, 0.035); // 构造无场景语义的低矮四号槽测试规格。

#[test]
fn cross_mesh_render_extraction_moves_source_coordinates_without_changing_grass_seed_or_density() {
    // 调用正式RenderApp提取合同；两个网格各保留自己的源像素、随机格和高度映射。
    for seed in [12345,98765] {
        let mut patch=GpuBladePatch::square(Vec3::new(3.,-2.,7.),64.,65,seed);
        patch.density_world=Vec4::new(-8.,-6.,8.,6.);patch.height_world=patch.density_world;
        let local=Transform::from_translation(patch.origin());
        for pose in [Transform::from_xyz(70.,3.,-20.),Transform::from_xyz(-12.,5.,90.).with_rotation(Quat::from_rotation_y(1.2))
            .with_scale(Vec3::new(2.,1.5,0.5))] {
            let global=GlobalTransform::from(pose).mul_transform(local);
            let extracted=<GpuBladePatch as ExtractComponent<bevy::render::RenderApp>>::extract_component((&patch,false,Some(&local),Some(&global))).unwrap();
            assert_eq!(extracted.grid,patch.grid);assert_eq!(extracted.density_world,patch.density_world);assert_eq!(extracted.height_world,patch.height_world);
            assert!(extracted.source_to_world.abs_diff_eq(pose.to_matrix(),0.0001));
            let source=Vec3::new(-2.,1.,3.);let world=extracted.source_to_world.transform_point3(source);
            assert!(world.abs_diff_eq(pose.to_matrix().transform_point3(source),0.0001));
            assert!(extracted.world_to_source.transform_point3(world).abs_diff_eq(source,0.0001));
        }
    }
}

#[test]
fn grass_patch_cpu_payload_keeps_legacy_offsets_and_eight_lod_extension() {
    assert_eq!(std::mem::size_of::<GpuBladePatch>(), 416); // 十八个vec4与两份场景坐标矩阵必须与WGSL一致。
    assert_eq!(GpuBladePatch::min_size().get(), 416);
    assert_eq!(
        std::mem::offset_of!(GpuBladePatch, view_lod_pixel_thresholds_ext),
        208
    );
    assert_eq!(std::mem::offset_of!(GpuBladePatch, view_lod_metadata), 272);
    assert_eq!(std::mem::offset_of!(GpuBladePatch, source_to_world), 288);
    assert_eq!(std::mem::offset_of!(GpuBladePatch, world_to_source), 352); // 新矩阵追加到旧字段之后，保持已有LOD字段偏移。
}

#[test]
fn local_lod_config_packs_all_one_to_eight_level_parameters() {
    for count in 1..=8 {
        let pixels = (0..count - 1)
            .map(|i| Vec2::new(80.0 / (i + 1) as f32, 90.0 / (i + 1) as f32))
            .collect::<Vec<_>>();
        let radii = (0..count)
            .map(|i| 40.0 * (i + 1) as f32)
            .collect::<Vec<_>>();
        let widths = (0..count - 1)
            .map(|i| 4.0 * (i + 1) as f32)
            .collect::<Vec<_>>();
        let config = GpuBladeViewLodConfig::from_slice(
            &pixels,
            Vec2::new(0.2, 0.6),
            0.7,
            0.1,
            &radii,
            2.0,
            &widths,
        )
        .unwrap();
        let view = GpuBladeView::screen_space_lod_with(config);
        let patch = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 1)
            .with_ultra_lod_budget(GpuBladeUltraLodBudget::High)
            .with_view_lod_config(config);
        assert_eq!(patch.lod_count(), count as u32);
        assert_eq!(
            patch.view_lod_metadata,
            Vec4::new(count as f32, 0.7, 0.1, 0.0)
        );
        assert_eq!(patch.view_lod_metadata, view.lod_metadata());
        assert_eq!(patch.uses_ultra_lod_chain(), count >= 4);
        for (index, expected) in pixels.iter().enumerate() {
            let actual = match index {
                0 => patch.view_lod_pixel_thresholds.zw(),
                1 => patch.view_lod_pixel_thresholds.xy(),
                2 => patch.view_lod_ultra_fade.xy(),
                _ => {
                    let packed = patch.view_lod_pixel_thresholds_ext[(index - 3) / 2];
                    if (index - 3) % 2 == 0 {
                        packed.xy()
                    } else {
                        packed.zw()
                    }
                }
            };
            assert_eq!(actual, *expected);
        }
        let packed_radii = [
            patch.view_lod_residency_radii.to_array(),
            patch.view_lod_residency_radii_ext.to_array(),
        ]
        .concat();
        assert_eq!(&packed_radii[..count], radii.as_slice());
        assert_eq!(
            patch.view_lod_residency_radii_ext,
            view.lod_residency_radii_ext()
        );
        assert_eq!(
            patch.view_lod_residency_transitions_ext,
            view.lod_residency_transitions_ext()
        );
        assert_eq!(
            patch.view_lod_pixel_thresholds_ext,
            view.lod_pixel_thresholds_ext()
        );
        for level in 0..count {
            assert_eq!(
                patch.with_lod_level(level as u32).uses_ultra_lod_budget(),
                count >= 4 && level == count - 1
            );
        }
        for bad_count in [0, 9, u32::MAX] {
            let mut invalid = config;
            invalid.lod_count = bad_count;
            assert_eq!(patch.with_view_lod_config(invalid), patch);
        }
        if count > 1 {
            let mut invalid = config;
            invalid.pixel_thresholds[count - 2] = Vec2::new(2.0, 1.0);
            assert_eq!(patch.with_view_lod_config(invalid), patch);
        }
    }
}

#[test] // 回归三个共享原型网格的LOD环带不会再次被Bevy自动合成一个绘制项。
fn grass_patch_requires_no_automatic_batching() {
    let mut world = World::new(); // 使用最小ECS世界触发require组件自动插入。
    let entity = world
        .spawn(GpuBladePatch::square(Vec3::ZERO, 64.0, 64, 1))
        .id(); // 模拟场景只插入公开草窗口组件。
    assert!(world.entity(entity).contains::<NoAutomaticBatching>()); // 每个窗口必须保留自己的动态uniform与实例范围。
} // 完成自动合批回归测试。

#[test] // F3关闭档位后必须显式移除旧渲染草窗，不能只停止下一次覆盖。
fn lod_debug_marker_removes_patch_from_extract_output() {
    let mut world = World::new(); // 使用最小主世界模拟开启与关闭的两个GPU草窗。
    world.spawn(GpuBladePatch::square(Vec3::ZERO, 64.0, 64, 1)); // 第一窗保持正常提取。
    world.spawn((
        GpuBladePatch::square(Vec3::ZERO, 64.0, 64, 2), // 第二窗使用相同合法参数。
        GpuBladeLodDebugDisabled,                       // F3关闭标记要求ExtractComponent返回None。
    )); // 完成关闭窗创建。
    let mut query = world.query::<<GpuBladePatch as ExtractComponent<bevy::render::RenderApp>>::QueryData>(); // 复用生产提取QueryData。
    let outputs = query
        .iter(&world)
        .map(<GpuBladePatch as ExtractComponent<bevy::render::RenderApp>>::extract_component)
        .collect::<Vec<_>>(); // 收集两窗的生产提取结果。
    assert_eq!(outputs.iter().filter(|output| output.is_some()).count(), 1); // 只有未关闭窗进入渲染世界。
    assert_eq!(outputs.iter().filter(|output| output.is_none()).count(), 1); // 关闭窗必须触发旧组件移除语义。
} // 完成GPU性能隔离提取契约回归。

#[test]
fn following_window_snaps_by_whole_world_cells() {
    let patch = GpuBladePatch::square(Vec3::ZERO, 716.8, 2049, 1); // 使用大型压力窗口规格。
    let first = patch.snapped_origin_xz(Vec2::new(10.0, -10.0), 64); // 尚未越过半个22.4米重定位步长。
    let second = patch.snapped_origin_xz(Vec2::new(12.0, -12.0), 64); // 越过边界后应各移动完整64个草格。
    assert_eq!(first, Vec2::ZERO); // 玩家仍在中心单元时窗口不发生连续滑动。
    assert!((second.x - 22.4).abs() < 0.001); // X 只按完整64格前进。
    assert!((second.y + 22.4).abs() < 0.001); // Z 同样按完整64格后退。
}

#[test]
fn height_field_is_explicit_and_preserves_world_bounds() {
    let patch = GpuBladePatch::square(Vec3::new(0.0, 4.37, 0.0), 716.8, 2049, 1); // 默认动态窗口仍走固定平面。
    assert!(!patch.uses_height_field()); // 没有绑定场景内容时shader不得误采fallback纹理。
    let patch = patch.with_height_field(
        Vec2::splat(-2500.0), // 大型高度纹理世界最小角。
        Vec2::splat(2500.0),  // 大型高度纹理世界最大角。
        -2.0,                 // 离线RGBA16高度编码下界。
        8.0,                  // 离线RGBA16高度编码上界。
    ); // 启用与新手村运行时相同的地表参数。
    assert!(patch.uses_height_field()); // 真实高度字段必须显式开启。
    assert_eq!(
        patch.height_world,
        Vec4::new(-2500.0, -2500.0, 2500.0, 2500.0)
    ); // 世界边界不发生轴翻转。
    assert_eq!(patch.height_decode, Vec4::new(-2.0, 10.0, 1.0, 1.0)); // shader按固定十米跨度解码RGBA16并默认完整激活。
}

#[test] // 阶段5淡入继续复用既有uniform空位，新增第八vec4只服务普通草/麦田内容包围。
fn activation_coverage_is_clamped_preserved_and_applied_by_compute() {
    let mut patch = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 1).with_activation_coverage(0.35); // 模拟地表驻留完成后处于渐入中段。
    assert_eq!(patch.activation_coverage(), 0.35); // 合法链式输入必须原样进入height_decode.w。
    patch.set_height_field(Vec2::splat(-32.0), Vec2::splat(32.0), -2.0, 8.0); // 热切换正式高度资产不应重启淡入。
    assert_eq!(patch.activation_coverage(), 0.35); // 高度范围更新必须保留流送状态机写入的覆盖率。
    patch.set_activation_coverage(2.0); // 模拟外部状态机一步越过淡入终点。
    assert_eq!(patch.activation_coverage(), 1.0); // 过大覆盖率必须钳制到完整显示。
    patch.set_activation_coverage(-1.0); // 模拟外部状态机写入负进度。
    assert_eq!(patch.activation_coverage(), 0.0); // 负覆盖率必须钳制到完全隐藏。
    patch.set_activation_coverage(f32::NAN); // 非有限输入不得污染GPU uniform。
    assert_eq!(patch.activation_coverage(), 0.0); // 非有限输入保持上一合法值。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("hash_world_cell_unit(fine_cell, ACTIVATION_SALT)"), // 首载选择键必须使用统一世界细格和独立固定盐。
        "Compute shader必须使用世界稳定整株选择推进全局激活覆盖率",
    ); // 完成Rust参数到WGSL整株淡入消费点的源码契约锁定。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("clamp(grass_patch.height_decode.w, 0.0, 1.0)"), // GPU仍直接消费草窗中的合法覆盖率。
        "Compute shader必须读取草窗激活覆盖率",
    ); // 防止阶段5状态机参数被声明但不再生效。
}

#[test] // 麦田profile必须与密度、几何LOD和层级标志共享grid.w而互不覆盖。
fn wheat_vegetation_config_preserves_existing_patch_flags() {
    let patch = GpuBladePatch::square(Vec3::ZERO, 117.6, 673, 11)
        .with_density_field(Vec2::splat(-2500.0), Vec2::splat(2500.0)) // 最低位启用麦田概率图。
        .with_lod_level(2) // 高两位几何身份选择远景麦簇。
        .with_hierarchical_lod(true) // 同源父簇位保证三档互斥。
        .with_vegetation_config(TALL_SLOT_1_CONFIG); // 两位内容profile选择通用一号槽外观与高秆包围。
    assert_eq!(patch.profile_slot(), GpuBladeProfileSlot::SLOT_1); // 一号内容编号必须可无损读回。
    assert!(patch.uses_density_field()); // 内容编号不得覆盖最低密度启用位。
    assert_eq!(patch.lod_level(), 2); // 内容编号不得覆盖近中远几何LOD位。
    assert!(patch.uses_hierarchical_lod()); // 内容编号不得覆盖同源父簇位。
    assert!((patch.max_blade_height() - 1.35).abs() < 0.0001); // Compute保守剔除必须覆盖最高成熟麦穗。
    assert!((patch.reference_blade_height() - 1.18).abs() < 0.0001); // 屏幕LOD必须使用典型麦株而非普通草高度。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("VEGETATION_PROFILE_SHIFT")
            && COMPUTE_SHADER_SOURCE.contains("grass_patch.vegetation_bounds"),
        "Compute shader必须消费麦田profile编号和内容级保守包围",
    ); // 防止Rust配置存在但GPU仍硬编码普通草尺寸。
}

#[test] // 第四级预算位必须与密度、层级、内容profile和两位LOD身份完全独立。
fn ultra_lod_budget_preserves_all_existing_patch_flags() {
    let patch = GpuBladePatch::square(Vec3::ZERO, 2464.0, 3521, 0x4B46_0004)
        .with_density_field(Vec2::splat(-2500.0), Vec2::splat(2500.0)) // 普通草继续读取正式密度图。
        .with_lod_level(3) // 选择第四级超远几何身份。
        .with_hierarchical_lod(true) // 四级共享世界父格严格交接。
        .with_vegetation_config(GpuBladeConfig::DEFAULT)
        .with_ultra_lod_budget(GpuBladeUltraLodBudget::Dense); // 稠密预算以3×3 LOD3格选一固定代理。
    assert!(patch.uses_ultra_lod_chain()); // 任一显式质量档都必须打开第四级链标志。
    assert_eq!(patch.ultra_lod_budget_block_side(), 3); // Dense固定映射为3×3温和增密预算。
    assert!(patch.uses_density_field()); // 预算位不得覆盖最低密度图标志。
    assert!(patch.uses_hierarchical_lod()); // 预算位不得覆盖父簇层级标志。
    assert_eq!(patch.lod_level(), 3); // 预算位不得覆盖两位几何LOD。
    assert_eq!(patch.profile_slot(), GpuBladeProfileSlot::SLOT_0); // 预算位不得把零号槽误解码为其他内容。

    let high = patch.with_ultra_lod_budget(GpuBladeUltraLodBudget::High);
    let medium = patch.with_ultra_lod_budget(GpuBladeUltraLodBudget::Medium);
    let low = patch.with_ultra_lod_budget(GpuBladeUltraLodBudget::Low);
    assert_eq!(high.ultra_lod_budget_block_side(), 2); // High增加代理密度但仍有固定结构上限。
    assert_eq!(medium.ultra_lod_budget_block_side(), 4); // Medium保留原4×4平衡回退预算。
    assert_eq!(low.ultra_lod_budget_block_side(), 8); // Low以8×8稀疏块提供最低固定成本回退。
    assert_eq!(high.instance_count(), patch.instance_count()); // 质量档不改变候选口径，只改变GPU稳定结构选择。
    assert_eq!(medium.instance_count(), patch.instance_count()); // 回退档同样保留完整候选遥测口径。
    assert_eq!(low.instance_count(), patch.instance_count()); // F3候选数不能因输出预算而被伪装减少。
} // 完成第四级标志位与质量档编码回归。

#[test] // 渲染世界驻留标志不得占用密度或近中远LOD位，也不得把候选数量清零。
fn resident_output_flag_preserves_patch_render_contract() {
    let mut patch = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 1)
        .with_density_field(Vec2::splat(-32.0), Vec2::splat(32.0)) // 启用最低位密度采样。
        .with_lod_level(2) // 启用第二至三位中的远景材质档。
        .with_hierarchical_lod(true); // 启用独立的正式父簇标志。
    let candidates = patch.instance_count(); // 保存进入等待帧前的真实候选数量。
    assert!(!patch.retains_resident_output_for_render_frame()); // 主世界正常参数默认不得冻结旧输出。
    patch.retain_resident_output_for_render_frame(); // 模拟高度或密度纹理热重载的一帧。
    assert!(patch.retains_resident_output_for_render_frame()); // 渲染副本必须显式进入驻留保留路径。
    assert_eq!(patch.instance_count(), candidates); // 保留旧输出与首次等待清零必须是两条不同路径。
    assert!(patch.uses_density_field()); // 高位驻留标志不得覆盖密度启用最低位。
    assert_eq!(patch.lod_level(), 2); // 高位驻留标志不得覆盖两个LOD材质位。
    assert!(patch.uses_hierarchical_lod()); // 临时驻留输出位不得覆盖持久父簇契约。
}

#[test] // 同源父簇必须由显式持久标志控制，且GPU在视图失效时只保留最远祖先。
fn hierarchical_lod_flag_is_independent_and_has_safe_gpu_fallback() {
    let mut patch = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 7)
        .with_density_field(Vec2::splat(-32.0), Vec2::splat(32.0))
        .with_lod_level(1)
        .with_hierarchical_lod(true); // 模拟正式中级窗口。
    assert!(patch.uses_hierarchical_lod()); // 链式接口必须写入持久层级位。
    assert!(patch.uses_density_field()); // 层级位不得覆盖密度最低位。
    assert_eq!(patch.lod_level(), 1); // 层级位不得覆盖材质级别。
    patch.set_hierarchical_lod(false); // 单圈兼容场景可显式退出父簇路径。
    assert!(!patch.uses_hierarchical_lod()); // 关闭后应恢复独立候选语义。
    assert!(patch.uses_density_field()); // 清除层级位仍不得触碰密度位。
    assert_eq!(patch.lod_level(), 1); // 清除层级位仍不得触碰LOD位。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("lod_selected = lod_level + 1u == content_lod_count()"),
        "相机视图无效时，任意档数都必须只保留实际存在的最远祖先",
    ); // 锁定WGSL安全回退分支不会被后续重构误删。
}

#[test]
fn render_frame_can_wait_for_height_without_losing_surface_contract() {
    let mut patch = GpuBladePatch::square(Vec3::new(0.0, 4.37, 0.0), 64.0, 64, 1)
        .with_height_field(Vec2::splat(-2500.0), Vec2::splat(2500.0), -2.0, 8.0); // 模拟主世界已经配置正式高度字段。
    patch.suppress_draw_for_render_frame(); // 模拟渲染图片仍在异步上传的单帧等待。
    assert_eq!(patch.instance_count(), 0); // 纹理未就绪时不得先显示错误固定平面草。
    assert!(patch.uses_height_field()); // 临时等待不得改写主世界要求的真实高度语义。
    assert_eq!(
        patch.height_world,
        Vec4::new(-2500.0, -2500.0, 2500.0, 2500.0)
    ); // 临时等待不得丢失下一帧仍需使用的世界范围。
    assert_eq!(patch.height_decode.x, -2.0); // 解码下界保持不变。
    assert_eq!(patch.height_decode.y, 10.0); // 解码跨度保持不变。
}

#[test]
fn density_field_uses_independent_world_bounds_and_render_wait() {
    let mut patch = GpuBladePatch::square(Vec3::ZERO, 716.8, 2049, 1)
        .with_lod_level(2) // 先写入远景材质位，验证后续密度标志不会覆盖它。
        .with_density_field(
            Vec2::new(-2500.0, -2400.0), // 使用故意不对称的最小角验证字段没有复用高度范围。
            Vec2::new(2500.0, 2400.0),   // 使用故意不对称的最大角验证X/Z顺序。
        ); // 模拟正式场景已启用红通道密度概率采样。
    assert!(patch.uses_density_field()); // 主世界配置必须显式打开最低标志位。
    assert_eq!(patch.lod_level(), 2); // 同一grid.w中的远景材质位必须完整保留。
    assert_eq!(
        patch.density_world,
        Vec4::new(-2500.0, -2400.0, 2500.0, 2400.0)
    ); // 世界矩形按X/Z原顺序打包。
    patch.suppress_draw_for_render_frame(); // 模拟密度图片尚未进入GPU资产表。
    assert_eq!(patch.instance_count(), 0); // 该帧必须跳过draw而不是先以百分百密度闪现。
    assert!(patch.uses_density_field()); // 临时等待不得清除下一帧仍需恢复的密度语义。
    patch.set_lod_level(0); // 再切回近景档验证只清除两个LOD位。
    assert!(patch.uses_density_field()); // 更新材质档不得意外关闭正式密度图。
    assert_eq!(patch.lod_level(), 0); // 近景档应能独立更新而不改变其他标志。
    assert_eq!(
        patch.density_world,
        Vec4::new(-2500.0, -2400.0, 2500.0, 2400.0)
    ); // 临时等待不得丢失下一帧范围。
}

#[test]
fn density_control_region_adds_a_strict_optional_world_clip() {
    let mut patch = GpuBladePatch::square(Vec3::ZERO, 1024.0, 257, 2) // 构造公里级候选窗口。
        .with_density_field(Vec2::splat(-3000.0), Vec2::splat(3000.0)); // 原始密度图仍覆盖完整六公里场景。
    assert!(!patch.uses_density_control_region()); // 默认零范围不得改变既有场景生草行为。
    patch.set_density_control_region(
        Vec2::new(910.0, -1340.0), // 东部芦汀受控矩形世界X/Z最小角。
        Vec2::new(2590.0, 60.0),   // 东部芦汀受控矩形世界X/Z最大角。
    ); // 在原始密度图之上追加首轮技术接入范围。
    assert!(patch.uses_density_control_region()); // 合法正范围必须启用GPU快速拒绝。
    assert_eq!(
        patch.density_control_world,
        Vec4::new(910.0, -1340.0, 2590.0, 60.0)
    ); // 受控区必须独立保存，不能覆盖密度图完整世界映射。
    assert!(COMPUTE_SHADER_SOURCE.contains("control_enabled")); // WGSL必须实际消费新增范围而非只扩充Rust结构。
    let original = patch; // 保存合法矩形验证异常更新的原子拒绝。
    patch.set_density_control_region(Vec2::ZERO, Vec2::splat(f32::INFINITY)); // 非有限上界不得污染GPU uniform。
    patch.set_density_control_region(Vec2::ONE, Vec2::ZERO); // 反向范围同样不得自动猜测调用方意图。
    assert_eq!(patch, original); // 两次非法设置均完整保留上一份合法契约。
}

#[test]
fn patch_local_lod_config_overrides_view_defaults_without_changing_candidates() {
    let baseline =
        GpuBladePatch::square(Vec3::ZERO, 1203.2, 377, 4).with_vegetation_config(LOW_SLOT_3_CONFIG); // 构造通用低矮四号槽超远候选块。
    let config = GpuBladeViewLodConfig::new_with_ultra(
        Vec2::new(3.5, 5.0),                  // 超远转远像素阈值。
        Vec2::new(12.0, 20.0),                // 远转中像素阈值。
        Vec2::new(38.0, 54.0),                // 中转近像素阈值。
        Vec2::new(0.25, 0.65),                // 低矮草冠近镜头避让。
        0.42,                                 // 沙洲矮草参考株高。
        0.40,                                 // 最细世界母格。
        Vec4::new(50.0, 100.0, 220.0, 600.0), // 四档常驻半径。
        2.0,                                  // 子级安全边距。
        Vec3::new(10.0, 20.0, 72.0),          // 三段常驻回退宽度。
    ); // 完成沙洲矮草集中局部标定。
    let patch = baseline.with_view_lod_config(config); // 只写入四个草窗级vec4。
    assert_eq!(patch.instance_count(), baseline.instance_count()); // 局部LOD参数不得改变377平方候选上限。
    assert_eq!(
        patch.view_lod_pixel_thresholds,
        Vec4::new(12.0, 20.0, 38.0, 54.0)
    ); // 两段阈值必须按WGSL读取顺序打包。
    assert_eq!(patch.view_lod_ultra_fade, Vec4::new(3.5, 5.0, 0.25, 0.65)); // 超远阈值与镜头避让必须同时保留。
    assert_eq!(
        patch.view_lod_residency_radii,
        Vec4::new(50.0, 100.0, 220.0, 600.0)
    ); // 四档基础半径保留原布局，局部配置开关由独立档数元数据承担。
    assert_eq!(
        patch.view_lod_residency_transitions,
        Vec4::new(10.0, 20.0, 72.0, 2.0)
    ); // 三段回退与安全边距必须保持确认规格。
    assert!(COMPUTE_SHADER_SOURCE.contains("uses_patch_view_lod_config")); // WGSL必须真实选择草窗局部配置而非只扩充Rust布局。
    assert!(COMPUTE_SHADER_SOURCE.contains("content_camera_fade_distances")); // 近镜头避让同样必须按内容选择。

    let mut invalid = patch; // 保存合法局部配置验证异常更新原子拒绝。
    invalid.set_view_lod_config(GpuBladeViewLodConfig::new_with_ultra(
        Vec2::new(5.0, 3.5),   // 故意反向超远阈值。
        Vec2::new(12.0, 20.0), // 其余维度保持合法。
        Vec2::new(38.0, 54.0),
        Vec2::new(0.25, 0.65),
        0.42,
        0.40,
        Vec4::new(50.0, 100.0, 220.0, 600.0),
        2.0,
        Vec3::new(10.0, 20.0, 72.0),
    )); // 非单调阈值不得部分覆盖已有uniform。
    assert_eq!(invalid, patch); // 非法局部配置完整保持上一份合法值。
}

#[test]
fn lod_ring_and_density_gain_keep_strict_safe_ranges() {
    let patch = GpuBladePatch::square(Vec3::ZERO, 100.0, 101, 1) // 构建半径上限五十米的测试窗口。
        .with_lod_range(20.0, 45.0, 5.0, 8.0) // 配置中圈常用的内外交叉过渡。
        .with_lod_level(1) // 中圈必须把材质级别随同候选参数送入GPU。
        .with_density_gain(8.0); // 使用能明确验证钳制行为的非零概率增益。
    assert_eq!(patch.inner_radius(), 20.0); // 内圈空洞必须按调用方世界米制距离保存。
    assert_eq!(patch.visible_radius(), 45.0); // 外圈完全消失半径继续由公开接口读取。
    assert_eq!(patch.density_gain(), 8.0); // 密度增益不得被LOD范围配置覆盖。
    assert_eq!(patch.lod_level(), 1); // 中景材质级别必须与密度启用位彼此独立。

    let clamped = GpuBladePatch::square(Vec3::ZERO, 100.0, 101, 1) // 再构造一块验证越界保护。
        .with_lod_range(90.0, 80.0, 0.0, 200.0) // 故意把内外半径和过渡都配置到非法范围。
        .with_lod_level(99) // 故意超过当前八档几何身份范围。
        .with_density_gain(1000.0); // 故意超过公开增益安全上界。
    assert_eq!(clamped.visible_radius(), 50.0); // 外半径不得越过正方形窗口内切圆。
    assert!(clamped.inner_radius() < clamped.visible_radius()); // 内半径必须为smoothstep保留严格正环宽。
    assert_eq!(clamped.density_gain(), 64.0); // 过大增益必须稳定钳制到有限上界。
    assert_eq!(clamped.lod_level(), 7); // 未定义级别必须稳定钳到最高几何身份档。
}

#[test] // 所有公开浮点输入都必须阻止NaN和无穷值进入GPU uniform。
fn grass_patch_rejects_non_finite_public_inputs() {
    let mut patch = GpuBladePatch::square(
        // 使用异常构造参数先验证初始净化路径。
        Vec3::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY), // 三个异常坐标不得写进世界中心。
        f32::INFINITY, // 无限边长不得生成无限格距、半径或AABB。
        16,            // 保留正常候选单边数量以隔离浮点输入行为。
        9,             // 固定种子不参与本测试。
    ); // 完成异常草窗构造。
    assert_eq!(patch.origin(), Vec3::ZERO); // 异常构造中心必须整体回退到有限原点。
    assert!(patch.grid_spacing().is_finite()); // 净化后的格距必须可安全参与吸附计算。
    let original = patch; // 保存全部416字节合法基线供后续原子拒绝断言。
    patch.set_origin(Vec3::splat(f32::NAN)); // 尝试污染窗口世界中心。
    patch.set_focus(Vec3::splat(f32::INFINITY)); // 尝试污染连续观察中心。
    patch.set_density_gain(f32::NAN); // 尝试污染密度概率增益。
    patch.set_lod_range(0.0, f32::INFINITY, 1.0, 1.0); // 尝试以无限外半径更新环带。
    patch.set_height_field(
        // 尝试以异常世界矩形启用高度纹理。
        Vec2::splat(f32::NAN), // 非有限最小角必须让整次设置失败。
        Vec2::ONE,             // 正常最大角不能导致部分字段写入。
        -2.0,                  // 合法高度下界同样不得被部分保存。
        8.0,                   // 合法高度上界同样不得被部分保存。
    ); // 完成异常高度契约调用。
    patch.set_density_field(Vec2::ZERO, Vec2::splat(f32::INFINITY)); // 尝试以异常上界启用密度纹理。
    assert_eq!(patch, original); // 所有可变接口必须完整保留调用前合法uniform，而不是部分更新。
    assert_eq!(
        // 非有限吸附输入同样不得返回NaN世界中心。
        patch.snapped_origin_xz(Vec2::splat(f32::NAN), 64), // 模拟玩家Transform短暂出现异常值。
        patch.origin().xz(),                                // 安全回退必须保持当前草窗中心。
    ); // 完成吸附异常回退断言。
    let chained = patch // 验证按值构建接口也保持旧值而不是传播异常。
        .with_visible_radius(f32::NAN) // 非有限可见半径应保持原值。
        .with_density_gain(f32::INFINITY) // 非有限密度增益应保持原值。
        .with_lod_range(0.0, f32::NAN, 1.0, 1.0); // 非有限环带应完整拒绝。
    assert_eq!(chained, patch); // 链式接口必须与可变接口采用同一原子拒绝语义。
} // 完成公开GPU草窗浮点边界回归测试。
#[test]
fn volume_candidates_use_each_lods_actual_world_spacing() {
    let mut counts = Vec::new();
    for level in 0..4 {
        let side = 1 << level;
        let patch = GpuBladePatch::square(Vec3::ZERO, 4., 32 / side + 1, 7)
            .with_lod_level(level)
            .with_hierarchical_lod(true)
            .with_volume_cell_source(8, 2.0);
        counts.push(patch.instance_count());
        assert_eq!(patch.density_world.y, 0.125, "LOD变化不能改变正式最细母格");
    }
    assert_eq!(counts, vec![8 * 18 * 18, 8 * 10 * 10, 8 * 6 * 6, 8 * 4 * 4]);
}
