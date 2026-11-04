//! 正式大地图GPU动态透明水面与玩家波源的纯逻辑测试。
//! 本文件验证Shader参考公式、岸边固定、寿命衰减和触发节奏，不加载GLB或渲染资源。

use super::*; // 复用父模块私有常量、组件和纯波纹 helper。

// 跨包的水/雨/地表完整片元契约由 SDK 的 tests/workspace 验证，单包测试不读取其他包的源码。

const WATER_SHADER_SOURCE: &str = include_str!("shaders/water.wgsl"); // 直接读取生产水面Shader，禁止测试维护第二份波形实现。
const WATER_FRAGMENT_SHADER_SOURCE: &str = include_str!("shaders/fragment.wgsl"); // 直接锁定正式透明PBR片元流动实现。
const WATER_VERTEX_SHADER_SOURCE: &str = include_str!("shaders/vertex.wgsl"); // 锁定前向顶点在无UV1时仍把UV0传入公共水采样。
const WATER_PREPASS_SHADER_SOURCE: &str = include_str!("shaders/prepass.wgsl"); // 锁定预通道与主通道共用同一uniform哨兵分支。
const WATER_MATERIAL_SOURCE: &str = include_str!("material.rs"); // 锁定扩展材质必须选择新片元入口。
const WATER_SHADER_REGISTRY_SOURCE: &str = include_str!("shaders.rs"); // 锁定内置Shader资产必须随插件登记。
const LEGACY_WATER_SYSTEMS_SOURCE: &str = include_str!("systems.rs"); // 锁定旧GLB动态水的方向光阴影接收策略。
const PAINTED_WATER_SPAWN_SOURCE: &str = include_str!("painted/spawn.rs"); // 锁定绘制水与GPU预热实体使用同一接影策略。

#[test] // 水面生产WGSL必须通过当前Bevy所用同版本Naga的语法与语义校验。
fn dynamic_water_shader_is_valid_wgsl() {
    let validation_source = WATER_SHADER_SOURCE
        .strip_prefix("#define_import_path feathrise_scene::shared::large_map::water::water")
        .expect("水面Shader必须保留稳定的Bevy导入路径")
        .replace("#{MATERIAL_BIND_GROUP}", "2"); // 仅把Bevy加载器负责的预处理占位符还原为合法测试绑定组，其他内容全部来自生产源码。
    let module = wgpu::naga::front::wgsl::parse_str(&validation_source)
        .unwrap_or_else(|error| panic!("动态水面WGSL语法错误：{error}")); // 解析失败时保留Naga精确行列信息。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(), // 覆盖结构布局、表达式、循环与内建函数合法性。
        wgpu::naga::valid::Capabilities::all(), // 使用完整WebGPU能力集合验证标准WGSL，不误拒绝正式后端能力。
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("动态水面WGSL语义错误：{error}")); // 阻止错误延迟到实机创建水材质管线时才暴露。
} // 结束生产水面Shader解析回归。

#[test] // 逐像素程序流场本体必须通过当前Bevy所用同版本Naga语法与语义校验。
fn ribbon_fragment_flow_helper_is_valid_wgsl() {
    let helper_start = WATER_FRAGMENT_SHADER_SOURCE
        .find("// 逐像素流动结果只保存标准PBR输入需要的有限标量")
        .expect("片元Shader必须保留程序流场起始标记"); // 只截取不依赖Bevy导入的正式生产helper，不复制另一份算法。
    let helper_end = WATER_FRAGMENT_SHADER_SOURCE
        .find("@fragment")
        .expect("片元Shader必须保留正式fragment入口"); // 标准PBR包装由源码契约测试继续锁定。
    let helper_source = &WATER_FRAGMENT_SHADER_SOURCE[helper_start..helper_end]; // 截取结构、常量和完整程序流场函数。
    assert!(
        helper_source.contains("fn ribbon_flow_frame_from_derivatives("), // 确认本次Naga输入包含缺TANGENT时的正式UV0雅可比回退，而不只是颜色流场。
        "Naga校验片段必须覆盖缺TANGENT流向坐标架helper",
    ); // 防止移动源码标记后悄悄漏掉新helper的语法与语义校验。
    assert!(
        helper_source.contains("fn sample_water_micro_normal("), // 同一生产片段还必须覆盖两张可选纹理与采样器参数。
        "Naga校验片段必须覆盖静水滚动微法线helper",
    ); // 防止微法线函数移出独立校验范围后只在实机创建管线时才发现错误。
    let validation_source = format!(
        "const RIBBON_STYLE_SENTINEL: f32 = 16.0;\nfn water_bank_fade(uv: vec2<f32>, fade_width: f32) -> f32 {{ return clamp(uv.y / max(fade_width, 0.0001), 0.0, 1.0); }}\nfn water_wrap_safe_phase_offset(elapsed: f32, angular_speed: f32) -> f32 {{ let cycles = round(angular_speed * 3600.0 / 6.283185307179586); return fract(elapsed / 3600.0 * cycles) * 6.283185307179586; }}\n{helper_source}",
    ); // 用同签名纯WGSL桩替代Bevy导入的共享岸边函数，使Naga能独立验证真实helper正文。
    let module = wgpu::naga::front::wgsl::parse_str(&validation_source)
        .unwrap_or_else(|error| panic!("样条水逐像素流场WGSL语法错误：{error}")); // 解析失败时保留精确行列。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(), // 覆盖正弦、结构构造和所有控制流语义。
        wgpu::naga::valid::Capabilities::all(),    // 使用完整WebGPU能力集合避免误拒绝正式后端。
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("样条水逐像素流场WGSL语义错误：{error}")); // 阻止错误延迟到实机管线创建。
} // 结束逐像素流场生产helper校验。

#[test] // 样条河必须保留无需环境反射也可观察的米制下游流纹，防止再次退化成顶点级微波。
fn ribbon_water_shader_keeps_visible_downstream_advection() {
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE
            .contains("water_wrap_safe_phase_offset(elapsed, visual_flow_speed"), // 每层使用负安全相位项保证沿UV0.x增加方向移动。
        "样条河逐像素流场必须按实际沿程米数和回绕安全速度向下游平移",
    ); // 锁定片元流向而不在测试中维护第二份Shader公式。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE
            .contains("let visual_flow_speed = min(max(average_flow_speed, 0.0), 8.0);"), // 整河平均速度和有限上限共同阻止局部速度梯度及极值制造频闪。
        "样条河逐像素流纹必须使用稳定且有上限的整河视觉速度",
    ); // 防止再次把逐顶点速度与绝对时间相乘造成空间相位翻转。
    assert!(
        !WATER_FRAGMENT_SHADER_SOURCE.contains("elapsed * local_flow_speed"), // 逐顶点速度梯度乘绝对时间会让相位随运行时长无限压缩并最终反转。
        "样条河任何视觉相位都不得恢复绝对时间乘局部流速",
    ); // 锁定A地图已复现的长期闪烁根因不能重新进入生产Shader。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains("* (1.0 - flow.trough * 0.24);"), // 暗色水团提供无需高光的直接颜色运动。
        "样条河必须保留非零逐像素暗流纹对比",
    ); // 防止后续只保留动态法线、再次让弱反射场景看不出运动。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE
            .contains("+ speed_energy * 0.22 + rapid_weight * 0.45 + waterfall_weight * 0.25",), // 普通高速河、急流和瀑布必须具有逐级泡沫层次。
        "样条水必须按水势与样式逐级增强破碎白沫",
    ); // 防止普通河、急流和瀑布再次退化成同一种平面水。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains(
            "let spline_ribbon_mode = encoded_ribbon_style >= -0.5 && encoded_ribbon_style <= 2.5;"
        ), // 只有专用高位样式哨兵附近的UV1才能进入样条分支。
        "普通第二套UV不得被误判为样条水流数据",
    ); // 防止带常规UV1的旧GLB水面被强制改色、加泡沫和扰动法线。
    assert!(
        WATER_SHADER_SOURCE.contains("const RIBBON_STYLE_SENTINEL: f32 = 16.0;"), // 顶点采样必须使用与Rust网格编码相同的高位值。
        "顶点Shader必须锁定样条UV1身份哨兵",
    ); // 防止顶点与片元对同一生成网格给出不同水体身份。
    assert!(
        !WATER_SHADER_SOURCE.contains("flow_visual_signal"), // 顶点层不应再重复生成同一颜色信号。
        "逐像素流纹接管后不得继续叠加顶点流纹乘色",
    ); // 防止两套相位互相打架或宽河重新出现三角插值痕迹。
} // 结束样条河下游逐像素流纹生产源码契约回归。

#[test] // Bevy Globals在每小时边界回绕，任意作者流速的三角相位必须仍连续。
fn quantized_water_phase_is_continuous_across_global_time_wrap() {
    const TAU: f32 = std::f32::consts::TAU;
    const WRAP_SECONDS: f32 = 3_600.0;
    fn phase(elapsed: f32, angular_speed: f32) -> f32 {
        let cycles = (angular_speed * WRAP_SECONDS / TAU).round();
        (elapsed / WRAP_SECONDS * cycles).fract() * TAU
    }
    fn wrapped_delta(left: f32, right: f32) -> f32 {
        (right - left).sin().atan2((right - left).cos()).abs()
    }

    for angular_speed in [0.105 * 0.37, 0.75 * 2.345, TAU * 7.913 / 2.8] {
        assert!(
            wrapped_delta(
                phase(WRAP_SECONDS, angular_speed),
                phase(0.0, angular_speed)
            ) < 0.0001,
        ); // 整点回绕前后的正弦/余弦方向严格相同。
        assert!(
            wrapped_delta(
                phase(WRAP_SECONDS - 0.01, angular_speed),
                phase(0.01, angular_speed),
            ) < angular_speed.abs() * 0.03 + 0.001,
        ); // 边界两侧只推进约二十毫秒，不发生与流速无关的整体跳相。
    }
    assert!(WATER_SHADER_SOURCE.contains("fn water_wrap_safe_phase_offset")); // 数值证明必须对应正式共享Shader helper。
} // 结束每小时水动画相位连续性回归。

#[test] // Blender整网格即使没有UV1也必须在顶点、预通道和片元三处共用UV0流场。
fn whole_mesh_water_uses_uniform_style_sentinel_without_uv1() {
    let normalized_fragment_shader = WATER_FRAGMENT_SHADER_SOURCE.replace("\r\n", "\n"); // Windows检出仍验证同一WGSL片段。
    assert!(
        WATER_SHADER_SOURCE.contains(
            "let encoded_whole_mesh_style = surface_settings.y - RIBBON_STYLE_SENTINEL;",
        ), // 顶点与预通道共用函数必须从uniform流向x槽解码整网格身份。
        "公共水采样必须支持uniform整网格样式哨兵",
    );
    assert!(
        WATER_SHADER_SOURCE.contains("let ribbon_mode = whole_mesh_mode || spline_ribbon_mode;"), // 整网格分支与旧样条分支共用已验证流场实现。
        "uniform哨兵和旧UV1样条必须并列进入流场",
    );
    assert!(
        WATER_SHADER_SOURCE.contains("spline_ribbon_mode && !whole_mesh_mode",), // 整网格即使原资产带普通UV1也只能使用uniform作者速度。
        "整网格水不得误读原UV1.x为局部流速",
    );
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains("var ribbon_mode = whole_mesh_mode;"), // 片元分支在UV1条件编译之外就可由uniform启用。
        "整网格片元流场不得依赖VERTEX_UVS_B",
    );
    assert!(
        normalized_fragment_shader.contains(
            "dynamic_water_uniform.current.surface_settings.w,\n        RIBBON_STYLE_SENTINEL + max(encoded_whole_mesh_style, 0.0),",
        ), // 无UV1时从uniform合成共用helper所需的速度与样式。
        "整网格片元流场必须使用uniform流速和样式",
    );
    for shader in [WATER_VERTEX_SHADER_SOURCE, WATER_PREPASS_SHADER_SOURCE] {
        assert!(shader.contains("vertex.uv,")); // 两个顶点入口都必须把原UV0传给共用采样，UV1只是可选的旧样条数据。
        assert!(shader.contains("rest_uv_b,")); // 无UV1时的有限占位值不影哇uniform整网格解码。
    } // 完成前向与prepass输入一致性检查。
} // 完成无UV1整网格流场源码契约回归。

#[test] // Blender整网格缺失TANGENT时必须保留波高位移，并只在可恢复真实UV0方向时扰动片元法线。
fn whole_mesh_water_safely_falls_back_without_vertex_tangents() {
    for shader in [WATER_VERTEX_SHADER_SOURCE, WATER_PREPASS_SHADER_SOURCE] {
        assert!(
            shader.contains("var rest_local_tangent = vec3<f32>(0.0, 0.0, 0.0);"), // 零向量是公共采样识别“无切线”的唯一入口契约。
            "前向与预通道缺TANGENT时都必须传入零向量",
        );
        assert!(
            !shader.contains("var rest_local_tangent = vec3<f32>(1.0, 0.0, 0.0);"), // 固定+X会在任意弯河上制造错误方向的动态法线。
            "缺TANGENT时不得伪造固定+X切线",
        );
    } // 锁定前向、阴影、深度和运动向量入口使用一致的缺失语义。
    assert!(
        WATER_SHADER_SOURCE.contains(
            "let has_valid_flow_tangent = dot(rest_local_tangent, rest_local_tangent) > 0.000001;",
        ), // 公共顶点采样必须显式区分合法切线与缺失零向量。
        "公共水采样必须识别合法流向切线",
    );
    assert!(
        WATER_SHADER_SOURCE.contains("if ribbon_mode && has_valid_flow_tangent {"), // 缺切线时只禁用沿程细节梯度，UV相位与波高计算仍位于此前的样条分支。
        "缺TANGENT时必须禁用方向性解析height_gradient",
    );
    assert!(
        WATER_SHADER_SOURCE.contains("primary_gradient = vec3<f32>(0.0, 0.0, 0.0);"), // 低频波高仍计算，但不能把uniform哨兵触发的+X回退写入法线。
        "缺TANGENT样条水必须把背景方向性梯度安全归零",
    );

    let derivative_position = WATER_FRAGMENT_SHADER_SOURCE
        .find("let world_position_dx = dpdx(in.world_position.xyz);")
        .expect("片元Shader必须统一求取世界位置导数"); // 世界位置与UV0导数共同构成可反演雅可比。
    let ribbon_branch_position = WATER_FRAGMENT_SHADER_SOURCE
        .find("    if ribbon_mode {")
        .expect("片元Shader必须保留动态水身份分支"); // 身份来自varying数据，导数不能放入这个非统一分支。
    assert!(
        derivative_position < ribbon_branch_position, // 四像素导数必须在任何动态水条件分支之前求值。
        "世界位置与UV0导数必须位于统一控制流区",
    );
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains("let uv_dx = dpdx(in.uv);"), // UV0屏幕x导数供带限和雅可比共同复用。
        "缺TANGENT回退必须读取UV0屏幕导数",
    );
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE
            .contains("let uv_determinant = uv_dx.x * uv_dy.y - uv_dy.x * uv_dx.y;",), // 行列式保留镜像UV的+U/+V方向，并识别不可反演映射。
        "缺TANGENT回退必须反演世界位置与UV0雅可比",
    );
    let mesh_tangent_position = WATER_FRAGMENT_SHADER_SOURCE
        .find("let tangent_candidate = in.world_tangent.xyz")
        .expect("片元Shader必须保留合法VERTEX_TANGENTS路径"); // 已有资产切线仍是首选来源。
    let derivative_fallback_position = WATER_FRAGMENT_SHADER_SOURCE
        .find("if !has_flow_frame {")
        .expect("片元Shader必须在网格切线失败后进入导数回退"); // 缺失或退化的网格切线才允许雅可比接管。
    assert!(
        mesh_tangent_position < derivative_fallback_position, // 源码顺序锁定合法网格切线优先级。
        "合法VERTEX_TANGENTS必须优先于UV0导数回退",
    );
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains("if derivative_frame.valid {"), // 退化雅可比不得把零向量送入normalize或修改PBR法线。
        "UV0雅可比退化时必须安全跳过方向性微法线",
    );
} // 完成无TANGENT整网格水安全降级源码契约回归。

#[test] // 新片元入口必须完整保留StandardMaterial透明度、PBR光照和后处理契约。
fn dynamic_water_fragment_keeps_standard_material_pbr_contract() {
    assert!(
        WATER_SHADER_REGISTRY_SOURCE.contains("DYNAMIC_WATER_FRAGMENT_SHADER_HANDLE"), // 固定句柄必须由插件登记为内置资产。
        "动态水面片元Shader句柄必须完成注册",
    ); // 防止仅新增文件却没有进入Shader资产表。
    assert!(
        WATER_SHADER_REGISTRY_SOURCE.contains("\"shaders/fragment.wgsl\""), // 注册入口必须指向正式生产文件。
        "动态水面片元Shader文件必须由内置资产注册",
    ); // 防止路径漂移导致运行时ShaderNotLoaded。
    assert!(
        WATER_MATERIAL_SOURCE.contains("fn fragment_shader() -> ShaderRef"), // 透明Blend河水只消费前向fragment_shader入口。
        "DynamicWaterExtension必须选择透明前向片元Shader",
    ); // 锁定A地图实际渲染路径而不是只接Deferred入口。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains("pbr_input_from_standard_material(in, is_front)"), // 先读取作者材质和顶点色。
        "动态水面片元必须从StandardMaterial完整PBR输入开始",
    ); // 禁止重新实现第二套低配河水材质。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE
            .contains("let original_alpha = pbr_input.material.base_color.a;"), // 保存作者最终Alpha。
        "逐像素流动必须显式保留作者透明度",
    ); // 明暗与白沫不能把透明河水改成不透明色块。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains("apply_pbr_lighting(pbr_input)"), // 继续使用标准直接光、环境光和阴影。
        "动态水面必须继续执行StandardMaterial PBR光照",
    ); // 防止为了可见流动退回Unlit替身材质。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE
            .contains("main_pass_post_lighting_processing(pbr_input, out.color)"), // 保留雾、透明预乘和曝光。
        "动态水面必须继续执行Bevy标准光照后处理",
    ); // 防止透明混合和天气雾契约在自定义片元中丢失。
} // 结束透明StandardMaterial PBR片元契约回归。

#[test] // 所有直接生成的动态水都必须让Standard PBR采样方向光阴影。
fn dynamic_water_spawn_paths_keep_directional_shadow_receiving_enabled() {
    assert!(
        !LEGACY_WATER_SYSTEMS_SOURCE.contains("NotShadowReceiver"), // 旧GLB水不得再显式退出方向光阴影接收。
        "旧GLB动态水必须接收山体对太阳/月亮直射光的遮挡",
    ); // 防止可见天体已被山体遮住时水面直射亮柱仍然存在。
    assert!(
        !PAINTED_WATER_SPAWN_SOURCE.contains("NotShadowReceiver"), // 绘制水与预热实体都不得生成禁接影标记。
        "绘制水必须接收山体对太阳/月亮直射光的遮挡",
    ); // 锁定正式水面仍可保留天空环境反射，但直射高光遵循级联阴影。
} // 完成直接生成水面的接影策略回归。

/// 构造只包含轮廓与接触状态的最小GPU水面CPU状态供纯逻辑测试使用。
fn test_water_surface() -> DynamicWaterSurface {
    // 测试不访问Assets<Mesh>或材质，只需要波源状态、岸线和静止水位。
    DynamicWaterSurface {
        // 使用与南京水域相同的静止水位和矩形测试岸线。
        interaction_footprint: WaterInteractionFootprint::Painted(PaintedWaterFootprint::from_visible_mesh(
            [1, 1], [-10.0, -5.0], [10.0, 5.0], vec![1.0; 4],
        )), // 保持原矩形接触范围，使用现行可见水面足迹。
        rest_water_level: -1.2,  // 与生成脚本中的秦淮河设计水位一致。
        flow_direction: Vec2::X, // 纯接触测试保持旧GLB默认流向。
        flow_speed: 1.0,         // 纯接触测试保持旧GLB默认流速。
        player_ripples: Vec::with_capacity(PLAYER_RIPPLE_CAPACITY), // 使用与运行时相同的固定预分配容量。
        last_player_takeoff_sequence: None, // 首次看到玩家只建立Tnua实际起跳序号基线。
        previous_player_local_xz: None,     // 初始尚未记录玩家局部位置。
        previous_player_local_y: None,      // 初始尚未记录脚底高度基准。
        distance_since_ripple: 0.0,         // 初始没有累计水中步距。
        player_was_in_water: false,         // 首次接触应触发入水波。
        jump_cycle: WaterJumpCycle::Idle,   // 初始没有等待落水结算的真实跳跃周期。
    } // 完成最小水面状态构造。
} // 结束测试水面构造 helper。

/// 使用当前已消费序号推进一帧，表示本帧没有新的Tnua实际起跳。
fn update_player_ripples(
    water_surface: &mut DynamicWaterSurface, // 接收测试水面的完整接触与波源状态。
    player_local_foot_position: Option<Vec3>, // 测试玩家脚底局部位置或缺失状态。
    delta_seconds: f32,                      // 推进波源寿命和深水下降历史的帧长。
) -> Option<PlayerWaterRipple> {
    let takeoff_sequence = water_surface.last_player_takeoff_sequence.unwrap_or(0); // 首帧使用零建立基线，后续保持最近已发布序号。
    super::update_player_ripples(
        water_surface,              // 调用生产纯逻辑更新同一测试状态。
        player_local_foot_position, // 传递当前可选脚底位置。
        Some(takeoff_sequence),     // 序号不变表示本帧没有新的实际起跳。
        delta_seconds,              // 使用调用方给出的真实帧长。
    ) // 返回本帧生产逻辑最后追加或合并的波源。
} // 结束无新起跳测试推进helper。

/// 发布一个新的Tnua实际起跳序号并推进同一帧水面状态。
fn update_player_ripples_with_takeoff(
    water_surface: &mut DynamicWaterSurface, // 接收即将消费新起跳身份的测试水面。
    player_local_foot_position: Option<Vec3>, // 实际起跳固定步完成后的玩家局部脚底。
    takeoff_sequence: u64,                   // 必须不同于水面上次已消费值，模拟Tnua ActionStarted。
    delta_seconds: f32,                      // 推进统一大波寿命与接触状态的帧长。
) -> Option<PlayerWaterRipple> {
    super::update_player_ripples(
        water_surface,              // 调用生产纯逻辑更新同一测试状态。
        player_local_foot_position, // 传递实际起跳后的可选脚底位置。
        Some(takeoff_sequence),     // 发布不会被渲染帧差漏掉的权威起跳身份。
        delta_seconds,              // 使用调用方给出的真实帧长。
    ) // 返回本帧生产逻辑追加或合并的强波。
} // 结束实际起跳测试推进helper。

#[test] // 瀑布持续动画不得借用、老化或淘汰玩家八槽事件池。
fn disabled_waterfall_interaction_never_touches_player_ripple_slots() {
    let mut water_surface = test_water_surface(); // 从字段完整的合法水面CPU状态开始。
    water_surface.interaction_footprint = WaterInteractionFootprint::Disabled; // 切换为统一生成入口给瀑布安装的禁用策略。
    for index in 0..PLAYER_RIPPLE_CAPACITY {
        water_surface.player_ripples.push(PlayerWaterRipple {
            center_local_xz: Vec2::new(index as f32, -(index as f32)), // 八个槽使用可辨别中心锁定FIFO内容。
            age: 0.25 + index as f32 * 0.01, // 每槽使用可辨别年龄确认禁用分支不会老化。
            amplitude: PLAYER_RIPPLE_ENTRY_AMPLITUDE, // 使用正式入水振幅填满固定池。
        });
    }
    let first_before = water_surface.player_ripples[0]; // 保存理论上会被满池新事件淘汰的最旧槽。
    let last_before = water_surface.player_ripples[PLAYER_RIPPLE_CAPACITY - 1]; // 保存末槽确认完整顺序不变。

    let emitted = update_player_ripples_with_takeoff(
        &mut water_surface,              // 调用与正式PostUpdate相同的纯接触逻辑。
        Some(Vec3::new(0.0, -2.0, 0.0)), // 玩家脚底穿过幕布投影也不能授权接触。
        1,                               // 新Tnua起跳序号不能绕过瀑布禁用门。
        1.0,                             // 一秒足以明显改变普通水面的全部波源年龄。
    );

    assert!(emitted.is_none()); // 瀑布不能发射任何玩家波源。
    assert_eq!(water_surface.player_ripples.len(), PLAYER_RIPPLE_CAPACITY); // 满八槽既不能扩容也不能淘汰。
    assert_eq!(
        water_surface.player_ripples[0].center_local_xz,
        first_before.center_local_xz
    ); // 最旧槽身份必须保留。
    assert_eq!(water_surface.player_ripples[0].age, first_before.age); // 禁用瀑布甚至不应老化借来的玩家槽。
    assert_eq!(
        water_surface.player_ripples[PLAYER_RIPPLE_CAPACITY - 1].center_local_xz,
        last_before.center_local_xz
    ); // 末槽顺序同样保持不变。
}

#[test] // 岸边 UV 必须把背景波与互动波共同压到零。
fn bank_fade_keeps_both_water_edges_fixed() {
    // 南北两岸分别对应横向 V 的零和一，河心对应二分之一。
    assert_eq!(calculate_bank_fade(&[0.0, 0.0]), 0.0); // 南岸埋边完全固定。
    assert_eq!(calculate_bank_fade(&[0.0, 1.0]), 0.0); // 北岸埋边完全固定。
    assert_eq!(calculate_bank_fade(&[0.0, 0.5]), 1.0); // 河心允许完整波高。
} // 结束岸边衰减测试。

#[test] // GPU背景波参考公式必须继续让两岸严格静止且河心保持缓波。
fn background_wave_reference_keeps_bank_fixed_and_moves_river_center() {
    let rest_position = [12.0, -1.2, -3.0]; // 使用非零局部XZ避免两组正弦同时落在零相位。
    let elapsed_seconds = 0.7; // 使用非整数时间覆盖两组不同传播速度。
    let bank_height = calculate_wave_height(
        &rest_position,  // 复用同一静止局部顶点。
        &[0.25, 0.0],    // V=0对应一侧河岸埋边。
        elapsed_seconds, // 复用同一背景波时间。
    ); // 计算GPU Shader需要复现的岸边参考高度。
    let center_height = calculate_wave_height(
        &rest_position,  // 保持空间相位完全相同。
        &[0.25, 0.5],    // V=0.5对应河心水带。
        elapsed_seconds, // 保持时间相位完全相同。
    ); // 计算GPU Shader需要复现的河心参考高度。
    assert_eq!(bank_height, 0.0); // 岸边背景波必须严格归零避免与地形裂开。
    assert_ne!(center_height, 0.0); // 河心必须保留两层缓波而不是退化为静态平面。
} // 结束背景缓波CPU参考公式回归。

#[test] // 同一波源必须形成向外传播的三层有限环带，环内与岸边都恢复零高度。
fn player_ripple_creates_multiple_concentric_layers_and_zero_height_at_bank() {
    // 在波前半径足够大时依次采样主波峰、内外波谷和已经恢复平水的环内位置。
    let ripple_age = 2.0; // 两秒时主波峰半径四米，能完整观察有限环带前后边界。
    let front_radius = ripple_age * PLAYER_RIPPLE_SPEED; // 当前环形波包主波峰位于传播速度乘以波源年龄的半径。
    let ripple = PlayerWaterRipple {
        // 使用运行时首次入水波的标准参数。
        center_local_xz: Vec2::ZERO, // 波源中心与测试顶点采用同一局部坐标系。
        age: ripple_age,             // 波源已离开脚下并进入可观察的中后段传播阶段。
        amplitude: PLAYER_RIPPLE_ENTRY_AMPLITUDE, // 使用标准入水波高度。
    }; // 完成测试波源定义。
    let main_crest_sample = calculate_player_ripple_sample(
        &[front_radius, -1.2, 0.0], // 测试顶点位于当前向外传播的环带主波峰中心。
        &[0.5, 0.5],                // 河心横向 UV 不衰减波高。
        &[ripple],                  // 仅叠加当前一圈测试波源。
    ); // 计算河心当前主波峰的几何高度和颜色信号。
    let inner_trough_sample = calculate_player_ripple_sample(
        &[
            front_radius - PLAYER_RIPPLE_LAYER_WAVELENGTH * 0.5, // 半个波长后的第一层内圈应成为负波谷。
            -1.2, // 静止水位只用作坐标占位而不影响径向距离。
            0.0,  // 只沿局部 X 方向移动采样点。
        ], // 完成波谷采样顶点坐标。
        &[0.5, 0.5], // 河心保留完整的负高度。
        &[ripple],   // 复用同一波源排除年龄和幅度差异。
    ); // 计算与主波峰相邻的内侧负波谷采样。
    let outer_trough_height = calculate_player_ripple_height(
        &[
            front_radius + PLAYER_RIPPLE_LAYER_WAVELENGTH * 0.5, // 主波峰外侧半个波长同样应形成负波谷。
            -1.2, // 静止水位只用作坐标占位而不影响径向距离。
            0.0,  // 只沿局部 X 方向移动采样点。
        ], // 完成外侧波谷采样顶点坐标。
        &[0.5, 0.5], // 河心保留完整的负高度。
        &[ripple],   // 复用同一波源排除年龄和幅度差异。
    ); // 计算主波峰外侧的第三层负波谷高度。
    let calm_inner_height = calculate_player_ripple_height(
        &[
            front_radius - PLAYER_RIPPLE_BAND_HALF_WIDTH - 0.1, // 越过有限环带内边界后必须完全恢复平水。
            -1.2, // 静止水位只用作坐标占位而不影响径向距离。
            0.0,  // 只沿局部 X 方向移动采样点。
        ], // 完成环内平水采样顶点坐标。
        &[0.5, 0.5], // 河心不应用岸边衰减。
        &[ripple],   // 复用同一波源排除年龄和幅度差异。
    ); // 计算波前已经通过后的环内静水高度。
    let bank_height = calculate_player_ripple_height(
        &[front_radius, -1.2, 0.0], // 使用同一个最外侧主波峰采样位置。
        &[0.5, 0.0],                // 岸边横向 UV 必须把波高压到零。
        &[ripple],                  // 复用同一波源排除年龄和幅度差异。
    ); // 计算岸边主波峰位置的高度。
    assert!(main_crest_sample.height > 0.0); // 中间主波峰必须高于静止水位并继续向外传播。
    assert!(inner_trough_sample.height < 0.0); // 内侧第一层必须低于静止水位，形成清楚的峰谷层次。
    assert!(outer_trough_height < 0.0); // 外侧第三层同样必须低于静止水位，构成完整三层环带。
    assert!(inner_trough_sample.visual_signal < 0.0); // 负波谷必须同时提供可压暗的同心颜色信号。
    assert_eq!(calm_inner_height, 0.0); // 波包通过后的环内区域必须恢复平水，不能形成实心亮块。
    assert_eq!(bank_height, 0.0); // 岸边顶点保持静止避免裂缝。
} // 结束玩家波峰、波谷与岸边固定测试。

#[test] // 同一个主波峰必须随年龄向外移动，而不是固定在玩家脚下上下闪动。
fn player_ripple_front_moves_outward_over_time() {
    // 选择相隔一点五秒的年龄，使后期采样点在早期有限环带之外。
    let early_age = 0.5; // 早期波前半径为一点二米。
    let late_age = 2.0; // 后期波前半径为四米，明显离开早期环带。
    let late_front_radius = late_age * PLAYER_RIPPLE_SPEED; // 计算后期主波峰的目标采样半径。
    let early_ripple = PlayerWaterRipple {
        // 构造尚未传播到后期采样点的早期波源。
        center_local_xz: Vec2::ZERO, // 波源固定在原点，只有波前半径随年龄变化。
        age: early_age,              // 使用较早年龄验证前方仍为静水。
        amplitude: PLAYER_RIPPLE_JUMP_AMPLITUDE, // 使用跳跃强度让传播差异足够明确。
    }; // 完成早期测试波源。
    let late_ripple = PlayerWaterRipple {
        // 构造已经传播到目标采样点的后期波源。
        center_local_xz: Vec2::ZERO, // 保持同一波源中心排除空间差异。
        age: late_age,               // 使用较晚年龄让主波峰到达目标半径。
        amplitude: PLAYER_RIPPLE_JUMP_AMPLITUDE, // 保持与早期波源相同的初始强度。
    }; // 完成后期测试波源。
    let before_arrival_height = calculate_player_ripple_height(
        &[late_front_radius, -1.2, 0.0], // 固定采样后期主波峰将到达的位置。
        &[0.5, 0.5],                     // 河心位置不应用岸边衰减。
        &[early_ripple],                 // 早期波前还没有到达该点。
    ); // 计算波前到达前的静水高度。
    let on_arrival_height = calculate_player_ripple_height(
        &[late_front_radius, -1.2, 0.0], // 复用同一个世界采样点。
        &[0.5, 0.5],                     // 河心位置不应用岸边衰减。
        &[late_ripple],                  // 后期主波峰恰好到达该点。
    ); // 计算波前到达时的正波峰高度。
    assert_eq!(before_arrival_height, 0.0); // 波前之外必须保持静水，不能提前整片发亮。
    assert!(on_arrival_height > 0.0); // 随年龄增长的主波峰必须真实移动到更远半径。
} // 结束波前向外传播测试。

#[test] // 玩家进入水中先发一圈，移动一个步距再发一圈，站定不继续发波。
fn water_contact_emits_entry_and_distance_based_step_ripples() {
    // 让脚底位于静止水位下二十厘米，模拟刚进入浅水区。
    let mut water_surface = test_water_surface(); // 构造空波源池和未接触状态。
    let in_water_foot_y = water_surface.rest_water_level - 0.2; // 让已转换的局部脚底位于水位下二十厘米。
    let entry_ripple = update_player_ripples(
        &mut water_surface,                         // 更新测试水面状态。
        Some(Vec3::new(0.0, in_water_foot_y, 0.0)), // 首次检测到玩家处于浅水中。
        1.0 / 60.0,                                 // 使用常见六十帧帧长。
    ); // 触发首次入水波。
    assert!(entry_ripple.is_some()); // 首次入水必须向真实几何波源池写入一个扰动。
    assert_eq!(water_surface.player_ripples.len(), 1); // 首次入水只生成一圈。

    let step_ripple = update_player_ripples(
        &mut water_surface, // 延续同一水面接触状态。
        Some(Vec3::new(
            PLAYER_RIPPLE_SPACING + 0.1, // 水平移动略超过一个发波步距。
            in_water_foot_y,             // 保持相同浅水深度。
            0.0,                         // 只沿水面局部 X 方向移动。
        )), // 完成第二帧玩家局部位置。
        1.0 / 60.0,         // 使用相同帧长估算移动速度。
    ); // 触发一圈按步距生成的脚步波。
    assert!(step_ripple.is_some()); // 跨过固定步距必须返回一个新的几何脚步波源。
    assert_eq!(water_surface.player_ripples.len(), 2); // 入水圈和脚步圈应同时处于寿命内。

    let standing_ripple = update_player_ripples(
        &mut water_surface, // 继续推进同一水面状态。
        Some(Vec3::new(
            PLAYER_RIPPLE_SPACING + 0.1, // 玩家保持上一帧水平位置不动。
            in_water_foot_y,             // 玩家仍位于浅水深度。
            0.0,                         // 保持局部 Z 不变。
        )), // 完成站定玩家位置。
        1.0 / 60.0,         // 推进一帧让已有波纹继续扩散。
    ); // 站定时只更新波源年龄而不发新圈。
    assert!(standing_ripple.is_none()); // 站定且不跳跃时不能向水面追加新几何波源。
    assert_eq!(water_surface.player_ripples.len(), 2); // 站定不会持续刷出波纹。
} // 结束入水和按距离发波测试。

mod jump; // 起跳、落水、连续跳、低帧率接管与传送隔离回归按职责拆到子模块。

#[test] // 河外玩家即使脚底高度低于水位也不能触发任何互动波。
fn footprint_rejects_low_player_position_outside_banks() {
    // 使用与入水测试相同深度，但把局部 Z 放到测试岸线之外。
    let mut water_surface = test_water_surface(); // 构造空波源池和有效矩形水面轮廓。
    let below_water_foot_y = water_surface.rest_water_level - 0.2; // 脚底位于水位下二十厘米。
    let outside_ripple = update_player_ripples(
        &mut water_surface,                            // 更新测试水面状态。
        Some(Vec3::new(0.0, below_water_foot_y, 6.0)), // 局部 Z=6 超出最大岸线 Z=5。
        1.0 / 60.0,                                    // 推进一帧执行完整接触判断。
    ); // 尝试在河外低处触发波纹。
    assert!(outside_ripple.is_none()); // 河外位置不能生成任何水面几何波源。
    assert!(water_surface.player_ripples.is_empty()); // 水平轮廓门控必须拒绝河外波源。
    assert!(!water_surface.player_was_in_water); // 河外位置不能进入持续水面接触状态。
} // 结束河外低处误触发回归测试。
