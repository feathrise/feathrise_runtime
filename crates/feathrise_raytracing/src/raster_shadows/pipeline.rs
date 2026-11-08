//! 光栅阴影兼容全屏管线与Bevy MeshView布局专用化。

use bevy::{
    asset::{AssetServer, Handle, load_embedded_asset},
    core_pipeline::FullscreenShader,
    pbr::{MeshPipelineViewLayoutKey, MeshPipelineViewLayouts},
    prelude::{Commands, Component, Res, Resource, default},
    render::render_resource::{
        BlendComponent, BlendFactor, BlendOperation, BlendState, CachedRenderPipelineId,
        ColorTargetState, ColorWrites, FragmentState, RenderPipelineDescriptor,
        SpecializedRenderPipeline, TextureFormat,
    },
    shader::{Shader, ShaderDefVal},
};

/// 活动视图已经专用化并可由Core3d节点查询的管线ID。
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) struct RasterShadowCompositePipelineId(pub(super) CachedRenderPipelineId);

/// 仅由视图绑定组、全屏顶点着色器与项目阴影片元着色器组成的持久模板。
#[derive(Resource)]
pub(super) struct RasterShadowCompositePipeline {
    mesh_view_layouts: MeshPipelineViewLayouts,
    fullscreen_shader: FullscreenShader,
    fragment_shader: Handle<Shader>,
}

/// 不透明主颜色已存在时，片元输出作为destination因子实现原位 `dst.rgb *= visibility`。
pub(super) fn raster_shadow_color_target(format: TextureFormat) -> ColorTargetState {
    ColorTargetState {
        format,
        blend: Some(BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::Zero,
                dst_factor: BlendFactor::Src,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::Zero,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
        }),
        write_mask: ColorWrites::RED | ColorWrites::GREEN | ColorWrites::BLUE,
    }
}

/// RenderDevice和MeshPipeline布局就绪后创建一次模板；具体视图变体继续异步编译。
pub(super) fn init_raster_shadow_composite_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mesh_view_layouts: Res<MeshPipelineViewLayouts>,
    fullscreen_shader: Res<FullscreenShader>,
) {
    commands.insert_resource(RasterShadowCompositePipeline {
        mesh_view_layouts: mesh_view_layouts.clone(),
        fullscreen_shader: fullscreen_shader.clone(),
        fragment_shader: load_embedded_asset!(
            asset_server.as_ref(),
            "raster_shadow_composite.wesl"
        ),
    });
}

/// 只保留会改变group0布局、阴影采样实现或目标格式的逐视图键。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RasterShadowCompositePipelineKey {
    pub(super) mesh_view_layout_key: MeshPipelineViewLayoutKey,
    pub(super) target_format: TextureFormat,
    pub(super) shadow_filter_method: u8,
}

impl SpecializedRenderPipeline for RasterShadowCompositePipeline {
    type Key = RasterShadowCompositePipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let view_layout = self
            .mesh_view_layouts
            .get_view_layout(key.mesh_view_layout_key);
        let mut shader_defs = vec![
            ShaderDefVal::Bool("DEPTH_PREPASS".into(), true),
            ShaderDefVal::Bool("DEFERRED_PREPASS".into(), true),
        ];
        match key.shadow_filter_method {
            1 => shader_defs.push("SHADOW_FILTER_METHOD_HARDWARE_2X2".into()),
            2 => shader_defs.push("SHADOW_FILTER_METHOD_GAUSSIAN".into()),
            3 => shader_defs.push("SHADOW_FILTER_METHOD_TEMPORAL".into()),
            _ => {}
        }
        RenderPipelineDescriptor {
            label: Some("solari_raster_shadow_composite_pipeline".into()),
            // WGSL只读取MeshView group0；不把环境贴图等无关group1扩进兼容层ABI。
            layout: vec![view_layout.main_layout],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.fragment_shader.clone(),
                shader_defs,
                targets: vec![Some(raster_shadow_color_target(key.target_format))],
                ..default()
            }),
            ..default()
        }
    }
}
