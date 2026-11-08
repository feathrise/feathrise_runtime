
// 与Bevy forward_io位置完全一致，并为深度Equal主通道显式保证位置不变量。
import bevy_pbr::render::mesh_functions;
import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::mesh_view_bindings::globals;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import feathrise_environment::mesh_wind::shaders::wind::tree_wind_uniform;
import feathrise_environment::mesh_wind::shaders::wind::displace_tree_wind_local;

struct TreeWindVertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @if(VERTEX_UVS_A) @location(2) uv: vec2<f32>,
    @if(VERTEX_UVS_B) @location(3) uv_b: vec2<f32>,
    @if(VERTEX_TANGENTS) @location(4) world_tangent: vec4<f32>,
    @if(VERTEX_COLORS) @location(5) color: vec4<f32>,
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) @location(6) @interpolate(flat) instance_index: u32,
    @if(VISIBILITY_RANGE_DITHER) @location(7) @interpolate(flat) visibility_range_dither: i32,
}

// 主渲染与阴影都读取当前帧风参数并调用唯一共享位移核心。
@vertex
fn vertex(vertex_in: Vertex) -> TreeWindVertexOutput {
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
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
    @if(VERTEX_NORMALS) out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );

    @if(VERTEX_UVS_A) out.uv = vertex.uv;
    @if(VERTEX_UVS_B) out.uv_b = vertex.uv_b;
    @if(VERTEX_TANGENTS) out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
    @if(VERTEX_COLORS) out.color = vertex.color;
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = vertex.instance_index;
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index,
        world_from_local[3],
    );

    return out;
}
