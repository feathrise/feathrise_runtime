//! 每视图预通道后处理；固定一次全屏绘制，不捕获或遍历场景对象。
use super::components::ShadingView;
use super::selection_outline::PreparedOutline;
use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{
        Core3dSystems, FullscreenShader,
        core_3d::{CORE_3D_DEPTH_FORMAT, Transparent3d, TransparentSortingInfo3d, main_transparent_pass_3d},
        prepass::ViewPrepassTextures, schedule::Core3d,
    },
    ecs::{query::ROQueryItem, system::{SystemParamItem, lifetimeless::SRes}},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponentPlugin, UniformComponentPlugin,
        },
        render_phase::{AddRenderCommand, DrawFunctions, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases},
        render_resource::{
            binding_types::{texture_2d, texture_depth_2d, uniform_buffer},
            *,
        },
        renderer::{RenderDevice, ViewQuery},
        sync_world::MainEntity,
        view::{ExtractedView, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

pub struct ShadingRenderPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AuthorColorQueue;

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
                .init_resource::<PreparedSolid>()
                .add_render_command::<Transparent3d, (SetItemPipeline, DrawAuthorColor)>()
                .add_systems(RenderStartup, init_pipeline)
                .add_systems(Render, queue_author_color
                    .in_set(RenderSystems::Queue)
                    .in_set(AuthorColorQueue))
                .add_systems(
                    Core3d,
                    prepare_solid
                        .in_set(Core3dSystems::MainPass)
                        .before(main_transparent_pass_3d),
                );
        }
        super::selection_outline::install(app);
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
                    texture_depth_2d(),
                    uniform_buffer::<ShadingView>(true),
                    uniform_buffer::<ViewUniform>(true),
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
            depth_stencil: Some(author_color_depth()),
            ..default()
        }
    }
}

/// 作者颜色只读预通道深度，透明通道自己的深度附件不能被全屏三角形改写。
pub(super) fn author_color_depth() -> DepthStencilState {
    DepthStencilState {
        format: CORE_3D_DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(CompareFunction::Always),
        stencil: StencilState::default(),
        bias: DepthBiasState::default(),
    }
}

fn uses_author_color(view: &ShadingView) -> bool {
    view.options.x <= 1.0 || view.options.w < 0.5
}

fn queue_author_color(
    views: Query<(Entity, &ExtractedView, &ShadingView, &Msaa)>,
    draw_functions: Res<DrawFunctions<Transparent3d>>,
    pipeline: Option<Res<SolidPipeline>>,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<SolidPipeline>>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
) {
    let Some(pipeline) = pipeline else { return; };
    let draw_function = draw_functions.read().id::<(SetItemPipeline, DrawAuthorColor)>();
    for (entity, view, settings, msaa) in &views {
        if !uses_author_color(settings) || *msaa != Msaa::Off {
            continue;
        }
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else { continue; };
        let id = pipelines.specialize(&cache, &pipeline, view.target_format);
        if cache.get_render_pipeline(id).is_none() {
            continue;
        }
        // 大气会混合天空和实体颜色。进入原透明通道后先原位调色，再画网格和手柄。
        // 这里的虚拟图元不能和参考网格复用同一个 phase 键。
        phase.add_transient(Transparent3d {
            sorting_info: TransparentSortingInfo3d::AlwaysOnTop,
            distance: f32::NEG_INFINITY,
            pipeline: id,
            entity: (entity, MainEntity::from(Entity::PLACEHOLDER)),
            draw_function,
            batch_range: 0..1,
            extra_index: PhaseItemExtraIndex::None,
            indexed: false,
        });
    }
}

#[derive(Default)]
struct BindCache {
    entries: Vec<(
        (TextureViewId, TextureViewId, BufferId, BufferId),
        BindGroup,
    )>,
}

#[derive(Resource, Default)]
struct PreparedSolid(Option<(Entity, BindGroup)>);

fn prepare_solid(
    view: ViewQuery<(
        &ViewTarget,
        Option<&ViewPrepassTextures>,
        &ShadingView,
    )>,
    pipeline: Option<Res<SolidPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<ShadingView>>,
    views: Res<ViewUniforms>,
    device: Res<RenderDevice>,
    mut bindings: Local<BindCache>,
    mut prepared: ResMut<PreparedSolid>,
) {
    let entity = view.entity();
    prepared.0 = None;
    let (target, prepass, settings) = view.into_inner();
    if !uses_author_color(settings) {
        return;
    }
    let Some(prepass) = prepass else { return; };
    let Some(pipeline) = pipeline else {
        return;
    };
    // 模式和附件跨帧采用。只准备已同步的深度及法线纹理，附件未齐时保留原图。
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
    let (Some(normal), Some(depth), Some(uniform), Some(buffer), Some(view_uniform), Some(view_buffer)) = (
        prepass.normal_view(),
        prepass.depth_only_view(),
        uniforms.uniforms().binding(),
        uniforms.uniforms().buffer(),
        views.uniforms.binding(),
        views.uniforms.buffer(),
    ) else {
        return;
    };
    let key = (normal.id(), depth.id(), buffer.id(), view_buffer.id());
    let slot = bindings
        .entries
        .iter()
        .position(|(candidate, _)| *candidate == key)
        .unwrap_or_else(|| {
            if bindings.entries.len() == 8 {
                bindings.entries.remove(0);
            }
            let binding = device.create_bind_group(
                "editor_solid_bind",
                &pipeline_cache.get_bind_group_layout(&pipeline.layout),
                &BindGroupEntries::sequential((normal, depth, uniform, view_uniform)),
            );
            bindings.entries.push((key, binding));
            bindings.entries.len() - 1
        });
    prepared.0 = Some((entity, bindings.entries[slot].1.clone()));
}

struct DrawAuthorColor;

impl RenderCommand<Transparent3d> for DrawAuthorColor {
    type Param = (SRes<PreparedSolid>, SRes<PreparedOutline>, SRes<PipelineCache>);
    type ViewQuery = (&'static ViewUniformOffset, &'static DynamicUniformIndex<ShadingView>);
    type ItemQuery = ();

    fn render<'w>(
        item: &Transparent3d,
        (view, settings): ROQueryItem<'w, '_, Self::ViewQuery>,
        _: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (solid, outline, cache): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let entity = item.entity.0;
        if let Some((_, binding)) = solid.into_inner().0.as_ref().filter(|(view, _)| *view == entity) {
            pass.set_bind_group(0, binding, &[settings.index(), view.offset]);
            pass.draw(0..3, 0..1);
        }
        // 选中蒙版先在同一视图离屏绘制。橙边在实体替色后合成，透明工具仍排在最上方。
        outline.into_inner().draw(entity, cache.into_inner(), pass);
        RenderCommandResult::Success
    }
}
