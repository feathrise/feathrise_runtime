// P3角色和GPU叶片不进入TLAS；标准shadow map提供它们的方向光可见度。
// 当前Solari输出未分离直接光/GI/自发光，因此只做有下限的乘法兼容，不宣称物理解。

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    pbr_deferred_types::{DEFERRED_FLAGS_UNLIT_BIT, DEFERRED_MESH_FLAGS_SHADOW_RECEIVER_BIT, unpack_flags, unpack_24bit_normal},
    prepass_utils::prepass_depth,
    shadows::fetch_directional_shadow,
    utils::octahedral_decode,
    view_transformations::{position_ndc_to_world, position_world_to_view},
}

// 完全遮挡时仍保留35%既有HDR颜色，限制把GI、环境项与自发光一起误压暗的近似误差。
const MINIMUM_COMPOSITE_VISIBILITY: f32 = 0.35;

fn reconstruct_world_position(pixel: vec2<u32>, depth: f32) -> vec3<f32> {
    let viewport = view_bindings::view.main_pass_viewport;
    // textureLoad使用附件绝对坐标；NDC必须先扣除主通道viewport原点再按内部尺寸归一化。
    let local_pixel = vec2<f32>(pixel) - viewport.xy;
    let uv = (local_pixel + vec2(0.5)) / viewport.zw;
    let ndc = vec3((uv - vec2(0.5)) * vec2(2.0, -2.0), depth);
    return position_ndc_to_world(ndc);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<u32>(in.position.xy);
    let depth = prepass_depth(in.position, 0u);
    if depth == 0.0 {
        return vec4(1.0); // 天空、清屏与未写入主深度的像素绝不变暗。
    }
    let gpixel = textureLoad(view_bindings::deferred_prepass_texture, vec2<i32>(pixel), 0);
    let flags = unpack_flags(gpixel.a);
    if (flags & DEFERRED_MESH_FLAGS_SHADOW_RECEIVER_BIT) == 0u
        || (flags & DEFERRED_FLAGS_UNLIT_BIT) != 0u
    {
        return vec4(1.0); // 尊重作者NotShadowReceiver和Unlit契约；草叶继续使用bit2。
    }
    let world_position = reconstruct_world_position(pixel, depth);
    let world_normal = octahedral_decode(unpack_24bit_normal(gpixel.a));
    let view_z = position_world_to_view(world_position).z;
    var standard_visibility = 1.0;
    for (var light_id = 0u; light_id < view_bindings::lights.n_directional_lights; light_id += 1u) {
        let light = &view_bindings::lights.directional_lights[light_id];
        // Bevy上传的方向光颜色已经乘过illuminance；零照度日/月灯只保留shadow atlas，不能压暗最终HDR。
        let light_has_energy = max(max((*light).color.r, (*light).color.g), (*light).color.b) > 0.0;
        if ((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u || !light_has_energy {
            continue; // 跳过无阴影灯和零能量常驻灯，只合并本帧真实发光天体的可见度。
        }
        standard_visibility = min(
            standard_visibility,
            fetch_directional_shadow(
                light_id,
                vec4(world_position, 1.0),
                world_normal,
                view_z,
                in.position.xy,
            ),
        );
    }
    return vec4(mix(MINIMUM_COMPOSITE_VISIBILITY, 1.0, clamp(standard_visibility, 0.0, 1.0)));
}
