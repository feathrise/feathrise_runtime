// 白模托底与线框的前向/Deferred PBR片元；正式地表预设仍由TerrainSurfaceExtension处理。


import bevy_pbr::render::pbr_functions::alpha_discard;
import bevy_pbr::render::pbr_fragment::pbr_input_from_standard_material;
import feathrise_scene::scenes::dynamic_scene::clipmap::shaders::height as clipmap_height;
@if(PREPASS_PIPELINE && PREPASS_FRAGMENT) import bevy_pbr::prepass::io::VertexOutput;
@if(PREPASS_PIPELINE && PREPASS_FRAGMENT) import bevy_pbr::prepass::io::FragmentOutput;
@if(PREPASS_PIPELINE && PREPASS_FRAGMENT) import bevy_pbr::deferred::functions::deferred_output;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::forward_io::VertexOutput;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::forward_io::FragmentOutput;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::pbr_functions::apply_pbr_lighting;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::pbr_functions::main_pass_post_lighting_processing;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT;

@if(!PREPASS_PIPELINE || PREPASS_FRAGMENT) @fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    if !clipmap_height::inside_surface(clipmap_height::source_position(in.world_position.xyz).xz) {
        discard; // 有限场景边界与阴影/depth pass 完全一致。
    }
    var pbr_input = pbr_input_from_standard_material(in, is_front); // 复用 Bevy StandardMaterial 光照参数。
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    ); // 保留透明遮罩语义。
    @if(PREPASS_PIPELINE) return deferred_output(in, pbr_input); // 正式 GBuffer，避免默认品红回退。
    @if(!PREPASS_PIPELINE) var out: FragmentOutput; // Forward 使用标准 PBR 光照和后处理。
    @if(!PREPASS_PIPELINE) if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input); // 接收 Bevy 动态灯光和阴影。
    } else {
        out.color = pbr_input.material.base_color; // 允许无光测试材质。
    }
    @if(!PREPASS_PIPELINE) out.color = main_pass_post_lighting_processing(pbr_input, out.color); // 雾和主通道后处理。
    @if(!PREPASS_PIPELINE) return out; // 返回正式 PBR 颜色。
}
