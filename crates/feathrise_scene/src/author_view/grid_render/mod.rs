//! 作者视图解析式无限网格；作为正式透明通道首项合成，避免大气覆盖细线或影响后续Gizmo。
mod components;
mod resources;

use super::components::GridView;
use components::GridBindGroup;
use resources::{BindCache, GridPipeline};
use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{
        FullscreenShader,
        core_3d::{Transparent3d, TransparentSortingInfo3d, CORE_3D_DEPTH_FORMAT},
    },
    ecs::{query::ROQueryItem, system::SystemParamItem},
    gizmos_render::GizmoRenderSystems,
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{ComponentUniforms, DynamicUniformIndex, ExtractComponentPlugin, UniformComponentPlugin},
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItemExtraIndex, RenderCommand, RenderCommandResult,
            SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
        },
        render_resource::{binding_types::uniform_buffer, *},
        renderer::RenderDevice,
        sync_world::MainEntity,
        view::{ExtractedView, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

pub struct GridRenderPlugin;
impl Plugin for GridRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "infinite.wgsl");
        app.add_plugins((
            ExtractComponentPlugin::<GridView>::default(),
            UniformComponentPlugin::<GridView>::default(),
        ));
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.init_resource::<SpecializedRenderPipelines<GridPipeline>>()
                .add_render_command::<Transparent3d, (SetItemPipeline, DrawGrid)>()
                .add_systems(RenderStartup, init_pipeline)
                .add_systems(Render, queue_grid
                    .in_set(RenderSystems::Queue)
                    .before(GizmoRenderSystems::QueueLineGizmos3d))
                .add_systems(Render, prepare_grid_bindings.in_set(RenderSystems::PrepareBindGroups));
        } // 原生大气明确早于整个透明通道；不依赖其私有render_sky函数或未约束的MainPass先后顺序。
    }
}

fn init_pipeline(mut commands: Commands, assets: Res<AssetServer>, fullscreen: Res<FullscreenShader>) {
    commands.insert_resource(GridPipeline {
        layout: BindGroupLayoutDescriptor::new("editor_grid_layout", &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (uniform_buffer::<ViewUniform>(true), uniform_buffer::<GridView>(true)),
        )),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "infinite.wgsl"),
    });
}

impl SpecializedRenderPipeline for GridPipeline {
    type Key = (TextureFormat, u32);
    fn specialize(&self, (format, samples): Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("editor_infinite_grid".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(CompareFunction::Always),
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }), // 匹配正式透明通道附件，只叠加参考线，不改真实深度、遮挡或拾取。
            multisample: MultisampleState { count: samples, ..default() },
            ..default()
        }
    }
}

fn queue_grid(
    views: Query<(Entity, &ExtractedView, &Msaa, &MainEntity), With<GridView>>,
    draw_functions: Res<DrawFunctions<Transparent3d>>,
    pipeline: Option<Res<GridPipeline>>,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<GridPipeline>>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
) {
    let Some(pipeline) = pipeline else { return; };
    let draw_function = draw_functions.read().get_id::<(SetItemPipeline, DrawGrid)>().unwrap();
    for (entity, view, msaa, main_entity) in &views {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else { continue; };
        let id = pipelines.specialize(&cache, &pipeline, (view.target_format, msaa.samples()));
        if cache.get_render_pipeline(id).is_none() {
            continue; // 异步管线尚未就绪时保留原画面，禁止同步等待或提交半成品。
        }
        // Bevy 0.19的AlwaysOnTop键是负无穷，IndexMap::sort_by_key稳定保留同键插入顺序。
        // 本系统严格早于Gizmo排队，因此网格先画，随后才画选中高亮、落点十字和变换手柄。
        // 复用相机身份，每个作者视图只追加一个临时项，不新增地图实体、网格或逐线编码。
        phase.add_transient(Transparent3d {
            sorting_info: TransparentSortingInfo3d::AlwaysOnTop,
            distance: f32::NEG_INFINITY,
            pipeline: id,
            entity: (entity, *main_entity),
            draw_function,
            batch_range: 0..1,
            extra_index: PhaseItemExtraIndex::None,
            indexed: false,
        });
    }
}

fn prepare_grid_bindings(
    mut commands: Commands,
    cameras: Query<(Entity, Option<&GridBindGroup>), With<GridView>>,
    retired: Query<Entity, (With<GridBindGroup>, Without<GridView>)>,
    pipeline: Option<Res<GridPipeline>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    views: Res<ViewUniforms>,
    grids: Res<ComponentUniforms<GridView>>,
    mut bindings: Local<BindCache>,
) {
    for entity in &retired {
        commands.entity(entity).remove::<GridBindGroup>();
    }
    if cameras.is_empty() { return; } // 网格关闭后不为其他视图的缓冲换代建立无用绑定。
    let Some(pipeline) = pipeline else { return; };
    let (Some(view_binding), Some(view_buffer), Some(grid_binding), Some(grid_buffer)) = (
        views.uniforms.binding(), views.uniforms.buffer(),
        grids.uniforms().binding(), grids.uniforms().buffer(),
    ) else {
        for (entity, installed) in &cameras {
            if installed.is_some() { commands.entity(entity).remove::<GridBindGroup>(); }
        }
        return;
    };
    let key = (view_buffer.id(), grid_buffer.id());
    let slot = bindings.entries.iter().position(|(candidate, _)| *candidate == key).unwrap_or_else(|| {
        if bindings.entries.len() == 8 { bindings.entries.remove(0); }
        let binding = device.create_bind_group(
            "editor_grid_bind", &cache.get_bind_group_layout(&pipeline.layout),
            &BindGroupEntries::sequential((view_binding, grid_binding)),
        );
        bindings.entries.push((key, binding));
        bindings.entries.len() - 1
    });
    for (entity, installed) in &cameras {
        if installed.is_some_and(|installed| installed.buffers == key) { continue; }
        commands.entity(entity).insert(GridBindGroup { buffers: key, binding: bindings.entries[slot].1.clone() });
    } // 缓冲换代才替换小型绑定组件；缓存最多8项，与地图、可见线条和像素数无关。
}

struct DrawGrid;
impl RenderCommand<Transparent3d> for DrawGrid {
    type Param = ();
    type ViewQuery = (
        &'static GridView,
        Option<&'static GridBindGroup>,
        &'static ViewUniformOffset,
        &'static DynamicUniformIndex<GridView>,
    );
    type ItemQuery = ();

    fn render<'w>(
        _item: &Transparent3d,
        (_, binding, view_offset, grid_offset): ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        _param: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(binding) = binding else { return RenderCommandResult::Skip; };
        // 组件本身也是绘制资格，不能让关闭后遗留的DynamicUniformIndex重新使用旧平面。
        // 复用正式透明通道的MSAA颜色和只读深度附件，后续resolve不会覆盖本次网格。
        pass.set_bind_group(0, &binding.binding, &[view_offset.offset, grid_offset.index()]);
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}
