// 白模托底与线框的前向/Deferred PBR片元；正式地表预设仍由TerrainSurfaceExtension处理。
#import bevy_pbr::{
    pbr_functions::alpha_discard,
    pbr_fragment::pbr_input_from_standard_material,
}
#import feathrise_scene::clipmap_height

#ifdef PREPASS_PIPELINE
#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
}
#else
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}
#endif

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    if !clipmap_height::inside_surface(clipmap_height::source_position(in.world_position.xyz).xz) {
        discard; // 有限地图边界与阴影/depth pass 完全一致。
    }
    var pbr_input = pbr_input_from_standard_material(in, is_front); // 复用 Bevy StandardMaterial 光照参数。
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color); // 保留透明遮罩语义。
#ifdef PREPASS_PIPELINE
    return deferred_output(in, pbr_input); // 正式 GBuffer，避免默认品红回退。
#else
    var out: FragmentOutput; // Forward 使用标准 PBR 光照和后处理。
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input); // 接收 Bevy 动态灯光和阴影。
    } else {
        out.color = pbr_input.material.base_color; // 允许无光测试材质。
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color); // 雾和主通道后处理。
    return out; // 返回正式 PBR 颜色。
#endif
}
