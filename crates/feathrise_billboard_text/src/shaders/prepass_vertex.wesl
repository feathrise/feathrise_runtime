// Billboard 文字深度/法线/运动向量/Deferred预通道顶点入口。
// 与Bevy prepass_io::VertexOutput保持location ABI，只切换当前帧投影矩阵。


import bevy_pbr::render::mesh_functions;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::prepass::io::{Vertex, decompress_vertex};

@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<uniform> billboard_projection_mode: f32;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<storage, read> guide_gate: vec4<f32>; // 与主通道共享实际分流资格。

struct BillboardPrepassVertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @if(VERTEX_UVS_A) @location(0) uv: vec2<f32>,
    @if(VERTEX_UVS_B) @location(1) uv_b: vec2<f32>,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS) @location(2) world_normal: vec3<f32>,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS) @location(3) world_tangent: vec4<f32>,
    @location(4) world_position: vec4<f32>,
    @if(MOTION_VECTOR_PREPASS) @location(5) previous_world_position: vec4<f32>,
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) @location(6) unclipped_depth: f32,
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) @location(7) @interpolate(flat) instance_index: u32,
    @if(VERTEX_COLORS) @location(8) color: vec4<f32>,
    @if(VISIBILITY_RANGE_DITHER) @location(9) @interpolate(flat) visibility_range_dither: i32,
}

fn billboard_project(world_position: vec3<f32>) -> vec4<f32> {
    // 引导只属于普通场景，始终匹配场景抖动；分流成功通过guide_gate裁掉几何。
    return view.clip_from_world * vec4<f32>(world_position, 1.0);
}

@vertex
fn vertex(vertex_in: Vertex) -> BillboardPrepassVertexOutput {
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    var out: BillboardPrepassVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = billboard_project(out.world_position.xyz);
    if guide_gate.x > 0.5 { out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0); } // 不向已分流场景写入字形深度、运动或GBuffer。

    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) out.unclipped_depth = out.position.z;
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) out.position.z = min(out.position.z, 1.0);

    @if(VERTEX_UVS_A) out.uv = vertex.uv;
    @if(VERTEX_UVS_B) out.uv_b = vertex.uv_b;

    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS) out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS) out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );

    @if(MOTION_VECTOR_PREPASS) let previous_world_from_local =
        mesh_functions::get_previous_world_from_local(vertex.instance_index);
    @if(MOTION_VECTOR_PREPASS) out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        previous_world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );

    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = vertex.instance_index;
    @if(VERTEX_COLORS) out.color = vertex.color;
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index,
        world_from_local[3],
    );

    return out;
}
