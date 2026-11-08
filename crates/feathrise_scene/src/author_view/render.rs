//! 每视图预通道后处理；固定一次全屏绘制，不捕获或遍历地图对象。
use super::components::ShadingView;
use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{
        Core3dSystems, FullscreenShader, prepass::ViewPrepassTextures, schedule::Core3d,
    },
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponentPlugin, UniformComponentPlugin,
        },
        render_resource::{
            binding_types::{texture_2d, texture_depth_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, ViewQuery},
        view::ViewTarget,
    },
};

pub struct ShadingRenderPlugin;
impl Plugin for ShadingRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "solid.wesl");
        embedded_asset!(app, "face_wire.wesl");
        app.add_plugins((
            ExtractComponentPlugin::<ShadingView>::default(),
            UniformComponentPlugin::<ShadingView>::default(),
        ));
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .init_resource::<SpecializedRenderPipelines<SolidPipeline>>()
                .add_systems(RenderStartup, init_pipeline)
                .add_systems(
                    Core3d,
                    solid_pass
                        .in_set(Core3dSystems::MainPass)
                        .after(bevy::core_pipeline::core_3d::main_opaque_pass_3d)
                        .before(bevy::core_pipeline::core_3d::main_transparent_pass_3d),
                ); // 先改变实体表面，再画透明内容与编辑Gizmo，不能盖掉点边面高亮和手柄。
        }
    }
    fn finish(&self, app: &mut App) {
        super::wireframe::install(app);
    }
}
#[derive(Resource)]
struct SolidPipeline {
    layout: BindGroupLayoutDescriptor,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}
fn init_pipeline(
    mut commands: Commands,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    commands.insert_resource(super::wireframe::FaceWireShader(load_embedded_asset!(
        assets.as_ref(),
        "face_wire.wesl"
    )));
    commands.insert_resource(SolidPipeline {
        layout: BindGroupLayoutDescriptor::new(
            "editor_solid_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    texture_depth_2d(),
                    uniform_buffer::<ShadingView>(true),
                ),
            ),
        ),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "solid.wesl"),
    });
}
impl SpecializedRenderPipeline for SolidPipeline {
    type Key = TextureFormat;
    fn specialize(&self, format: TextureFormat) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("editor_solid_pipeline".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}
#[derive(Default)]
struct BindCache {
    entries: Vec<(
        (TextureViewId, TextureViewId, TextureViewId, BufferId),
        BindGroup,
    )>,
}
fn solid_pass(
    view: ViewQuery<(
        &ViewTarget,
        &ViewPrepassTextures,
        &ShadingView,
        &DynamicUniformIndex<ShadingView>,
    )>,
    pipeline: Option<Res<SolidPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<SolidPipeline>>,
    uniforms: Res<ComponentUniforms<ShadingView>>,
    mut bindings: Local<BindCache>,
    mut context: RenderContext,
) {
    let (target, prepass, settings, index) = view.into_inner();
    if settings.options.x > 1.0 && settings.options.w > 0.5 {
        return;
    }
    let Some(pipeline) = pipeline else {
        return;
    };
    let id = pipelines.specialize(&pipeline_cache, &pipeline, target.main_texture_format());
    let Some(compiled) = pipeline_cache.get_render_pipeline(id) else {
        return;
    };
    // MainWorld模式和RenderApp附件可能跨帧交接；实际GPU纹理才是绑定合同的依据。
    // 必须在post_process_write交换主纹理之前检查，跳过旧附件时保持本帧原画面有效。
    let (Some(normal_attachment), Some(depth_attachment)) = (&prepass.normal, &prepass.depth)
    else {
        return;
    };
    let normal_texture = &normal_attachment.texture.texture;
    let depth_texture = &depth_attachment.texture.texture;
    if normal_texture.sample_count() != 1
        || depth_texture.sample_count() != 1
        || normal_texture.size() != depth_texture.size()
        || normal_texture.size() != target.main_texture().size()
    {
        return;
    }
    let (Some(normal), Some(depth), Some(uniform), Some(buffer)) = (
        prepass.normal_view(),
        prepass.depth_only_view(),
        uniforms.uniforms().binding(),
        uniforms.uniforms().buffer(),
    ) else {
        return;
    };
    let post = target.post_process_write();
    let key = (post.source.id(), normal.id(), depth.id(), buffer.id());
    let slot = bindings
        .entries
        .iter()
        .position(|(candidate, _)| *candidate == key)
        .unwrap_or_else(|| {
            if bindings.entries.len() == 8 {
                bindings.entries.remove(0);
            }
            let binding = context.render_device().create_bind_group(
                "editor_solid_bind",
                &pipeline_cache.get_bind_group_layout(&pipeline.layout),
                &BindGroupEntries::sequential((post.source, normal, depth, uniform)),
            );
            bindings.entries.push((key, binding));
            bindings.entries.len() - 1
        });
    let mut pass = context
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("editor_solid_view"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(compiled);
    pass.set_bind_group(0, &bindings.entries[slot].1, &[index.index()]);
    pass.draw(0..3, 0..1);
}
