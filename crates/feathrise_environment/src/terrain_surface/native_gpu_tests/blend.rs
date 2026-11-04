//! 正式索引PBR混合数学的GPU查询：高度重加权、覆盖回退、RGBA/PBR与法线输出。
pub(super) const QUERY: &str = r#"
@group(0) @binding(118) var<storage,read_write> native_results: array<f32>;
fn write_native_blend(offset:u32,value:IndexedBlendedSurface) {
    native_results[offset]=value.base_color.x; native_results[offset+1u]=value.base_color.y;
    native_results[offset+2u]=value.base_color.z; native_results[offset+3u]=value.base_color.w;
    native_results[offset+4u]=value.pbr.x; native_results[offset+5u]=value.pbr.y; native_results[offset+6u]=value.pbr.z;
    native_results[offset+7u]=value.normal.x; native_results[offset+8u]=value.normal.y; native_results[offset+9u]=value.normal.z;
}
fn verify_native_blends() {
    let colors=mat4x4<f32>(vec4<f32>(1.0,0.0,0.0,1.0),vec4<f32>(0.0,1.0,0.0,1.0),
        vec4<f32>(0.0,0.0,1.0,1.0),vec4<f32>(1.0));
    let heights=mat4x4<f32>(vec4<f32>(0.2,0.3,0.4,0.5),vec4<f32>(0.4,0.5,0.6,1.0),
        vec4<f32>(0.6,0.7,0.8,0.0),vec4<f32>(0.8,0.9,1.0,0.75));
    let normals=mat4x3<f32>(vec3<f32>(1.0,0.0,0.0),vec3<f32>(0.0,1.0,0.0),
        vec3<f32>(0.0,0.0,1.0),vec3<f32>(0.0,-1.0,0.0));
    write_native_blend(136u,blend_indexed_samples(vec4<f32>(0.4,0.3,0.2,0.1),colors,heights,normals,
        vec4<f32>(0.0,0.5,1.0,1.0),vec3<f32>(0.0,1.0,0.0)));
    // 四槽高度贡献都被压为零时保留原0.5覆盖量；相反单位法线抵消时使用几何法线。
    let zero_heights=mat4x4<f32>(vec4<f32>(0.2,0.3,0.4,0.0),vec4<f32>(0.4,0.5,0.6,0.0),
        vec4<f32>(0.6,0.7,0.8,0.0),vec4<f32>(0.8,0.9,1.0,0.0));
    let opposite_normals=mat4x3<f32>(vec3<f32>(1.0,0.0,0.0),vec3<f32>(-1.0,0.0,0.0),
        vec3<f32>(-1.0,0.0,0.0),vec3<f32>(1.0,0.0,0.0));
    write_native_blend(146u,blend_indexed_samples(vec4<f32>(0.2,0.15,0.1,0.05),colors,zero_heights,
        opposite_normals,vec4<f32>(2.0),vec3<f32>(0.0,1.0,0.0)));
}
"#;

pub(super) fn assert_queries(values: &[f32]) {
    // 预期来自两组具体线性组合；不维护CPU版材质采样或混合算法。
    for (case, expected) in [
        [0.55, 0.6, 0.15, 1., 0.38, 0.48, 0.58, 0.8, 0.6, 0.],
        [0.25, 0.2, 0.15, 0.5, 0.2, 0.25, 0.3, 0., 1., 0.],
    ]
    .into_iter()
    .enumerate()
    {
        for (channel, expected) in expected.into_iter().enumerate() {
            assert!(
                (values[136 + case * 10 + channel] - expected).abs() < 0.00001,
                "索引混合案例{case}的颜色/PBR/法线通道{channel}错误"
            );
        }
    }
}
