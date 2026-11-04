#import bevy_pbr::{
    mesh_functions,
    prepass_io::Vertex,
    view_transformations::position_world_to_clip,
}
#import bevy_render::globals::Globals
#import feathrise_environment::mesh_wind::wind::{
    tree_wind_uniform,
    displace_tree_wind_local,
}

// Bevy 0.19预通道把Globals放在group 0 / binding 1，与主视图binding 11不同。
@group(0) @binding(1) var<uniform> globals: Globals;

// 与Bevy prepass_io位置完全一致，并让预通道与Equal主通道共享位置不变量保证。
struct TreeWindPrepassVertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
#ifdef VERTEX_UVS_A
    @location(0) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(1) uv_b: vec2<f32>,
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    @location(2) world_normal: vec3<f32>,
#ifdef VERTEX_TANGENTS
    @location(3) world_tangent: vec4<f32>,
#endif
#endif
    @location(4) world_position: vec4<f32>,
#ifdef MOTION_VECTOR_PREPASS
    @location(5) previous_world_position: vec4<f32>,
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(6) unclipped_depth: f32,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(7) instance_index: u32,
#endif
#ifdef VERTEX_COLORS
    @location(8) color: vec4<f32>,
#endif
#ifdef VISIBILITY_RANGE_DITHER
    @location(9) @interpolate(flat) visibility_range_dither: i32,
#endif
}

// 深度、法线和延迟使用当前姿态；运动向量同时重建上一帧风与世界变换。
@vertex
fn vertex(vertex: Vertex) -> TreeWindPrepassVertexOutput {
    var out: TreeWindPrepassVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let displaced_local = displace_tree_wind_local(
        tree_wind_uniform.current,
        vertex.position,
        world_from_local,
        globals.time,
        tree_wind_uniform.aabb_min.y,
        tree_wind_uniform.aabb_max.y,
    );

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(displaced_local, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

    // 预通道也保留快速导入法线，确保PBR主画面与法线预通道接口一致。
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef MOTION_VECTOR_PREPASS
    let previous_world_from_local =
        mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let previous_displaced_local = displace_tree_wind_local(
        tree_wind_uniform.previous,
        vertex.position,
        previous_world_from_local,
        globals.time - globals.delta_time,
        tree_wind_uniform.aabb_min.y,
        tree_wind_uniform.aabb_max.y,
    );
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        previous_world_from_local,
        vec4<f32>(previous_displaced_local, 1.0),
    );
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index,
        world_from_local[3],
    );
#endif

    return out;
}
