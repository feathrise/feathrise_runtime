// 内置作者网格的真实面边界片元；只过滤线模式中的三角化对角线，正式显示/碰撞仍用原三角。
#ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::VertexOutput
#ifndef EDITOR_WIRE_DEPTH_ONLY
#import bevy_pbr::prepass_io::FragmentOutput
#endif
#ifdef DEFERRED_PREPASS
#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_deferred_functions::deferred_output,
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}
#endif
#ifdef MOTION_VECTOR_PREPASS
#import bevy_pbr::pbr_prepass_functions::calculate_motion_vector
#endif
#else
#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}
#endif

fn discard_diagonal(uv: vec2<f32>) {
    // 每个三角三个角共用mask，低位是barycentric；留出间隔避免插值误差跨mask边界。
    let mask = u32(floor((uv.x + 0.001) / 4.0));
    let x = uv.x - f32(mask) * 4.0;
    let bary = vec3<f32>(x, uv.y, 1.0 - x - uv.y);
    // 无条件求导，避免mask分支造成非一致控制流；像素距离容忍硬件线光栅的半像素偏移。
    let pixels = abs(bary) / max(fwidth(bary), vec3<f32>(0.000001));
    var distance = 1000000.0;
    if (mask & 1u) != 0u { distance = min(distance, pixels.x); }
    if (mask & 2u) != 0u { distance = min(distance, pixels.y); }
    if (mask & 4u) != 0u { distance = min(distance, pixels.z); }
    if distance > 1.1 { discard; }
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool)
#ifndef EDITOR_WIRE_DEPTH_ONLY
    -> FragmentOutput
#endif
{
    discard_diagonal(in.uv_b);
#ifndef EDITOR_WIRE_DEPTH_ONLY
#ifdef PREPASS_PIPELINE
#ifdef DEFERRED_PREPASS
    var pbr = pbr_input_from_standard_material(in, is_front);
    pbr.material.flags |= STANDARD_MATERIAL_FLAGS_UNLIT_BIT;
    pbr.material.base_color = vec4<f32>(0.0025, 0.0025, 0.0025, 1.0);
    return deferred_output(in, pbr);
#else
    var out: FragmentOutput;
#ifdef NORMAL_PREPASS
    out.normal = vec4<f32>(in.world_normal * 0.5 + vec3<f32>(0.5), 1.0);
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = calculate_motion_vector(in.world_position, in.previous_world_position);
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif
    return out;
#endif
#else
    var out: FragmentOutput;
    out.color = vec4<f32>(0.0025, 0.0025, 0.0025, 1.0);
    return out;
#endif
#endif
}
