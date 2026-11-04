//! 测试专用的生产地表源码、宿主类型桩与预处理；单包和仓库集成验证共用，不进入生产 API。

pub(super) const TERRAIN: &str = include_str!("shaders/terrain_surface.wgsl"); // 单地形固定层与索引层都读取正式生产正文。
pub(super) const WEATHER: &str = include_str!("shaders/surface_weather.wgsl"); // 正式64字节共享天气ABI。
pub(super) const RAIN: &str = include_str!("shaders/rain.wgsl"); // 正式固定九候选雨纹算法。
pub(super) const WET: &str = include_str!("shaders/wet_surface.wgsl"); // 正式湿润和积水覆盖算法。

// 桩只替代Bevy PBR类型和输出，不重写地表采样、湿润、雨纹或Deferred元数据正文。
pub(super) const PBR_STUB: &str = r#"
struct VertexOutput { // 片元共同输入，世界法线与位置是实际生产使用字段。
    @builtin(position) position: vec4<f32>, // 标准光栅坐标。
    @location(0) world_position: vec4<f32>, // 世界空间米制坐标。
    @location(1) world_normal: vec3<f32>, // 几何法线。
#ifdef VERTEX_COLORS
    @location(2) color:vec4<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv:vec2<f32>,
    @location(4) uv_b:vec2<f32>,
#endif
}
struct Material { base_color: vec4<f32>, perceptual_roughness: f32, metallic: f32, reflectance: vec3<f32>, } // 本地表实际修改的PBR字段，与Bevy0.19.1的RGB反射率一致。
struct PbrInput { N: vec3<f32>, world_normal: vec3<f32>, material: Material, diffuse_occlusion: vec3<f32>, } // 保留细法线和几何法线区分。
struct View { world_position: vec3<f32>, } // 相机只参与雨纹距离分档。
@group(0) @binding(0) var<uniform> view: View; // 与生产材质组不冲突。
fn pbr_input_from_standard_material(v: VertexOutput, front: bool) -> PbrInput { // 光照建立使用简单有限初值。
    return PbrInput(v.world_normal, v.world_normal, Material(vec4<f32>(1.0), 0.5, 0.0, vec3<f32>(0.5)), vec3<f32>(1.0)); // 其余所有材料变化由生产fragment计算。
}
#ifdef PREPASS_PIPELINE
struct FragmentOutput { @location(0) deferred: vec4<u32>, } // 标准GBuffer使用整数打包字段。
fn deferred_output(v: VertexOutput, p: PbrInput) -> FragmentOutput { return FragmentOutput(vec4<u32>(0u)); } // 元数据写入仍执行真实生产代码。
fn unpack_flags(v: u32) -> u32 { return v; } // 不验证Bevy已有位打包实现。
fn pack_24bit_normal_and_flags(n: vec2<f32>, flags: u32) -> u32 { return flags; } // 只约束真实调用的参数类型。
fn octahedral_encode(n: vec3<f32>) -> vec2<f32> { return n.xy; } // Bevy标准法线编码是测试边界。
#else
struct FragmentOutput { @location(0) color: vec4<f32>, } // 普通前向颜色输出。
fn apply_pbr_lighting(p: PbrInput) -> vec4<f32> { return p.material.base_color; } // 仅替代外部光照，不吞掉生产PBR字段修改。
fn main_pass_post_lighting_processing(p: PbrInput, c: vec4<f32>) -> vec4<f32> { return c; } // 雾和曝光由Bevy负责。
#endif
"#; // 真实渲染视觉仍需实机验证，此桩保证生产片元类型与控制流正确。

pub(super) fn preprocess(source: &str, defines: &[&str]) -> String {
    // 只展开现有Bevy导入和条件指令，正文原样保留。
    let mut result = String::new(); // 小型测试源码拼接不进入运行时。
    let mut active = vec![true]; // 跟踪父级条件是否激活。
    let mut importing = false; // 生产多行#import在此由拼接库替代。
    for line in source.lines() {
        // 测试单次线性读取全部生产文本。
        let trimmed = line.trim(); // 仅指令识别忽略缩进。
        if importing {
            importing = !trimmed.ends_with('}');
            continue;
        } // 跳过多行导入剩余内容。
        if trimmed.starts_with("#import ") {
            importing = trimmed.ends_with('{');
            continue;
        } // 单行导入直接跳过。
        if trimmed.starts_with("#define_import_path") {
            continue;
        } // Naga不认识Bevy库路径声明。
        if let Some(symbol) = trimmed.strip_prefix("#ifdef ") {
            // 按目标渲染通道选择正式分支。
            active.push(*active.last().unwrap() && defines.contains(&symbol)); // 嵌套条件不能越过父级禁用。
            continue; // 条件指令不进入WGSL正文。
        } // 完成条件压栈。
        if trimmed == "#else" {
            // 使用父级与当前条件构造互斥分支。
            let current = active.pop().expect("else必须有if"); // 读取当前分支状态。
            active.push(*active.last().unwrap() && !current); // 父级禁用时else同样禁用。
            continue; // 排除Bevy预处理指令。
        } // 完成else处理。
        if trimmed == "#endif" {
            active.pop();
            continue;
        } // 恢复父级条件。
        if *active.last().unwrap() {
            result.push_str(line);
            result.push('\n');
        } // 只复制生效的生产正文。
    } // 完成受控测试预处理。
    result.replace("#{MATERIAL_BIND_GROUP}", "2") // 材质组固定二，生产绑定编号保持原值。
} // 结束Bevy预处理替换。

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
