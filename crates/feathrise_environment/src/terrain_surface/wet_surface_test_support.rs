//! 测试专用的生产地表源码、宿主类型桩与预处理；单包和仓库集成验证共用，不进入生产 API。

pub(super) const TERRAIN: &str = include_str!("shaders/terrain_surface.wesl"); // 单地形固定层与索引层都读取正式生产正文。
pub(super) const WEATHER: &str = include_str!("shaders/surface_weather.wesl"); // 正式64字节共享天气ABI。
pub(super) const RAIN: &str = include_str!("shaders/rain.wesl"); // 正式固定九候选雨纹算法。
pub(super) const WET: &str = include_str!("shaders/wet_surface.wesl"); // 正式湿润和积水覆盖算法。

// 桩只替代Bevy PBR类型和输出，不重写地表采样、湿润、雨纹或Deferred元数据正文。
pub(super) const PBR_STUB: &str = r#"
struct VertexOutput { // 片元共同输入，世界法线与位置是实际生产使用字段。
    @builtin(position) position: vec4<f32>, // 标准光栅坐标。
    @location(0) world_position: vec4<f32>, // 世界空间米制坐标。
    @location(1) world_normal: vec3<f32>, // 几何法线。
    @if(VERTEX_COLORS) @location(2) color:vec4<f32>,

    @if(VERTEX_UVS_B) @location(3) uv:vec2<f32>,
    @if(VERTEX_UVS_B) @location(4) uv_b:vec2<f32>,

}
struct Material { base_color: vec4<f32>, perceptual_roughness: f32, metallic: f32, reflectance: vec3<f32>, } // 本地表实际修改的PBR字段，与Bevy0.19.1的RGB反射率一致。
struct PbrInput { N: vec3<f32>, world_normal: vec3<f32>, material: Material, diffuse_occlusion: vec3<f32>, } // 保留细法线和几何法线区分。
struct View { world_position: vec3<f32>, } // 相机只参与雨纹距离分档。
@group(0) @binding(0) var<uniform> view: View; // 与生产材质组不冲突。
fn pbr_input_from_standard_material(v: VertexOutput, front: bool) -> PbrInput { // 光照建立使用简单有限初值。
    return PbrInput(v.world_normal, v.world_normal, Material(vec4<f32>(1.0), 0.5, 0.0, vec3<f32>(0.5)), vec3<f32>(1.0)); // 其余所有材料变化由生产fragment计算。
}
@if(PREPASS_PIPELINE) struct FragmentOutput { @location(0) deferred: vec4<u32>, } // 标准GBuffer使用整数打包字段。
@if(PREPASS_PIPELINE) fn deferred_output(v: VertexOutput, p: PbrInput) -> FragmentOutput { return FragmentOutput(vec4<u32>(0u)); } // 元数据写入仍执行真实生产代码。
@if(PREPASS_PIPELINE) fn unpack_flags(v: u32) -> u32 { return v; } // 不验证Bevy已有位打包实现。
@if(PREPASS_PIPELINE) fn pack_24bit_normal_and_flags(n: vec2<f32>, flags: u32) -> u32 { return flags; } // 只约束真实调用的参数类型。
@if(PREPASS_PIPELINE) fn octahedral_encode(n: vec3<f32>) -> vec2<f32> { return n.xy; } // Bevy标准法线编码是测试边界。
@if(!PREPASS_PIPELINE) struct FragmentOutput { @location(0) color: vec4<f32>, } // 普通前向颜色输出。
@if(!PREPASS_PIPELINE) fn apply_pbr_lighting(p: PbrInput) -> vec4<f32> { return p.material.base_color; } // 仅替代外部光照，不吞掉生产PBR字段修改。
@if(!PREPASS_PIPELINE) fn main_pass_post_lighting_processing(p: PbrInput, c: vec4<f32>) -> vec4<f32> { return c; } // 雾和曝光由Bevy负责。
"#; // 真实渲染视觉仍需实机验证，此桩保证生产片元类型与控制流正确。

pub(super) fn preprocess(source: &str, defines: &[&str]) -> String {
    let mut module = source.parse::<wesl::syntax::TranslationUnit>().expect("生产 WESL 应可解析");
    let mut features = wesl::Features::new();
    for name in defines {
        features.set(name, true);
    }
    wesl::pass::condcomp(&mut module, &features).expect("展开当前地表管线变体");
    module.imports.clear();
    module.to_string().replace("constants::MATERIAL_BIND_GROUP", "2")
}

pub(super) fn validate(source: &str, label: &str) {
    // 解析和验证都使用与Bevy一致的Naga版本。
    let module = wgpu::naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("{label}语法错误：{}", error.emit_to_string(source))); // 保留生产源码精确定位。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{label}语义错误：{error:?}")); // 实际验证类型、绑定、入口及导数一致性。
} // 结束通用生产WGSL验证。
