//! 完整水片元接线回归：保留生产水/雨/天气库，只用有限桩替代Bevy PBR。

const WATER_SHADER_SOURCE: &str = include_str!("../../../crates/feathrise_scene/src/shared/large_scene/water/shaders/water.wesl");
const WATER_FRAGMENT_SHADER_SOURCE: &str = include_str!("../../../crates/feathrise_scene/src/shared/large_scene/water/shaders/fragment.wesl");

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
struct Material { base_color: vec4<f32>, perceptual_roughness: f32, flags: u32, alpha_cutoff: f32, reflectance: vec3<f32>, metallic: f32, } // 保持生产PBR的作者反射率类型。
struct PbrInput { N: vec3<f32>, material: Material, specular_occlusion: f32, }
struct Globals { time: f32, }
struct View { world_position: vec3<f32>, clip_from_world: mat4x4<f32>, view_from_world: mat4x4<f32>, view_from_clip: mat4x4<f32>, clip_from_view: mat4x4<f32>, main_pass_viewport: vec4<f32>, }
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<uniform> view: View;
@if(VIEW_TRANSMISSION_TEXTURE) @group(0) @binding(24) var view_transmission_texture: texture_2d<f32>;
@if(VIEW_TRANSMISSION_TEXTURE) @group(0) @binding(25) var view_transmission_sampler: sampler;
@if(DEPTH_PREPASS && MULTISAMPLED) @group(0) @binding(20) var depth_prepass_texture: texture_depth_multisampled_2d;
@if(DEPTH_PREPASS && !MULTISAMPLED) @group(0) @binding(20) var depth_prepass_texture: texture_depth_2d;
@if(DEPTH_PREPASS) fn prepass_depth(pixel: vec4<f32>, sample_index: u32) -> f32 {
    @if(MULTISAMPLED) return textureLoad(depth_prepass_texture, vec2<i32>(pixel.xy), i32(sample_index));
    @else return textureLoad(depth_prepass_texture, vec2<i32>(pixel.xy), 0);
}
const STANDARD_MATERIAL_FLAGS_UNLIT_BIT: u32 = 1u;
fn pbr_input_from_standard_material(v: VertexOutput, front: bool) -> PbrInput {
    return PbrInput(v.world_normal, Material(v.color, 0.2, 0u, 0.5, vec3<f32>(0.5), 0.0), 1.0); // 非金属默认预设，不代替生产反射算法。
}
fn alpha_discard(flags: u32, alpha_cutoff: f32, c: vec4<f32>) -> vec4<f32> { return c; }
fn apply_pbr_lighting(p: PbrInput) -> vec4<f32> { return p.material.base_color; }
fn main_pass_post_lighting_processing(p: PbrInput, c: vec4<f32>) -> vec4<f32> { return c; }
"#; // 桩不模拟光照正确性，仅让Naga实际验证完整生产fragment类型与导数一致性。

use super::terrain_support::preprocess;

#[test] // 湖面、无UV网格、UV0整网格与UV1样条的完整生产片元都必须可编译。
fn rain_water_fragment_variants_are_valid_wgsl() {
    // 无窗口、无GPU，真实验证全部新雨纹接线。
    let rain = include_str!(
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/rain.wesl"
    ); // 地面与水共用同一雨纹库。
    let weather = include_str!(
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/surface_weather.wesl"
    ); // 真实64字节storage布局也纳入校验。
    let wet = include_str!(
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/wet_surface.wesl"
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
        &["DEPTH_PREPASS"], // 辅助相机有深度但未启用屏幕传输时仍须回退。
        &["DEPTH_PREPASS", "VIEW_TRANSMISSION_TEXTURE"],
        &["DEPTH_PREPASS", "VIEW_TRANSMISSION_TEXTURE", "VERTEX_COLORS"],
        &["DEPTH_PREPASS", "VIEW_TRANSMISSION_TEXTURE", "VERTEX_UVS_A"],
        &[
            "DEPTH_PREPASS", "VIEW_TRANSMISSION_TEXTURE",
            "VERTEX_UVS_A",
            "VERTEX_UVS_B",
            "VERTEX_TANGENTS",
            "VERTEX_COLORS",
        ],
        &["DEPTH_PREPASS", "VIEW_TRANSMISSION_TEXTURE", "TONEMAP_IN_SHADER"],
        &[
            "DEPTH_PREPASS", "VIEW_TRANSMISSION_TEXTURE",
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
        // 固定编译变体，不创建运行时资源。
        let clipmap = if defines.contains(&"CLIPMAP_WATER") {
            let stitches = feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false)
                .replace("#{MATERIAL_BIND_GROUP}", "2");
            let height = preprocess(include_str!("../../../crates/feathrise_scene/src/scenes/dynamic_scene/clipmap/shaders/height.wesl"), defines);
            let water = preprocess(
                include_str!("../../../crates/feathrise_scene/src/scenes/dynamic_scene/water/shaders/clipmap_water.wesl"),
                defines,
            )
            .replace("clipmap_height::", "");
            format!("{stitches}\n{height}\n{water}")
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
            preprocess(include_str!("../../../crates/feathrise_scene/src/shared/large_scene/water/shaders/reflection.wesl"), defines),
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
        "../../../crates/feathrise_environment/src/terrain_surface/shaders/rain.wesl"
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
