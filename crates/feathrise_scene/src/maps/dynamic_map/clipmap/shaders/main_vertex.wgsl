// 主视图的组合 Clipmap Mesh 顶点入口；只支持静态无骨骼/无形变地形。
#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
}
#import feathrise_scene::clipmap_height

@vertex
fn vertex(input: Vertex) -> VertexOutput {
    var out: VertexOutput; // 满足 Bevy StandardMaterial 的前向 PBR 输出布局。
    let world_from_local = mesh_functions::get_world_from_local(input.instance_index); // 单实体组合 Mesh 的相机吸附平移。
    let flat = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(input.position, 1.0)); // 尚未按各层独立吸附的世界位置。
#ifdef VERTEX_UVS_B
    let lod = input.uv_b; // x=细边或本级，y=组合 Mesh 的当前级。
    out.uv_b = input.uv_b; // 保留 Bevy 顶点输出契约。
#else
    let lod = vec2<f32>(1.0, 0.0); // 非本项目 Mesh 的安全编译回退。
#endif
    let world_xz = clipmap_height::stable_world_xz(clipmap_height::source_position(flat.xyz).xz,
        clipmap_height::source_position(world_from_local[3].xyz).xz, lod);
    out.world_position = clipmap_height::world_position(vec3<f32>(world_xz.x, clipmap_height::height_at(world_xz, lod), world_xz.y));
    out.position = position_world_to_clip(out.world_position.xyz); // Forward 与深度路径使用相同世界面。
    out.world_normal = clipmap_height::world_normal(clipmap_height::normal_at(world_xz, lod));
#ifdef VERTEX_UVS_A
    out.uv = input.uv; // 保留可选基础 UV 以兼容 StandardMaterial。
#endif
#ifdef VERTEX_COLORS
    out.color = input.color; // 保留可选顶点色。
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = input.instance_index; // Bevy 材质批次读取当前实例。
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(input.instance_index, world_from_local[3]); // 远景可见性过渡。
#endif
    return out; // 不使用 CPU 地形网格重建。
}
