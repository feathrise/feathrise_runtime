// 雨水高度图的Clipmap静水代理：同一波形、R8、真实水深和缺页裁切后编码世界Y。

import bevy_pbr::render::forward_io::VertexOutput;
import feathrise_scene::maps::dynamic_map::water::shaders::clipmap_water;

@group(constants::MATERIAL_BIND_GROUP) @binding(127)
var<uniform> source_coverage: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if source_coverage.x <= 0.0 {
        discard;
    }
    @if(VERTEX_UVS_A && VERTEX_UVS_B) let coverage = clipmap_water::surface_coverage(in.uv, in.uv_b)
        * source_coverage.x;
    @if(VERTEX_UVS_A && VERTEX_UVS_B) if coverage <= 0.001 {
        discard;
    }
    @if(VERTEX_UVS_A && !VERTEX_UVS_B) discard;
    @if(!VERTEX_UVS_A) discard;
    let encoded = in.world_position.y / 16.0;
    return vec4<f32>(floor(encoded), fract(encoded), coverage, 1.0);
}
