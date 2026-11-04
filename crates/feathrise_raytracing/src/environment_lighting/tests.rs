//! Solari环境IBL兼容层的源码、混合与调度回归。

use bevy::render::render_resource::{
    BlendFactor,
    BlendOperation,
    ColorWrites,
    TextureFormat, // 精确检查HDR加法状态。
};

use super::pipeline::environment_color_target; // 直接验证管线目标而不启动RenderApp。

#[test]
fn environment_target_adds_rgb_without_writing_alpha() {
    let target = environment_color_target(TextureFormat::Rgba16Float); // 使用正式Solari HDR格式构造目标。
    let blend = target.blend.expect("环境兼容层必须使用硬件加法混合"); // 缺失blend会覆盖既有Solari颜色。
    assert_eq!(blend.color.src_factor, BlendFactor::One); // 完整保留Shader环境能量。
    assert_eq!(blend.color.dst_factor, BlendFactor::One); // 完整保留既有HDR主颜色。
    assert_eq!(blend.color.operation, BlendOperation::Add); // 两项只允许相加。
    assert_eq!(blend.alpha.src_factor, BlendFactor::Zero); // 环境层不拥有覆盖度。
    assert_eq!(blend.alpha.dst_factor, BlendFactor::One); // 目标Alpha保持原值。
    assert_eq!(
        target.write_mask,
        ColorWrites::RED | ColorWrites::GREEN | ColorWrites::BLUE
    ); // 管线层再次禁止Alpha写入。
}

#[test]
fn shader_consumes_both_environment_channels_and_preserves_content_boundaries() {
    let shader = include_str!("solari_environment_lighting.wgsl"); // 锁定随crate嵌入的真实源码。
    assert!(shader.contains("environment_map::environment_map_light(")); // 必须复用Bevy标准漫反射与GGX镜面采样。
    assert!(shader.contains("environment_light.diffuse")); // 天空补光必须进入HDR结果。
    assert!(shader.contains("environment_light.specular")); // 天空反射必须进入普通材质HDR结果。
    assert!(shader.contains("DEFERRED_FLAGS_GPU_VEGETATION_BIT")); // 草叶Forward最终色不得双算环境项。
    assert!(shader.contains("DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT")); // 动态地形镜面稳定基线必须保留。
    assert!(shader.contains("DEFERRED_FLAGS_UNLIT_BIT")); // Unlit材质不得被环境光改变。
    assert!(shader.contains("view.exposure * indirect_light")); // 与标准PBR保持同一HDR曝光语义。
    assert!(shader.contains("depth == 0.0")); // 可见天空背景不能被当作表面再次照亮。
    assert!(!shader.contains("textureStore(")); // 本层只通过受控颜色附件加法写入。
    assert!(!shader.contains("reservoir")); // 不修改已验收的Solari DI/GI历史算法。
    assert!(!shader.contains("constants.reset")); // 连续昼夜参数变化不能逐帧清空历史。
}

#[test]
fn environment_runs_after_shadow_and_before_transparency_and_rr() {
    let module = include_str!("mod.rs"); // 调度契约由安装模块唯一声明。
    let node = include_str!("node.rs"); // 节点附件行为单独锁定。
    let pipeline = include_str!("pipeline.rs"); // 两组环境绑定布局单独锁定。
    assert!(module.contains(".after(main_opaque_pass_3d)")); // 先等待GPU草叶Forward覆盖完成。
    assert!(module.contains(".after(RasterShadowCompositeSet)")); // 全局天空IBL不得被shadow map误乘。
    assert!(module.contains(".before(main_transmissive_pass_3d)")); // 水等透明PBR晚于本层且自行采环境图。
    assert!(module.contains(".before(main_transparent_pass_3d)")); // 粒子和透明内容不被GBuffer环境项覆盖。
    assert!(node.contains("target.get_color_attachment()")); // 原位HDR写入不翻转ViewTarget历史。
    assert!(!node.contains("post_process_write()")); // RR前主目标不能被私自交换。
    assert!(node.contains("mesh_view_bind_group.binding_array")); // group1必须绑定真实漫反射与镜面Cube。
    assert!(pipeline.contains("view_layout.binding_array_layout")); // 管线ABI必须声明同一group1布局。
    assert!(pipeline.contains("MULTIPLE_LIGHT_PROBES_IN_ARRAY")); // Vulkan绑定数组路径继续支持局部探针回退天空。
}
