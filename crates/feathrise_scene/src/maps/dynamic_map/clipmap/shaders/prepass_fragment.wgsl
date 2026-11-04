// 阴影深度、法线和运动预通道的地图边界裁切；Mask 模式确保 depth-only 也运行片元。
#import bevy_pbr::prepass_io::VertexOutput
#import feathrise_scene::clipmap_height

#ifdef PREPASS_FRAGMENT
#import bevy_pbr::prepass_io::FragmentOutput
#ifdef MOTION_VECTOR_PREPASS
#import bevy_pbr::pbr_prepass_functions::calculate_motion_vector
#endif

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    if !clipmap_height::inside_surface(clipmap_height::source_position(in.world_position.xyz).xz) {
        discard; // 阴影与主视图不能在地图外生成多余地形。
    }
    var out: FragmentOutput; // 按 Bevy 传入的预通道 defines 写需要的目标。
#ifdef NORMAL_PREPASS
    out.normal = vec4<f32>(in.world_normal * 0.5 + vec3<f32>(0.5), 1.0); // 与位移梯度同一法线。
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = calculate_motion_vector(in.world_position, in.previous_world_position); // 相机移动产生真实视差。
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth; // Bevy 无原生深度裁切能力的阴影仿真。
#endif
    return out; // Deferred 分支由 main_fragment 写完整 GBuffer。
}
#else
@fragment
fn fragment(in: VertexOutput) {
    if !clipmap_height::inside_surface(clipmap_height::source_position(in.world_position.xyz).xz) {
        discard; // 无颜色目标的深度与阴影也须裁切有限地图。
    }
}
#endif
