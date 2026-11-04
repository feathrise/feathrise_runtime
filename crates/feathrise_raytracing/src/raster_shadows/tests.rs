//! 光栅阴影兼容层的源码、混合与稳定边界回归。

use bevy::{
    prelude::Entity,
    render::render_resource::{BlendFactor, BlendOperation, ColorWrites, TextureFormat},
};
use feathrise_settings::GraphicsRuntimeRequirements;

use super::pipeline::raster_shadow_color_target;

#[test]
fn composite_target_multiplies_rgb_without_writing_alpha() {
    let target = raster_shadow_color_target(TextureFormat::Rgba16Float);
    let blend = target.blend.expect("兼容层必须使用硬件blend原位乘色");
    assert_eq!(blend.color.src_factor, BlendFactor::Zero);
    assert_eq!(blend.color.dst_factor, BlendFactor::Src);
    assert_eq!(blend.color.operation, BlendOperation::Add);
    assert_eq!(
        target.write_mask,
        ColorWrites::RED | ColorWrites::GREEN | ColorWrites::BLUE
    );
}

#[test]
fn shader_keeps_bounded_visibility_and_never_writes_history() {
    let shader = include_str!("raster_shadow_composite.wgsl");
    let energy_gate = shader
        .find("let light_has_energy =")
        .expect("方向光阴影采样前必须识别GPU预乘后的零能量灯"); // 锁定日/月常驻灯的能量门位置。
    let shadow_fetch = shader
        .rfind("fetch_directional_shadow(")
        .expect("阴影兼容层必须实际采样方向光shadow map"); // 跳过import中的同名符号，只定位真实调用。
    assert!(shader.contains("const MINIMUM_COMPOSITE_VISIBILITY: f32 = 0.35;"));
    assert!(shader.contains("DEFERRED_MESH_FLAGS_SHADOW_RECEIVER_BIT"));
    assert!(shader.contains("DEFERRED_FLAGS_UNLIT_BIT"));
    assert!(shader.contains("fetch_directional_shadow("));
    assert!(shader.contains("(*light).color")); // Bevy的GPU颜色已乘照度，不能另建一套CPU活动灯状态。
    assert!(shader.contains("|| !light_has_energy")); // 阴影flag仍开但照度为零时必须返回中性可见度。
    assert!(
        energy_gate < shadow_fetch,
        "零能量方向光必须在shadow atlas采样前被跳过"
    ); // 防止未来重构把门移到采样之后。
    assert!(shader.contains("view.main_pass_viewport"));
    assert!(shader.contains("vec2<f32>(pixel) - viewport.xy"));
    assert!(shader.contains("depth == 0.0"));
    assert!(!shader.contains("textureStore("));
    assert!(!shader.contains("constants.reset"));
    assert!(!shader.contains("reservoir"));
}

#[test]
fn raytracing_session_no_longer_suspends_standard_shadow_maps() {
    let warmup = GraphicsRuntimeRequirements::solari_warmup(Entity::PLACEHOLDER);
    assert!(
        !warmup.suspend_standard_shadows,
        "P3角色与GPU叶片不进TLAS，Solari活动租约必须持续保留标准shadow map",
    );
    let lifecycle = include_str!("../lifecycle/materials.rs");
    let denoising = include_str!("../denoising/systems.rs");
    assert!(!lifecycle.contains("with_standard_shadows_suspended(true)"));
    assert!(!denoising.contains("with_standard_shadows_suspended(runtime.stage"));
}

#[test]
fn composite_runs_after_opaque_grass_and_before_transmission_and_rr() {
    let module = include_str!("mod.rs");
    let node = include_str!("node.rs");
    assert!(module.contains(".after(main_opaque_pass_3d)"));
    assert!(module.contains(".before(main_transmissive_pass_3d)"));
    assert!(module.contains(".before(main_transparent_pass_3d)"));
    assert!(node.contains("target.get_color_attachment()"));
    assert!(!node.contains("post_process_write()"));
    assert!(node.contains("&SolariLighting"));
    assert!(node.contains("&DepthPrepass"));
    assert!(node.contains("&DeferredPrepass"));
}
