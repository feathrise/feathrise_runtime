// 深度、法线、运动与阴影都执行和前向水面相同的R8、真实水深及缺页裁切。
#import bevy_pbr::prepass_io::VertexOutput
#import feathrise_scene::clipmap_water

#ifdef PREPASS_FRAGMENT
#import bevy_pbr::prepass_io::FragmentOutput
#ifdef MOTION_VECTOR_PREPASS
#import bevy_pbr::pbr_prepass_functions::calculate_motion_vector
#endif

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
#ifdef VERTEX_UVS_A
#ifdef VERTEX_UVS_B
    if clipmap_water::surface_coverage(in.uv, in.uv_b) <= 0.001 {
        discard;
    }
#else
    discard;
#endif
#else
    discard;
#endif
    return FragmentOutput(
#ifdef NORMAL_PREPASS
        vec4<f32>(in.world_normal * 0.5 + vec3<f32>(0.5), 1.0),
#endif
#ifdef MOTION_VECTOR_PREPASS
        calculate_motion_vector(in.world_position, in.previous_world_position),
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
        in.unclipped_depth,
#endif
    ); // 直接构造预通道输出，避免Vulkan拒绝带接口布局的Function结构变量。
}
#else
@fragment
fn fragment(in: VertexOutput) {
#ifdef VERTEX_UVS_A
#ifdef VERTEX_UVS_B
    if clipmap_water::surface_coverage(in.uv, in.uv_b) <= 0.001 {
        discard;
    }
#else
    discard;
#endif
#else
    discard;
#endif
}
#endif
