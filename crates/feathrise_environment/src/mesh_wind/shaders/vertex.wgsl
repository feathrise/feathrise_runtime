#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    mesh_view_bindings::globals,
    view_transformations::position_world_to_clip,
}
#import feathrise_environment::mesh_wind::wind::{
    tree_wind_uniform,
    displace_tree_wind_local,
}

// 与Bevy forward_io位置完全一致，并为深度Equal主通道显式保证位置不变量。
struct TreeWindVertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_TANGENTS
    @location(4) world_tangent: vec4<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(6) @interpolate(flat) instance_index: u32,
#endif
#ifdef VISIBILITY_RANGE_DITHER
    @location(7) @interpolate(flat) visibility_range_dither: i32,
#endif
}

// 主渲染与阴影都读取当前帧风参数并调用唯一共享位移核心。
@vertex
fn vertex(vertex: Vertex) -> TreeWindVertexOutput {
    var out: TreeWindVertexOutput;
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

    // 快速法线路径保留导入法线，不为风位移重复采样或重建法线。
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
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
