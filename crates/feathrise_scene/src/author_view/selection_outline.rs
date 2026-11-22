//! 工作背景下的选中投影。原始网格绘入单通道遮罩，合成时只在覆盖范围外画橙边。

use super::ShadingView;
use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{Core3dSystems, FullscreenShader, core_3d, schedule::Core3d},
    mesh::{MeshVertexBufferLayoutRef, VertexAttributeValues},
    prelude::*,
    render::{
        Extract, ExtractSchedule, RenderApp, RenderStartup,
        camera::ExtractedCamera,
        mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator},
        render_asset::RenderAssets,
        render_phase::TrackedRenderPass,
        render_resource::{
            binding_types::{texture_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        sync_world::RenderEntity,
        texture::TextureCache,
        view::{ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

const MAX_PRIMITIVES: usize = 64; // 与选择解析的完整发布上限一致。

/// 仅引用已有网格实体。提取时读取已经传播的世界变换，包括父级非均匀缩放和剪切。
#[derive(Component, PartialEq, Eq)]
pub struct SelectionOutline(Vec<Entity>);

impl SelectionOutline {
    pub fn new(entities: Vec<Entity>) -> Self {
        debug_assert!(!entities.is_empty() && entities.len() <= MAX_PRIMITIVES);
        Self(entities)
    }
}

/// 遮罩复用静态、不裁切的三角表面，其他材质和变形源继续使用原有选择表现。
pub fn supports_selection_outline(mesh: &Mesh, material: &StandardMaterial) -> bool {
    material.alpha_mode == AlphaMode::Opaque
        && mesh.primitive_topology() == PrimitiveTopology::TriangleList
        && matches!(
            mesh.try_attribute(Mesh::ATTRIBUTE_POSITION),
            Ok(VertexAttributeValues::Float32x3(_))
        )
        && !mesh
            .try_contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
            .unwrap_or(true)
        && !mesh.try_has_morph_targets().unwrap_or(true)
}

#[derive(Component)]
struct OutlineSources(Vec<(AssetId<Mesh>, Mat4)>);

fn extract_selection(
    mut commands: Commands,
    views: Extract<
        Query<
            (
                &RenderEntity,
                Option<&SelectionOutline>,
                Option<&ShadingView>,
            ),
            With<Camera3d>,
        >,
    >,
    meshes: Extract<Query<(&Mesh3d, &GlobalTransform, &InheritedVisibility)>>,
) {
    for (camera, selection, shading) in &views {
        let sources = selection
            .filter(|selection| (1..=MAX_PRIMITIVES).contains(&selection.0.len()))
            .filter(|_| shading.is_some_and(ShadingView::uses_work_background))
            .and_then(|selection| {
                selection
                    .0
                    .iter()
                    .map(|entity| {
                        let (mesh, transform, visible) = meshes.get(*entity).ok()?;
                        visible.get().then(|| (mesh.0.id(), transform.to_matrix()))
                    })
                    .collect::<Option<Vec<_>>>()
            });
        if let Some(sources) = sources {
            commands.entity(camera.id()).insert(OutlineSources(sources));
        } else {
            commands.entity(camera.id()).remove::<OutlineSources>();
        }
    }
}

#[derive(Resource)]
struct OutlinePipelines {
    mask_layout: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
}

impl SpecializedMeshPipeline for OutlinePipelines {
    type Key = ();

    fn specialize(
        &self,
        _: (),
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let positions = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
        let defs = vec![bevy::shader::ShaderDefVal::Bool(
            "SELECTION_MASK".into(),
            true,
        )];
        Ok(RenderPipelineDescriptor {
            label: Some("editor_selection_mask".into()),
            layout: vec![self.mask_layout.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: defs.clone(),
                entry_point: Some("mask_vertex".into()),
                buffers: vec![positions],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: defs,
                entry_point: Some("mask_fragment".into()),
                targets: vec![Some(TextureFormat::R8Unorm.into())],
                ..default()
            }),
            ..default()
        })
    }
}

impl SpecializedRenderPipeline for OutlinePipelines {
    type Key = TextureFormat;

    fn specialize(&self, format: TextureFormat) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("editor_selection_outline".into()),
            layout: vec![self.composite_layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                entry_point: Some("outline_fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            depth_stencil: Some(super::render::author_color_depth()),
            ..default()
        }
    }
}

fn setup(mut commands: Commands, assets: Res<AssetServer>, fullscreen: Res<FullscreenShader>) {
    commands.insert_resource(OutlinePipelines {
        mask_layout: BindGroupLayoutDescriptor::new(
            "editor_selection_mask",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX,
                (
                    uniform_buffer::<ViewUniform>(true),
                    uniform_buffer::<Mat4>(true),
                ),
            ),
        ),
        composite_layout: BindGroupLayoutDescriptor::new(
            "editor_selection_outline",
            &BindGroupLayoutEntries::single(
                ShaderStages::FRAGMENT,
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        ),
        shader: load_embedded_asset!(assets.as_ref(), "selection_outline.wesl"),
        fullscreen: fullscreen.clone(),
    });
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "selection_outline.wesl");
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<SpecializedMeshPipelines<OutlinePipelines>>()
            .init_resource::<SpecializedRenderPipelines<OutlinePipelines>>()
            .init_resource::<PreparedOutline>()
            .add_systems(ExtractSchedule, extract_selection)
            .add_systems(RenderStartup, setup)
            .add_systems(
                Core3d,
                prepare_outline
                    .in_set(Core3dSystems::MainPass)
                    .before(core_3d::main_transparent_pass_3d),
            );
    }
}

#[derive(Resource, Default)]
pub(super) struct PreparedOutline(Option<(Entity, CachedRenderPipelineId, BindGroup)>);

impl PreparedOutline {
    pub(super) fn draw<'w>(&'w self, entity: Entity, cache: &'w PipelineCache, pass: &mut TrackedRenderPass<'w>) {
        let Some((view, pipeline, binding)) = &self.0 else { return; };
        if *view != entity {
            return;
        }
        let Some(pipeline) = cache.get_render_pipeline(*pipeline) else { return; };
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, binding, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn prepare_outline(
    view: ViewQuery<(
        Option<&OutlineSources>,
        &ViewTarget,
        &ExtractedCamera,
        &ViewUniformOffset,
        Option<&MainPassResolutionOverride>,
    )>,
    programs: Res<OutlinePipelines>,
    cache: Res<PipelineCache>,
    mut mask_pipelines: ResMut<SpecializedMeshPipelines<OutlinePipelines>>,
    mut composite_pipelines: ResMut<SpecializedRenderPipelines<OutlinePipelines>>,
    meshes: Res<RenderAssets<RenderMesh>>,
    allocator: Res<MeshAllocator>,
    views: Res<ViewUniforms>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut textures: ResMut<TextureCache>,
    mut transforms: Local<DynamicUniformBuffer<Mat4>>,
    mut context: RenderContext,
    mut prepared: ResMut<PreparedOutline>,
) {
    let entity = view.entity();
    prepared.0 = None;
    let (selection, target, camera, view_offset, resolution) = view.into_inner();
    let Some(selection) = selection else { return; };
    let size = target.main_texture().size();
    // 工作背景持有单采样附件租约；换挡时跳过尚未归还的旧附件。
    if target.sampled_main_texture().is_some() || size.width == 0 || size.height == 0 {
        return;
    }
    let composite = composite_pipelines.specialize(&cache, &programs, target.main_texture_format());
    transforms.clear();
    let mut draws = Vec::with_capacity(selection.0.len());
    for (id, world_from_model) in &selection.0 {
        let (Some(mesh), Some(vertex)) = (meshes.get(*id), allocator.mesh_vertex_slice(id)) else {
            return;
        };
        let Ok(pipeline) = mask_pipelines.specialize(&cache, &programs, (), &mesh.layout) else {
            return;
        };
        let Some(pipeline) = cache.get_render_pipeline(pipeline) else {
            return;
        };
        let index = match mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                count,
                index_format,
            } => {
                let Some(slice) = allocator.mesh_index_slice(id) else {
                    return;
                };
                Some((slice, count, index_format))
            }
            RenderMeshBufferInfo::NonIndexed => None,
        };
        draws.push((
            pipeline,
            vertex,
            mesh.vertex_count,
            index,
            transforms.push(world_from_model),
        ));
    }
    if cache.get_render_pipeline(composite).is_none() {
        return;
    }
    let Some(view_uniform) = views.uniforms.binding() else { return; };
    transforms.write_buffer(&device, &queue);
    let Some(affine_uniform) = transforms.binding() else {
        return;
    };
    let mask_uniforms = device.create_bind_group(
        "editor_selection_mask",
        &cache.get_bind_group_layout(&programs.mask_layout),
        &BindGroupEntries::sequential((view_uniform, affine_uniform)),
    );
    let mask = textures.get(
        &device,
        TextureDescriptor {
            label: Some("editor_selection_mask"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
    );
    let viewport = Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution);
    {
        // 直接引用 Bevy 已上传的顶点/索引分配，且一批齐备后才描边，避免多材质模型出现半个轮廓。
        let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("editor_selection_mask"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &mask.default_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(default()),
                    store: StoreOp::Store,
                },
            })],
            ..default()
        });
        if let Some(viewport) = &viewport {
            pass.set_camera_viewport(viewport);
        }
        for (pipeline, vertex, vertex_count, index, offset) in draws {
            pass.set_render_pipeline(pipeline);
            pass.set_bind_group(0, &mask_uniforms, &[view_offset.offset, offset]);
            pass.set_vertex_buffer(0, vertex.buffer.slice(..));
            if let Some((index, count, format)) = index {
                pass.set_index_buffer(index.buffer.slice(..), format);
                pass.draw_indexed(
                    index.range.start..index.range.start + count,
                    vertex.range.start as i32,
                    0..1,
                );
            } else {
                pass.draw(vertex.range.start..vertex.range.start + vertex_count, 0..1);
            }
        }
    }
    let outline = device.create_bind_group(
        "editor_selection_outline",
        &cache.get_bind_group_layout(&programs.composite_layout),
        &BindGroupEntries::single(&mask.default_view),
    );
    prepared.0 = Some((entity, composite, outline));
}
