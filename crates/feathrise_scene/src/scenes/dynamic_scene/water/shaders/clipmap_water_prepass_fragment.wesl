// 深度、法线、运动与阴影都执行和前向水面相同的R8、真实水深及缺页裁切。


import bevy_pbr::prepass::io::VertexOutput;
import feathrise_scene::scenes::dynamic_scene::water::shaders::clipmap_water;
@if(PREPASS_FRAGMENT) import bevy_pbr::prepass::io::FragmentOutput;
@if(PREPASS_FRAGMENT && MOTION_VECTOR_PREPASS) import bevy_pbr::render::pbr_prepass_functions::calculate_motion_vector;

@if(PREPASS_FRAGMENT) @fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    @if(VERTEX_UVS_A && VERTEX_UVS_B) if clipmap_water::surface_coverage(in.uv, in.uv_b) <= 0.001 {
        discard;
    }
    @if(VERTEX_UVS_A && !VERTEX_UVS_B) discard;
    @if(!VERTEX_UVS_A) discard;
    var out: FragmentOutput;
    @if(NORMAL_PREPASS) out.normal = vec4<f32>(in.world_normal * 0.5 + vec3<f32>(0.5), 1.0);
    @if(MOTION_VECTOR_PREPASS) out.motion_vector = calculate_motion_vector(in.world_position, in.previous_world_position);
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) out.frag_depth = in.unclipped_depth;
    return out;
}
@if(!PREPASS_FRAGMENT) @fragment
fn fragment(in: VertexOutput) {
    @if(VERTEX_UVS_A && VERTEX_UVS_B) if clipmap_water::surface_coverage(in.uv, in.uv_b) <= 0.001 {
        discard;
    }
    @if(VERTEX_UVS_A && !VERTEX_UVS_B) discard;
    @if(!VERTEX_UVS_A) discard;
}
