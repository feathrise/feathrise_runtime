
// 与Bevy forward_io位置完全一致，并为深度Equal主通道显式保证位置不变量。
import bevy_pbr::render::mesh_functions;
import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::view_transformations::position_world_to_clip;
import feathrise_scene::shared::large_map::water::shaders::water::dynamic_water_uniform;
import feathrise_scene::shared::large_map::water::shaders::water::sample_dynamic_water;
import feathrise_scene::shared::large_map::water::shaders::water::dynamic_water_vertex_color;
import bevy_pbr::render::mesh_view_bindings::globals;

struct DynamicWaterVertexOutput {
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

// 主画面与阴影只执行GPU顶点采样；缺失TANGENT以零向量进入公共采样，禁止伪造固定+X流向法线。
@vertex
fn vertex(vertex_in: Vertex) -> DynamicWaterVertexOutput {
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    var out: DynamicWaterVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var rest_local_normal = vec3<f32>(0.0, 1.0, 0.0);
    @if(VERTEX_NORMALS) rest_local_normal = vertex.normal;
    var rest_local_tangent = vec3<f32>(0.0, 0.0, 0.0); // 零向量显式表示GLB没有合法切线，公共采样仍保留波高位移。
    @if(VERTEX_TANGENTS) rest_local_tangent = vertex.tangent.xyz;
    var rest_uv_b = vec2<f32>(-1.0, 0.0);
    @if(VERTEX_UVS_B) rest_uv_b = vertex.uv_b;
    var rest_vertex_alpha = 1.0;
    @if(VERTEX_COLORS) rest_vertex_alpha = vertex.color.a;
    let water_sample = sample_dynamic_water(
        false,
        globals.time, // 视觉动画读取RenderWorld共享时间，纯视觉整网格水无需逐材质CPU写入。
        vertex.position,
        rest_local_normal,
        rest_local_tangent,
        vertex.uv,
        rest_uv_b,
        rest_vertex_alpha,
    );

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(water_sample.local_position, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        water_sample.local_normal,
        vertex.instance_index,
    );

    @if(VERTEX_UVS_A) out.uv = vertex.uv;
    @if(VERTEX_UVS_B) out.uv_b = vertex.uv_b;
    @if(VERTEX_TANGENTS) out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
    @if(VERTEX_COLORS) out.color = dynamic_water_vertex_color(
        vertex.color,
        water_sample.visual_signal, // 样条流纹已由逐像素片元阶段生成，此处只保留玩家波谷顶点乘色。
    );
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = vertex.instance_index;
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index,
        world_from_local[3],
    );

    return out;
}
