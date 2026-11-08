// 主视图的组合 Clipmap Mesh 顶点入口；只支持静态无骨骼/无形变地形。

import bevy_pbr::render::mesh_functions;
import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import feathrise_scene::maps::dynamic_map::clipmap::shaders::height as clipmap_height;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    let input = decompress_vertex(vertex_in, vertex_in.instance_index);
    var out: VertexOutput; // 满足 Bevy StandardMaterial 的前向 PBR 输出布局。
    let world_from_local = mesh_functions::get_world_from_local(input.instance_index); // 单实体组合 Mesh 的相机吸附平移。
    let flat = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(input.position, 1.0)); // 尚未按各层独立吸附的世界位置。
    @if(VERTEX_UVS_B) let lod = input.uv_b; // x=细边或本级，y=组合 Mesh 的当前级。
    @if(VERTEX_UVS_B) out.uv_b = input.uv_b; // 保留 Bevy 顶点输出契约。
    @if(!VERTEX_UVS_B) let lod = vec2<f32>(1.0, 0.0); // 非本项目 Mesh 的安全编译回退。
    let world_xz = clipmap_height::stable_world_xz(clipmap_height::source_position(flat.xyz).xz,
        clipmap_height::source_position(world_from_local[3].xyz).xz, lod);
    out.world_position = clipmap_height::world_position(vec3<f32>(world_xz.x, clipmap_height::height_at(world_xz, lod), world_xz.y));
    out.position = position_world_to_clip(out.world_position.xyz); // Forward 与深度路径使用相同世界面。
    out.world_normal = clipmap_height::world_normal(clipmap_height::normal_at(world_xz, lod));
    @if(VERTEX_UVS_A) out.uv = input.uv; // 保留可选基础 UV 以兼容 StandardMaterial。
    @if(VERTEX_COLORS) out.color = input.color; // 保留可选顶点色。
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = input.instance_index; // Bevy 材质批次读取当前实例。
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(input.instance_index, world_from_local[3]); // 远景可见性过渡。
    return out; // 不使用 CPU 地形网格重建。
}
