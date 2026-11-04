//! Billboard 最终颜色与时间重建引导材质。

use bevy::{
    material::OpaqueRendererMethod,
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

/// 时间引导层只保留字形主体；透明软边仍由最终 Blend 四边形输出。
pub const BILLBOARD_TEMPORAL_GUIDE_ALPHA_CUTOFF: f32 = 0.5;

/// 保留原材质Uniform布局的兼容值；最终颜色投影现由实际绘制管线的WORLD_UI_PASS决定。
pub const BILLBOARD_PROJECTION_JITTERED: f32 = 0.0;
pub const BILLBOARD_PROJECTION_UNJITTERED: f32 = 1.0;
pub const BILLBOARD_GUIDE_GATE: Handle<bevy::render::storage::ShaderBuffer> =
    bevy::asset::uuid_handle!("891dd0a9-3d21-4b57-9a4a-b4fb5824d113"); // 固定16字节实际分流信号，普通渲染默认零。

/// 最终透明文字的顶点阶段扩展。
///
/// 【功能设计】只有实际独立绘制的WORLD_UI_PASS管线使用无抖动矩阵；普通与失败回退
/// 使用原场景矩阵。保留旧Uniform绑定以兼容既有材质创建接口，它不再决定颜色投影，
/// 避免主世界请求与RenderWorld当帧分流结果不一致。
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct BillboardColorExtension {
    #[uniform(100)]
    pub projection_mode: f32,
}

impl Default for BillboardColorExtension {
    fn default() -> Self {
        Self {
            projection_mode: BILLBOARD_PROJECTION_JITTERED,
        }
    }
}

impl MaterialExtension for BillboardColorExtension {
    fn vertex_shader() -> ShaderRef {
        super::shaders::BILLBOARD_COLOR_VERTEX_SHADER.clone().into()
    }

    /// 最终颜色层不写预通道；RR需要的深度/运动输入由独立引导子实体提供。
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// 最终透明文字材质：保留StandardMaterial片元/PBR绑定，仅替换顶点投影选择。
pub type BillboardColorMaterial = ExtendedMaterial<StandardMaterial, BillboardColorExtension>;

/// 只为时间引导层建立独立材质类型，避免被业务侧 `StandardMaterial` Deferred 桥计入预算。
///
/// 扩展保留Bevy PBR片元；额外共享信号只在实际分流成功时裁掉引导顶点。
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct BillboardTemporalGuideExtension {
    #[uniform(100)]
    pub(crate) projection_mode: f32,
    #[storage(101, read_only)] // 所有引导共享同帧信号，避免逐实体修改材质。
    pub(crate) guide_gate: Handle<bevy::render::storage::ShaderBuffer>, // 由通用插件初始化、独立UI在RenderWorld更新。
}

impl Default for BillboardTemporalGuideExtension {
    fn default() -> Self {
        Self {
            projection_mode: BILLBOARD_PROJECTION_JITTERED,
            guide_gate: BILLBOARD_GUIDE_GATE, // 默认允许正常引导，分流成功才暂停。
        }
    }
}

impl MaterialExtension for BillboardTemporalGuideExtension {
    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // 标记引导顶点，普通文字不读取额外绑定。
        descriptor
            .vertex
            .shader_defs
            .push("BILLBOARD_TEMPORAL_GUIDE".into()); // 同时覆盖引导主通道与预通道。
        Ok(()) // 不改变原材质布局与深度状态。
    } // 引导专用定义结束。
    fn vertex_shader() -> ShaderRef {
        super::shaders::BILLBOARD_COLOR_VERTEX_SHADER.clone().into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        super::shaders::BILLBOARD_PREPASS_VERTEX_SHADER
            .clone()
            .into()
    }

    fn deferred_vertex_shader() -> ShaderRef {
        super::shaders::BILLBOARD_PREPASS_VERTEX_SHADER
            .clone()
            .into()
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// 只在 Deferred + Motion + TemporalJitter 主视图中可见的字形引导材质。
pub type BillboardTemporalGuideMaterial =
    ExtendedMaterial<StandardMaterial, BillboardTemporalGuideExtension>;

/// 构造最终显示用的透明无光照文字材质。
pub(crate) fn billboard_color_material(image: Handle<Image>) -> BillboardColorMaterial {
    BillboardColorMaterial {
        base: StandardMaterial {
            base_color_texture: Some(image), // 显示离屏相机已经烘焙完成的文字颜色与透明度。
            alpha_mode: AlphaMode::Blend,    // 保留字体抗锯齿软边和完全透明背景。
            unlit: true,                     // 头顶 UI 不随场景光照改变颜色。
            depth_bias: 0.0, // 世界 UI 必须服从主场景深度，前景草叶或物体遮挡时不能穿透显示。
            ..default()
        },
        extension: BillboardColorExtension::default(), // 普通画面默认保留TemporalJitter。
    }
}

/// 构造 RR 所需的字形深度、运动向量与完整 GBuffer 引导材质。
pub(crate) fn billboard_temporal_guide_material(
    image: Handle<Image>,
) -> BillboardTemporalGuideMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            base_color_texture: Some(image), // 引导层与最终文字逐实例共享同一张烘焙纹理。
            alpha_mode: AlphaMode::Mask(BILLBOARD_TEMPORAL_GUIDE_ALPHA_CUTOFF),
            opaque_render_method: OpaqueRendererMethod::Deferred, // 同一像素同时写 GBuffer、深度与 motion。
            unlit: true, // GBuffer 仍保留字形原色，且不依赖专属 RenderLayer 上是否存在灯光。
            perceptual_roughness: 1.0, // 引导表面不提供会随视角跳变的高光历史。
            reflectance: 0.0,
            depth_bias: 0.0, // 引导层也必须服从场景深度，避免它替被遮挡文字写入错误的历史深度。
            ..default()
        },
        extension: BillboardTemporalGuideExtension::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_material_keeps_transparent_unlit_contract() {
        let image = Handle::<Image>::default();
        let material = billboard_color_material(image.clone());

        assert_eq!(material.base.base_color_texture, Some(image));
        assert_eq!(material.base.alpha_mode, AlphaMode::Blend);
        assert!(material.base.unlit);
        assert_eq!(material.base.depth_bias, 0.0);
        assert_eq!(
            material.base.opaque_render_method,
            OpaqueRendererMethod::Auto
        );
        assert_eq!(
            material.extension.projection_mode,
            BILLBOARD_PROJECTION_JITTERED
        );
    }

    #[test]
    fn temporal_guide_writes_masked_deferred_surface() {
        let image = Handle::<Image>::default();
        let material = billboard_temporal_guide_material(image.clone());

        assert_eq!(material.base.base_color_texture, Some(image));
        assert_eq!(
            material.base.alpha_mode,
            AlphaMode::Mask(BILLBOARD_TEMPORAL_GUIDE_ALPHA_CUTOFF)
        );
        assert_eq!(
            material.base.opaque_render_method,
            OpaqueRendererMethod::Deferred
        );
        assert!(material.base.unlit);
        assert_eq!(material.base.depth_bias, 0.0);
        assert_eq!(material.base.perceptual_roughness, 1.0);
        assert_eq!(material.base.reflectance, 0.0);
        assert_eq!(
            material.extension.projection_mode,
            BILLBOARD_PROJECTION_JITTERED
        );
    }
}
