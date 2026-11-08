
// 与Bevy forward_io位置完全一致，并为深度Equal主通道显式保证位置不变量。
import bevy_pbr::render::mesh_functions;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::view_transformations::position_world_to_clip;
import feathrise_vegetation::mesh_backend::billboard::shaders::billboard::vegetation_billboard_world_from_local;
import feathrise_vegetation::mesh_backend::billboard::shaders::billboard::vegetation_billboard_normal_local_to_world;

struct VegetationBillboardVertexOutput {
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

// 主画面与阴影视图各自读取Bevy LOD观察位置，同一实例无需CPU改写共享Transform。
@vertex
fn vertex(vertex_in: Vertex) -> VegetationBillboardVertexOutput {
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    var out: VegetationBillboardVertexOutput; // 初始化与Bevy前向PBR兼容的顶点输出。
    let source_world_from_local =
        mesh_functions::get_world_from_local(vertex.instance_index); // 读取ECS传播的作者世界矩阵。
    let billboard_world_from_local = vegetation_billboard_world_from_local(
        source_world_from_local, // 保留平移、缩放与退化回退姿态。
        view.lod_view_world_position, // 主视图为自身相机，阴影视图使用关联玩法相机LOD原点。
    ); // 完成当前渲染视图专属GPU朝向。

    out.world_position = mesh_functions::mesh_position_local_to_world(
        billboard_world_from_local, // 顶点只在GPU使用定向后的世界矩阵。
        vec4<f32>(vertex.position, 1.0), // CPU Mesh资产和Transform保持不变。
    ); // 完成世界顶点位置计算。
    out.position = position_world_to_clip(out.world_position.xyz); // 使用当前主或阴影视图投影。

    @if(VERTEX_NORMALS) out.world_normal = vegetation_billboard_normal_local_to_world(
        billboard_world_from_local, // 法线必须跟随GPU朝向而不是作者静态矩阵。
        vertex.normal, // 保留导入Mesh的局部法线细节。
    ); // 完成PBR世界法线计算。

    @if(VERTEX_UVS_A) out.uv = vertex.uv; // 基础颜色和AlphaMask继续读取原第一套UV。
    @if(VERTEX_UVS_B) out.uv_b = vertex.uv_b; // 光照贴图等第二套UV保持原样。
    @if(VERTEX_TANGENTS) out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        billboard_world_from_local, // 法线贴图切线与GPU朝向保持一致。
        vertex.tangent, // 保留导入MikkTSpace局部切线。
        vertex.instance_index, // 原实例标志继续提供镜像行列式符号。
    ); // 完成世界切线计算。
    @if(VERTEX_COLORS) out.color = vertex.color; // 顶点色不参与公告板朝向并原样传递。
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = vertex.instance_index; // 片元阶段继续读取同一实例材质数据。
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, // 继续使用Bevy当前实例的宽域LOD索引。
        source_world_from_local[3], // GPU朝向不改变节点锚点，因此距离口径保持原样。
    ); // 完成LOD交叉抖动级别读取。

    return out; // 返回与StandardMaterial片元阶段完全兼容的顶点数据。
}
