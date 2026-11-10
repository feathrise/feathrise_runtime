// Clipmap静水主通道：环带提供XZ拓扑，在源图坐标求水位和波形后返回世界。

import bevy_pbr::render::mesh_functions;
import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import bevy_pbr::render::mesh_view_bindings::globals;
import feathrise_scene::scenes::dynamic_scene::clipmap::shaders::height as clipmap_height;
import feathrise_scene::scenes::dynamic_scene::water::shaders::clipmap_water;
import feathrise_scene::shared::large_scene::water::shaders::water::dynamic_water_uniform;
import feathrise_scene::shared::large_scene::water::shaders::water::sample_dynamic_water;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    let input = decompress_vertex(vertex_in, vertex_in.instance_index);
    let transform = mesh_functions::get_world_from_local(input.instance_index);
    let flat = mesh_functions::mesh_position_local_to_world(transform, vec4<f32>(input.position, 1.0));
    @if(VERTEX_UVS_B) let lod = input.uv_b;
    @if(!VERTEX_UVS_B) let lod = vec2<f32>(0.0);
    var world = clipmap_height::stable_world_xz(clipmap_height::source_position(flat.xyz).xz, clipmap_height::source_position(transform[3].xyz).xz, lod);
    var terrain_height = clipmap_height::height_at(world, lod);
    var known = clipmap_water::known_height(world, lod);
    @if(VERTEX_UVS_B) if clipmap_water::clipmap_water_params.surface.w>0.5 { world=clipmap_height::source_position(flat.xyz).xz;terrain_height=input.uv_b.x;known=input.uv_b.y; }
    let uv = clipmap_water::map_uv(world);
    let rest = vec3<f32>(world.x, clipmap_water::water_level(), world.y);
    let shore_fade = clipmap_water::shore_wave_fade(terrain_height, known);
    let sample = sample_dynamic_water(
        false,
        globals.time,
        rest,
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        uv,
        vec2<f32>(-1.0, 0.0),
        1.0,
    );
    // 所有风浪、Gerstner水平位移和玩家波共用同一岸带衰减；保持作者R8边界的原自由波语义。
    let shore_position = mix(rest, sample.local_position, shore_fade);
    let shore_normal = normalize(mix(vec3<f32>(0.0, 1.0, 0.0), sample.local_normal, shore_fade));
    let world_position = clipmap_height::world_position(shore_position); // 只乘源实例框架，不重复应用会吸附移动的环带变换。
    var out: VertexOutput;
    out.position = position_world_to_clip(world_position.xyz);
    out.world_position = world_position;
    out.world_normal = clipmap_height::world_normal(shore_normal);
    @if(VERTEX_UVS_A) out.uv = uv;
    @if(VERTEX_UVS_B) out.uv_b = vec2<f32>(terrain_height, known);
    @if(VERTEX_TANGENTS) out.world_tangent = vec4<f32>(clipmap_height::world_normal(vec3<f32>(1.0, 0.0, 0.0)), 1.0);
    @if(VERTEX_COLORS) out.color = vec4<f32>(1.0);
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = input.instance_index;
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(input.instance_index, transform[3]);
    return out;
}
