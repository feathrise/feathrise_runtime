//! 完整水片元接线回归：保留生产水/雨/天气库，只用有限桩替代Bevy PBR。

const WATER_SHADER_SOURCE: &str = include_str!("../../../crates/feathrise_scene/src/shared/large_map/water/shaders/water.wgsl");
const WATER_FRAGMENT_SHADER_SOURCE: &str = include_str!("../../../crates/feathrise_scene/src/shared/large_map/water/shaders/fragment.wgsl");

// 测试桩只表达标准PBR输入输出类型；真实雨纹、岸线、法线和片元分支全部来自生产源码。
const PBR_STUB: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(4) world_tangent: vec4<f32>,
    @location(5) color: vec4<f32>,
}
struct FragmentOutput { @location(0) color: vec4<f32>, }
struct Material { base_color: vec4<f32>, perceptual_roughness: f32, flags: u32, reflectance: vec3<f32>, metallic: f32, } // 保持生产PBR的作者反射率类型。
struct PbrInput { N: vec3<f32>, material: Material, specular_occlusion: f32, }
struct Globals { time: f32, }
struct View { world_position: vec3<f32>, clip_from_world: mat4x4<f32>, view_from_world: mat4x4<f32>, view_from_clip: mat4x4<f32>, clip_from_view: mat4x4<f32>, main_pass_viewport: vec4<f32>, }
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(24) var view_transmission_texture: texture_2d<f32>;
@group(0) @binding(25) var view_transmission_sampler: sampler;
#ifdef DEPTH_PREPASS
#ifdef MULTISAMPLED
@group(0) @binding(20) var depth_prepass_texture: texture_depth_multisampled_2d; // Bevy标准MSAA深度绑定类型。
#else
@group(0) @binding(20) var depth_prepass_texture: texture_depth_2d; // Bevy单采样深度绑定类型。
#endif
fn prepass_depth(pixel: vec4<f32>, sample_index: u32) -> f32 { // 按Bevy0.19 prepass_utils核验实际textureLoad签名。
#ifdef MULTISAMPLED
    return textureLoad(depth_prepass_texture, vec2<i32>(pixel.xy), i32(sample_index)); // 多采样的第三参数是样本号。
#else
    return textureLoad(depth_prepass_texture, vec2<i32>(pixel.xy), 0); // 单采样的第三参数是mip。
#endif
}
#endif
const STANDARD_MATERIAL_FLAGS_UNLIT_BIT: u32 = 1u;
fn pbr_input_from_standard_material(v: VertexOutput, front: bool) -> PbrInput {
    return PbrInput(v.world_normal, Material(v.color, 0.2, 0u, vec3<f32>(0.5), 0.0), 1.0); // 非金属默认预设，不代替生产反射算法。
}
fn alpha_discard(m: Material, c: vec4<f32>) -> vec4<f32> { return c; }
fn apply_pbr_lighting(p: PbrInput) -> vec4<f32> { return p.material.base_color; }
fn main_pass_post_lighting_processing(p: PbrInput, c: vec4<f32>) -> vec4<f32> { return c; }
"#; // 桩不模拟光照正确性，仅让Naga实际验证完整生产fragment类型与导数一致性。

fn preprocess(source: &str, defines: &[&str]) -> String {
    // 仅处理本水片元现有的#import/#ifdef/#endif。
    let mut output = String::new(); // 测试允许同步拼接小型Shader源码。
    let mut active = vec![true]; // 条件栈保持嵌套UV与切线分支一致。
    let mut importing = false; // 多行Bevy导入由实际生产库文本代替。
    for line in source.lines() {
        // 每一生产行最多处理一次。
        let trimmed = line.trim(); // 预处理不改变正文内部表达式。
        if importing {
            // 跳过尚未闭合的多行导入。
            importing = !trimmed.ends_with('}'); // 当前两份生产导入都以右花括号结束。
            continue; // 导入符号已经由真实库和PBR桩提供。
        } // 完成多行导入处理。
        if trimmed.starts_with("#import ") {
            // 单行和多行导入统一识别。
            importing = trimmed.ends_with('{'); // 单行符号导入无需继续跳过。
            continue; // 交给拼接库完成导入替换。
        } // 完成导入首行。
        if trimmed.starts_with("#define_import_path") {
            continue;
        } // 独立Naga不接受Bevy路径指令。
        if let Some(symbol) = trimmed.strip_prefix("#ifdef ") {
            // 按当前网格属性展开真实分支。
            active.push(*active.last().unwrap() && defines.contains(&symbol)); // 外层禁用时内层不能重新启用。
            continue; // 条件指令不是标准WGSL。
        } // 完成条件压栈。
        if let Some(symbol) = trimmed.strip_prefix("#ifndef ") {
            // Clipmap专用分支关闭时仍应验证原GLB/HN路径。
            active.push(*active.last().unwrap() && !defines.contains(&symbol));
            continue;
        } // 完成反向条件压栈。
        if trimmed.starts_with("#else") {
            let previous = active.pop().unwrap();
            active.push(*active.last().unwrap() && !previous);
            continue;
        } // 非HDR回退与SSR主体互斥展开。
        if trimmed.starts_with("#endif") {
            active.pop();
            continue;
        } // 支持共享库条件尾部的说明注释。
        if *active.last().unwrap() {
            output.push_str(line);
            output.push('\n');
        } // 只复制实际启用的完整生产行。
    } // 完成小型测试预处理。
    output.replace("#{MATERIAL_BIND_GROUP}", "2") // 固定测试材质组，不改绑定编号或布局。
} // 结束生产源码展开。

#[test] // 湖面、无UV网格、UV0整网格与UV1样条的完整生产片元都必须可编译。
fn rain_water_fragment_variants_are_valid_wgsl() {
    // 无窗口、无GPU，真实验证全部新雨纹接线。
    let rain = include_str!(
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/rain.wgsl"
    ); // 地面与水共用同一雨纹库。
    let weather = include_str!(
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/surface_weather.wgsl"
    ); // 真实64字节storage布局也纳入校验。
    let wet = include_str!(
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/wet_surface.wgsl"
    ); // P4环境反射辅助函数使用同一生产正文。
    let variants: &[&[&str]] = &[
        &[],
        &["VERTEX_COLORS"],
        &["VERTEX_UVS_A"],
        &[
            "VERTEX_UVS_A",
            "VERTEX_UVS_B",
            "VERTEX_TANGENTS",
            "VERTEX_COLORS",
        ],
        &["DEPTH_PREPASS"],
        &["DEPTH_PREPASS", "VERTEX_COLORS"],
        &["DEPTH_PREPASS", "VERTEX_UVS_A"],
        &[
            "DEPTH_PREPASS",
            "VERTEX_UVS_A",
            "VERTEX_UVS_B",
            "VERTEX_TANGENTS",
            "VERTEX_COLORS",
        ],
        &["DEPTH_PREPASS", "TONEMAP_IN_SHADER"],
        &[
            "DEPTH_PREPASS",
            "MULTISAMPLED",
            "VERTEX_UVS_A",
            "VERTEX_UVS_B",
            "VERTEX_TANGENTS",
            "VERTEX_COLORS",
        ],
        &[
            "CLIPMAP_WATER",
            "VERTEX_UVS_A",
            "VERTEX_UVS_B",
            "VERTEX_TANGENTS",
            "VERTEX_COLORS",
        ],
    ]; // 覆盖SSR、无深度、非HDR回退和MSAA深度纹理十类路径。
    for defines in variants {
        // 固定十个编译变体，不创建运行时资源。
        let clipmap = if defines.contains(&"CLIPMAP_WATER") {
            let height = preprocess(&format!("{}\n{}",
                feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false),
                include_str!("../../../crates/feathrise_scene/src/maps/dynamic_map/clipmap/shaders/height.wgsl")),defines);
            let water = preprocess(
                include_str!("../../../crates/feathrise_scene/src/maps/dynamic_map/water/shaders/clipmap_water.wgsl"),
                defines,
            )
            .replace("clipmap_height::", "");
            format!("{height}\n{water}")
        } else {
            String::new()
        }; // Naga测试展平Bevy模块命名空间，正文和绑定仍取正式Shader。
        let source = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            preprocess(PBR_STUB, defines),
            preprocess(WATER_SHADER_SOURCE, defines),
            preprocess(rain, defines),
            format!(
                "{}\n{}",
                feathrise_environment::rain_shelter::RAIN_SHELTER_RAY_WGSL,
                preprocess(weather, defines)
            ),
            preprocess(wet, defines),
            preprocess(include_str!("../../../crates/feathrise_scene/src/shared/large_map/water/shaders/reflection.wgsl"), defines),
            clipmap,
            preprocess(WATER_FRAGMENT_SHADER_SOURCE, defines)
        )
        .replace("clipmap_water::", ""); // 只在测试源中展平正式模块调用。
        let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| {
            panic!(
                "水雨片元{defines:?}语法错误：{}",
                error.emit_to_string(&source)
            )
        }); // 保留完整源码定位。
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("水雨片元{defines:?}语义错误：{error}")); // 导数一致性和entry point也实际验证。
    } // 完成固定变体集合。
} // 结束生产水雨完整片元回归。

#[test] // 风雨纹相位必须同样兼容天气4096秒回绕，不修改原有基础水波时钟。
fn rain_wind_phase_divides_weather_clock_wrap() {
    // 用生产相位契约约束未来速度调整。
    let rain = include_str!(
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/rain.wgsl"
    ); // P4风纹已统一到地表和水面共用库。
    assert!(
        WATER_FRAGMENT_SHADER_SOURCE.contains("let wind_slope = sample_surface_wind_slope(")
    ); // 真正水片元必须消费共享风纹。
    for speed in [0.5_f32, 0.25] {
        // 主副波两个周期都必须整除天气回绕。
        assert_eq!((4096.0_f32 * speed).fract(), 0.0); // 回绕点两层均连续。
        assert!(rain.contains(&format!("fract(elapsed * {speed})"))); // 防止生产相位脱离验证的周期。
    } // 完成固定两层时钟契约。
} // 结束风雨相位回归。
