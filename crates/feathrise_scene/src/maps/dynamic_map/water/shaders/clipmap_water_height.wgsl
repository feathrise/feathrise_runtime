// 雨水高度图的Clipmap静水代理：同一波形、R8、真实水深和缺页裁切后编码世界Y。
#import bevy_pbr::forward_io::VertexOutput
#import feathrise_scene::clipmap_water

@group(#{MATERIAL_BIND_GROUP}) @binding(127)
var<uniform> source_coverage: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if source_coverage.x <= 0.0 {
        discard;
    }
#ifdef VERTEX_UVS_A
#ifdef VERTEX_UVS_B
    let coverage = clipmap_water::surface_coverage(in.uv, in.uv_b)
        * source_coverage.x;
    if coverage <= 0.001 {
        discard;
    }
#else
    discard;
#endif
#else
    discard;
#endif
    let encoded = in.world_position.y / 16.0;
    return vec4<f32>(floor(encoded), fract(encoded), coverage, 1.0);
}
