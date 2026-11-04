//! 雨天地表生产库与完整Forward/Deferred片元的Naga回归，以及覆盖门控的数学边界验证。

use super::wet_surface_test_support::*;

/// 原生GPU烟雾测试复用同一完整生产片元，只替换Bevy宿主接口。
pub(super) fn native_forward_source() -> String {
    let defines = ["VERTEX_COLORS", "VERTEX_UVS_B"];
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        preprocess(PBR_STUB, &defines),
        include_str!("shaders/rain_shelter.wgsl"),
        preprocess(WEATHER, &defines),
        preprocess(RAIN, &defines),
        preprocess(WET, &defines),
        preprocess(TERRAIN, &defines)
    )
    .replace("@group(2)", "@group(0)")
}

#[test] // 独立湿润函数无Bevy全局依赖，应能单独通过真实Naga验证。
fn wet_surface_helper_is_valid_wgsl() {
    validate(&preprocess(WET, &[]), "湿润库");
} // 不维护第二份算法。

#[test] // 真实水介质的垂直反射率约百分之二；禁止用金属反射或亮色伪造水面。
fn puddle_reflectance_matches_water_ior_and_standard_pbr() {
    // 独立物理计算锁定Bevy映射与项目常量。
    let reflectance = WET
        .split("const WATER_SURFACE_REFLECTANCE: f32 = ")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .parse::<f32>()
        .unwrap(); // 读取生产数值而非测试侧重复常量。
    let expected_f0 = ((1.33_f32 - 1.0) / (1.33 + 1.0)).powi(2); // 空气与水界面的正常入射Fresnel。
    assert!((0.16 * reflectance * reflectance - expected_f0).abs() < 0.000001); // Bevy标准反射率重映射应与水IOR一致。
    assert!(TERRAIN.contains(
        "mix(pbr_input.material.reflectance, vec3<f32>(WATER_SURFACE_REFLECTANCE), wet.puddle)"
    )); // 仅积水膜替换反射，干地与湿土仍由作者材质控制。
    assert!(TERRAIN.contains("mix(pbr_input.material.metallic, 0.0, wet.puddle)")); // 水膜不是金属。
} // 完成物理反射率与覆盖边界回归。

#[test] // 晴天原值、有限方差及真实PBR输出必须保持，不能通过额外亮色伪造倒影。
fn rain_reflection_preserves_dry_authoring_and_bounds_microfacet_spread() {
    // 此契约配合完整生产WGSL三路径解析，锁定反射实现边界。
    let reflection = WET
        .split("fn water_reflection_roughness(")
        .nth(1)
        .unwrap()
        .split("struct WetSurfaceSample")
        .next()
        .unwrap(); // 只审计正式反射函数。
    assert!(reflection.contains("if rain_weight <= 0.0 { return author_roughness; }")); // 无雨必须精确保持晴天原值。
    assert!(
        reflection
            .contains("rain_weight * (0.014 + 0.026 * clamp(wind, 0.0, 1.0) + 0.040 * unresolved)")
    ); // 阻止无限风强或时间放大镜面分布。
    assert!(reflection.contains("sqrt(min(base * base + variance, 1.0))")); // 限制感知粗糙度范围且仅展宽，不提高高光能量。
    assert!(!reflection.contains("textureSample")); // 不额外取样环境，不增加反射纹理访问。
    assert!(!reflection.contains("color")); // 反射只通过原PBR响应，不合成凭空出现的建筑颜色。
    assert!((0.12_f32 * 0.12 + 0.014 + 0.026 + 0.040).sqrt() < 0.31); // 最大雨风和远景下，水膜仍保留有限光滑程度。
} // 完成晴天基线与有限镜面展宽契约。

#[test] // HN02直通材质在干燥状态完整保留Blender PbrInput，雨湿逻辑位于采样门外。
fn passthrough_skips_author_layers_before_shared_weather() {
    // 完整fragment Naga测试同时编译此生产运行时分支。
    let gate = TERRAIN
        .find("if terrain_surface.appearance.w < 1.5 {")
        .expect("必须保留原材质直通门"); // w=2不进入作者层替换。
    let indexed = TERRAIN
        .find("let indexed = blend_indexed_surface")
        .expect("索引地表保留原路径"); // 动态地图仍经过原混合。
    let fixed = TERRAIN
        .find("let blended = blend_surface")
        .expect("固定地表保留原路径"); // HN01仍经过原混合。
    let join = TERRAIN
        .find("固定层、索引层和原材质直通在此汇合")
        .expect("三种模式必须在天气前汇合"); // 不把雨湿嵌入某一种地图模式。
    let weather = TERRAIN
        .find("if surface_weather.weather.x > 0.0001")
        .expect("必须保留共用天气门"); // 直通也能随天气变湿。
    assert!(gate < indexed && indexed < fixed && fixed < join && join < weather); // 层采样门覆盖所有旧PBR覆写，但不覆盖雨湿。
    assert!(
        TERRAIN[join..weather]
            .lines()
            .all(|line| !line.contains("pbr_input.material"))
    ); // 汇合到天气门之间不得额外覆盖原颜色。
} // 结束直通原色与同源天气入口契约。

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    // 数学边界测试显式使用WGSL定义的三次插值。
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0); // 固定合法阈值防止除零。
    t * t * (3.0 - 2.0 * t) // 连续值和一阶导数共同保证软边。
} // 结束测试侧数学原语。

fn coverage(potential: f32, amount: f32, normal_y: f32, noise: f32) -> f32 {
    // 只推导覆盖门控，不复制PBR或GPU噪声实现。
    let threshold = 1.02 - amount.clamp(0.0, 1.0) * 0.96; // 与下方源码契约绑定的生产阈值。
    smoothstep(0.0, 0.08, potential)
        * smoothstep(0.94, 0.997, normal_y)
        * smoothstep(threshold, threshold + 0.10, potential - noise * 0.14)
        * smoothstep(0.0, 0.08, amount) // 乘法门保证任一禁止条件严格归零。
} // 结束覆盖函数的数学推导。

#[test] // 无雨积累、无作者潜力、陡坡必须精确零覆盖，湿润历史独立存在不授权水坑。
fn dry_unpainted_and_steep_surfaces_have_no_puddles() {
    // 锁定实际生产公式后再验证边界，避免参考测试与实现静默分叉。
    assert!(WET.contains("let threshold = 1.02 - clamp(amount, 0.0, 1.0) * 0.96;")); // 阈值数学必须与生产一致。
    assert!(WET.contains("smoothstep(0.94, 0.997, geometry_normal.y)")); // 陡坡门必须使用几何法线。
    assert!(WET.contains("eligible * smoothstep(threshold, threshold + 0.10, potential - noise * 0.14) * smoothstep(0.0, 0.08, amount)")); // 归零因子不能被未来重构移除。
    for noise in [0.0, 0.5, 1.0] {
        // 覆盖全部噪声极值与平均值。
        assert_eq!(coverage(1.0, 0.0, 1.0, noise), 0.0); // 干燥零积水量禁止水膜。
        assert_eq!(coverage(0.0, 1.0, 1.0, noise), 0.0); // 无作者图/未涂绘位置禁止水膜。
        assert_eq!(coverage(1.0, 1.0, 0.94, noise), 0.0); // 二十度附近及更陡面禁止水膜。
        assert_eq!(coverage(1.0, 1.0, 0.0, noise), 0.0); // 墙面不能积水。
    } // 完成独立门控边界。
    assert!(coverage(1.0, 1.0, 1.0, 0.5) > 0.99); // 合法强潜力平地必须确实能出现完整积水。
} // 结束干燥、缺遮罩和陡坡回归。

#[test] // 覆盖应随积水历史单调增长且阈值边界连续，不能突然铺成整图镜面。
fn puddle_growth_is_monotonic_and_boundary_is_continuous() {
    // 数学测试不声称代替GPU实机画面验收。
    let mut previous = 0.0; // 从干燥状态开始检查完整形成过程。
    for step in 0..=1000 {
        // 固定一千个代表时间样本仅存在于测试。
        let current = coverage(0.58, step as f32 / 1000.0, 1.0, 0.5); // 同一作者点随积水历史推进。
        assert!(current >= previous - 0.000001); // 扩张过程中同一点不得反向干燥。
        assert!((0.0..=1.0).contains(&current)); // 所有门控维持合法凸混合范围。
        previous = current; // 保存当前值供下一个历史样本比较。
    } // 完成全形成过程扫描。
    for amount in [0.3, 0.6, 0.9] {
        // 验证不同雨程对应的边缘阈值。
        let boundary = 1.02 - amount * 0.96 + 0.5 * 0.14; // 反解正式边缘噪声修正后的潜力阈值。
        let before = coverage(boundary - 0.00001, amount, 1.0, 0.5); // 边缘外极近邻。
        let after = coverage(boundary + 0.00001, amount, 1.0, 0.5); // 边缘内极近邻。
        assert!((after - before).abs() < 0.00001); // 光滑边缘不存在二值跳变。
    } // 完成不同阈值的连续边界检查。
} // 结束连续扩张数学回归。
