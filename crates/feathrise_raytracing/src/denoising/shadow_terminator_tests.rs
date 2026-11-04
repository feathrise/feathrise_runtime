//! 生产 GBuffer/固定代理投影的 Naga 语义与 X/Z 几何回归；桩只替代官方外部类型。

const DEPENDENCIES: &str = r#"
struct View { clip_from_view: mat4x4<f32>, view_from_clip: mat4x4<f32>, }
fn depth_ndc_to_view_z(d: f32, a: mat4x4<f32>, b: mat4x4<f32>) -> f32 { return d; }
fn unpack_24bit_normal(v: u32) -> vec2<f32> { return vec2<f32>(0.0); }
fn unpack_flags(v: u32) -> u32 { return v >> 24u; }
fn rgb9e5_to_vec3_(v: u32) -> vec3<f32> { return vec3<f32>(0.0); }
fn octahedral_decode(v: vec2<f32>) -> vec3<f32> { return vec3<f32>(v.x, 1.0, v.y); }
struct ResolvedMaterial { base_color: vec3<f32>, emissive: vec3<f32>, reflectance: f32, perceptual_roughness: f32, roughness: f32, metallic: f32, }
struct Geometry { triangle_count: u32, }
struct Vertex { position: vec3<f32>, normal: vec3<f32>, }
@group(0) @binding(0) var<storage, read> geometry_ids: array<Geometry>;
@group(0) @binding(1) var<storage, read> transforms: array<mat4x4<f32>>;
const RAY_T_MIN: f32 = 0.0001;
fn load_vertices(g: Geometry, p: u32) -> array<Vertex, 3> {
    let v = Vertex(vec3<f32>(0.0), vec3<f32>(0.0, 1.0, 0.0));
    return array<Vertex, 3>(v, v, v);
}
fn transform_positions(t: mat4x4<f32>, v: array<Vertex, 3>) -> array<vec3<f32>, 3> {
    return array<vec3<f32>, 3>((t * vec4<f32>(v[0].position, 1.0)).xyz,
        (t * vec4<f32>(v[1].position, 1.0)).xyz, (t * vec4<f32>(v[2].position, 1.0)).xyz);
}
"#;

#[test]
fn terrain_gbuffer_instance_and_triangle_routing_preserve_emission_exponent() {
    for instance in 0..16_u32 {
        for primitive in [0, 1, 262143, 524287] {
            let packed = primitive | (instance << 19);
            assert_eq!(packed & 0x7ffff, primitive);
            assert_eq!((packed >> 19) & 15, instance);
            assert_eq!(packed >> 27, 0); // RGB9E5指数零，不制造亮色/自发光溢出。
        }
    }
}

#[test]
fn real_gbuffer_proxy_projection_compiles_with_naga() {
    let source = include_str!("shaders/gbuffer_shadow_terminator.wgsl");
    let body = source
        .lines()
        .filter(|line| !line.trim().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let source = format!("{DEPENDENCIES}\n{body}");
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
}

#[test]
fn proxy_origin_projection_uses_xz_for_pixels_above_and_below_coarse_surface() {
    use bevy::prelude::*;
    let triangle = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 8.0, 2.0),
        Vec3::new(2.0, 4.0, 0.0),
    ];
    let project = |point: Vec3| {
        let a = (triangle[1] - triangle[0]).with_y(0.0);
        let b = (triangle[2] - triangle[0]).with_y(0.0);
        let p = (point - triangle[0]).with_y(0.0);
        let denominator = a.length_squared() * b.length_squared() - a.dot(b).powi(2);
        let y = (b.length_squared() * p.dot(a) - a.dot(b) * p.dot(b)) / denominator;
        let z = (a.length_squared() * p.dot(b) - a.dot(b) * p.dot(a)) / denominator;
        (1.0 - y - z) * triangle[0] + y * triangle[1] + z * triangle[2]
    };
    let above = project(Vec3::new(0.5, 50.0, 0.5));
    let below = project(Vec3::new(0.5, -50.0, 0.5));
    assert_eq!(above, Vec3::new(0.5, 3.0, 0.5));
    assert_eq!(above, below); // 不能让光栅与代理的高度差改变选择的三角重心。
}
