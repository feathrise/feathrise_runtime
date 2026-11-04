// Clipmap静水主通道：环带提供XZ拓扑，在源图坐标求水位和波形后返回世界。
#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
}
#import bevy_pbr::mesh_view_bindings::globals
#import feathrise_scene::clipmap_height
#import feathrise_scene::clipmap_water
#import feathrise_scene::shared::large_map::water::water::{
    dynamic_water_uniform,
    sample_dynamic_water,
}

@vertex
fn vertex(input: Vertex) -> VertexOutput {
    let transform = mesh_functions::get_world_from_local(input.instance_index);
    let flat = mesh_functions::mesh_position_local_to_world(transform, vec4<f32>(input.position, 1.0));
#ifdef VERTEX_UVS_B
    let lod = input.uv_b;
#else
    let lod = vec2<f32>(0.0);
#endif
    var world = clipmap_height::stable_world_xz(clipmap_height::source_position(flat.xyz).xz, clipmap_height::source_position(transform[3].xyz).xz, lod);
    var terrain_height = clipmap_height::height_at(world, lod);
    var known = clipmap_water::known_height(world, lod);
#ifdef VERTEX_UVS_B
    if clipmap_water::clipmap_water_params.surface.w>0.5 { world=clipmap_height::source_position(flat.xyz).xz;terrain_height=input.uv_b.x;known=input.uv_b.y; }
#endif
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
    return VertexOutput(
        position_world_to_clip(world_position.xyz),
        world_position,
        clipmap_height::world_normal(shore_normal),
#ifdef VERTEX_UVS_A
        uv,
#endif
#ifdef VERTEX_UVS_B
        vec2<f32>(terrain_height, known), // 片元按真实三角面内插地形高度并拒绝缺页。
#endif
#ifdef VERTEX_TANGENTS
        vec4<f32>(clipmap_height::world_normal(vec3<f32>(1.0, 0.0, 0.0)), 1.0),
#endif
#ifdef VERTEX_COLORS
        vec4<f32>(1.0),
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
        input.instance_index,
#endif
#ifdef VISIBILITY_RANGE_DITHER
        mesh_functions::get_visibility_range_dither_level(input.instance_index, transform[3]),
#endif
    ); // 直接构造阶段输出，避免Naga把接口布局装饰带入Vulkan Function变量。
}
