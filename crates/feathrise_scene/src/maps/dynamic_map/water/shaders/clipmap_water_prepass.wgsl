// Clipmap静水预通道：当前与历史波形都从稳定源图XZ重建，环带吸附不产生整片运动脉冲。
#import bevy_pbr::{
    mesh_functions,
    prepass_io::{Vertex, VertexOutput},
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
    // 主画面、运动和雨水代理都沿用真实岸坡衰减，避免不同通道出现两条水边。
    let current_position = clipmap_height::world_position(mix(rest, sample.local_position, shore_fade));
    let shore_normal = normalize(mix(vec3<f32>(0.0, 1.0, 0.0), sample.local_normal, shore_fade));
    var clip_position = position_world_to_clip(current_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    let unclipped_depth = clip_position.z;
    clip_position.z = min(clip_position.z, 1.0);
#endif
#ifdef MOTION_VECTOR_PREPASS
    let previous_time = select(
        globals.time - globals.delta_time,
        globals.time + 3600.0 - globals.delta_time,
        globals.time < globals.delta_time,
    );
    let previous = sample_dynamic_water(
        true,
        previous_time,
        rest,
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        uv,
        vec2<f32>(-1.0, 0.0),
        1.0,
    );
    let previous_world_position = clipmap_height::world_position(mix(rest, previous.local_position, shore_fade)); // 不读取上一帧环带实体平移。
#endif
    return VertexOutput(
        clip_position,
#ifdef VERTEX_UVS_A
        uv,
#endif
#ifdef VERTEX_UVS_B
        vec2<f32>(terrain_height, known),
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
        clipmap_height::world_normal(shore_normal),
#ifdef VERTEX_TANGENTS
        vec4<f32>(clipmap_height::world_normal(vec3<f32>(1.0, 0.0, 0.0)), 1.0),
#endif
#endif
        current_position,
#ifdef MOTION_VECTOR_PREPASS
        previous_world_position,
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
        unclipped_depth,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
        input.instance_index,
#endif
#ifdef VERTEX_COLORS
        vec4<f32>(1.0),
#endif
#ifdef VISIBILITY_RANGE_DITHER
        mesh_functions::get_visibility_range_dither_level(input.instance_index, transform[3]),
#endif
    ); // 直接构造阶段输出，禁止接口Offset装饰污染Vulkan函数局部变量。
}
