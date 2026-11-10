// Bevy 0.19 阴影、深度、法线、运动和 Deferred 共用的高度位移顶点入口。

import bevy_pbr::render::mesh_functions;
import bevy_pbr::prepass::io::{Vertex, decompress_vertex};
import bevy_pbr::prepass::io::VertexOutput;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import feathrise_scene::scenes::dynamic_scene::clipmap::shaders::height as clipmap_height;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    let input = decompress_vertex(vertex_in, vertex_in.instance_index);
    var out: VertexOutput; // Bevy prepass/shadow 的条件字段由 shader defs 裁减。
    let world_from_local = mesh_functions::get_world_from_local(input.instance_index); // 与主视图相同的实例变换。
    let flat = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(input.position, 1.0)); // 尚未按各层独立吸附的世界位置。
    @if(VERTEX_UVS_B) let lod = input.uv_b; // 组合 Mesh 的细边和层级编码。
    @if(VERTEX_UVS_B) out.uv_b = input.uv_b; // 供预通道材质保持顶点属性契约。
    @if(!VERTEX_UVS_B) let lod = vec2<f32>(1.0, 0.0); // 调试网格回退为最高精度。
    let world_xz = clipmap_height::stable_world_xz(clipmap_height::source_position(flat.xyz).xz,
        clipmap_height::source_position(world_from_local[3].xyz).xz, lod);
    out.world_position = clipmap_height::world_position(vec3<f32>(world_xz.x, clipmap_height::height_at(world_xz, lod), world_xz.y));
    out.position = position_world_to_clip(out.world_position.xyz); // 深度、阴影、Deferred 同一顶点高度。
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) out.unclipped_depth = out.position.z; // 沿用 Bevy 定向光阴影的深度仿真。
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) out.position.z = min(out.position.z, 1.0); // 无原生 depth_clip_control 时避免前平面误裁。
    @if(VERTEX_UVS_A) out.uv = input.uv; // 可选基础 UV。
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS) out.world_normal = clipmap_height::world_normal(clipmap_height::normal_at(world_xz, lod));
    @if(MOTION_VECTOR_PREPASS) out.previous_world_position = out.world_position; // 地形静止于世界；相机中心滑动不应产生虚假物体速度。
    @if(VERTEX_COLORS) out.color = input.color; // 保留可选顶点色。
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = input.instance_index; // Bevy 材质绑定使用当前实例。
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(input.instance_index, world_from_local[3]); // 可见性范围一致。
    return out; // 编辑高度造成的上一帧高度差仍需要后续历史页或响应遮罩。
}
