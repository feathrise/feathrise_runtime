//! GPU 草地 Deferred 表面与时间重建输入通道。
//!
//! 【功能设计】草叶继续使用 GPU 间接光栅且不进入 TLAS，但 DLSS RR 等时间重建必须在草叶像素
//! 取得同一草叶的深度、运动矢量和材质引导。普通前向视图在 AlphaMask 预通道写草叶深度与
//! 运动矢量，再由自定义 AlphaMask 主通道着色；启用 DeferredPrepass 的 Solari 视图先在
//! AlphaMask Deferred 阶段写入草叶自己的深度、GBuffer、lighting pass id 与运动矢量，随后仍
//! 由同一自定义 AlphaMask 植被光照覆盖最终颜色，避免严格单面 PBR 把薄叶压黑。后续射线继续
//! 只遍历获准的有界 TLAS；两类视图都复用 Compute 已生成的紧凑实例和间接命令，不增加 CPU
//! 逐草工作。

use bevy::{
    core_pipeline::{
        core_3d::CORE_3D_DEPTH_FORMAT,
        deferred::AlphaMask3dDeferred,
        prepass::{
            AlphaMask3dPrepass, DeferredPrepass, OpaqueNoLightmap3dBatchSetKey,
            OpaqueNoLightmap3dBinKey, prepass_target_descriptors,
        },
    },
    ecs::system::{SystemParam, SystemParamItem, lifetimeless::*},
    mesh::{MeshVertexBufferLayoutRef, VertexBufferLayout},
    pbr::{
        MeshPipelineKey, PrepassPipeline, RenderMeshInstances, SetMeshBindGroup,
        SetPrepassViewBindGroup, SetPrepassViewEmptyBindGroup, ViewKeyPrepassCache,
    },
    prelude::*,
    render::{
        mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator},
        render_asset::RenderAssets,
        render_phase::{
            BinnedRenderPhaseType, DrawFunctions, PhaseItem, RenderCommand, RenderCommandResult,
            SetItemPipeline, TrackedRenderPass, ViewBinnedRenderPhases,
        },
        render_resource::{
            CachedPipelineState, ColorTargetState, CompareFunction, DepthBiasState,
            DepthStencilState, Face, FragmentState, MultisampleState, PipelineCache,
            PrimitiveState, RenderPipelineDescriptor, SpecializedMeshPipeline,
            SpecializedMeshPipelineError, SpecializedMeshPipelines, StencilFaceState, StencilState,
            VertexAttribute, VertexFormat, VertexState, VertexStepMode,
        },
        view::{ExtractedView, RenderVisibleEntities},
    },
    shader::ShaderCacheError, // 区分Shader热加载等待与不可恢复管线错误，保持Deferred主绘制诊断准确。
};

use super::{commands::SetGpuBladeInteractionBindGroup, pipeline::GpuBladePipeline};
use crate::{
    components::{GpuBladePatch, GpuBladePreparedBuffers, GpuBladeView},
    constants::{
        GPU_GRASS_COMPACT_INSTANCE_BYTES, GPU_GRASS_DEPTH_BUCKET_COUNT,
        GPU_GRASS_DEPTH_BUCKET_STATE_BYTES,
    },
    resources::{GpuBladeRenderFrameCounters, GpuBladeTimingQueries}, // Deferred仅报告输入阻塞，主绘制与GPU计时继续由Forward颜色阶段唯一承担。
};

/// 为普通 Normal/Motion 预通道和 Deferred 草叶表面共用的专用化模板。
#[derive(Resource)]
pub(super) struct GpuBladePrepassPipeline {
    shader: Handle<Shader>, // 与主通道共用同一草叶形变源码，避免深度轮廓与最终颜色错位。
    prepass_pipeline: PrepassPipeline, // 复用 Bevy 当前版本的预通道视图、空组和网格布局。
    interaction_layout: bevy::render::render_resource::BindGroupLayoutDescriptor, // 风场与内容 profile 必须和主通道逐帧一致。
}

/// Bevy 预通道资源建立后，延迟安装草地时间重建管线模板。
pub(super) fn init_gpu_grass_prepass_pipeline(
    mut commands: Commands,
    main_pipeline: Res<GpuBladePipeline>,
    prepass_pipeline: Res<PrepassPipeline>,
    existing: Option<Res<GpuBladePrepassPipeline>>,
) {
    if existing.is_some() {
        return; // 热重载及稳定帧只复用既有模板和专用化缓存。
    }
    commands.insert_resource(GpuBladePrepassPipeline {
        shader: main_pipeline.shader.clone(),
        prepass_pipeline: prepass_pipeline.clone(),
        interaction_layout: main_pipeline.interaction_layout.clone(),
    });
}

impl SpecializedMeshPipeline for GpuBladePrepassPipeline {
    type Key = MeshPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mesh_vertex_layout = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
        ])?; // 形变输入与主通道保持完全相同的原型契约。
        let instance_vertex_layout = VertexBufferLayout {
            array_stride: GPU_GRASS_COMPACT_INSTANCE_BYTES,
            step_mode: VertexStepMode::Instance,
            attributes: vec![
                VertexAttribute {
                    format: VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 3,
                },
                VertexAttribute {
                    format: VertexFormat::Uint32,
                    offset: 12,
                    shader_location: 4,
                },
                VertexAttribute {
                    format: VertexFormat::Uint32,
                    offset: 16,
                    shader_location: 5,
                },
            ],
        };
        let mut shader_defs = vec!["GPU_GRASS_PREPASS".into()]; // 预通道公共变体始终重建当前草叶轮廓，历史姿态仅由Motion位按需启用。
        let writes_motion_vectors = key.contains(MeshPipelineKey::MOTION_VECTOR_PREPASS); // TAA/RR才需要上一帧位置与location1，SSAO-only必须使用无历史布局。
        let writes_normal_prepass = key.contains(MeshPipelineKey::NORMAL_PREPASS); // SSAO可与TAA同时要求法线和运动附件，专用化必须逐槽匹配真实RenderPass。
        let writes_deferred_surface = key.contains(MeshPipelineKey::DEFERRED_PREPASS); // Deferred视图必须提供RR解析所需的真实草叶材质表面。
        if writes_motion_vectors {
            shader_defs.push("MOTION_VECTOR_PREPASS".into()); // 只有真实Motion附件存在时才导入历史视图并写location1。
        }
        if writes_normal_prepass {
            shader_defs.push("NORMAL_PREPASS".into()); // 普通前向视图为SSAO写Bevy标准location0世界法线编码。
        }
        if writes_normal_prepass || writes_deferred_surface {
            shader_defs.push("NORMAL_PREPASS_OR_DEFERRED_PREPASS".into()); // 两类表面路径只重建并插值一份动态草叶世界法线。
        }
        if writes_deferred_surface {
            shader_defs.push("GPU_GRASS_DEFERRED_SURFACE".into()); // 只让四附件管线编译草叶GBuffer输出与额外插值。
        } // 普通前向预通道只启用当前视图实际存在的法线与运动附件。
        Ok(RenderPipelineDescriptor {
            label: Some(if writes_deferred_surface && writes_motion_vectors {
                "gpu_grass_deferred_surface_motion_pipeline".into() // Solari+RR路径写完整草叶首表面与时间输入。
            } else if writes_deferred_surface {
                "gpu_grass_deferred_surface_pipeline".into() // 无时间重建的Deferred路径只写当前草叶首表面。
            } else if writes_normal_prepass {
                if writes_motion_vectors {
                    "gpu_grass_normal_motion_prepass_pipeline".into() // TAA与SSAO并用时同时覆盖草叶法线和运动附件。
                } else {
                    "gpu_grass_normal_prepass_pipeline".into() // FXAA/关闭AA与SSAO并用时只写当前草叶法线。
                }
            } else {
                "gpu_grass_depth_motion_prepass_pipeline".into() // 普通前向路径只需要深度与运动向量。
            }),
            layout: vec![
                if writes_motion_vectors {
                    self.prepass_pipeline.view_layout_motion_vectors.clone()
                } else {
                    self.prepass_pipeline.view_layout_no_motion_vectors.clone()
                }, // group0必须与SetPrepassViewBindGroup按MotionVectorPrepass选择的真实绑定组完全一致。
                self.prepass_pipeline.empty_layout.clone(),
                self.prepass_pipeline.mesh_layouts.model_only.clone(),
                self.interaction_layout.clone(),
            ],
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: shader_defs.clone(),
                entry_point: Some("prepass_vertex".into()),
                buffers: vec![mesh_vertex_layout, instance_vertex_layout],
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                entry_point: Some("prepass_fragment".into()),
                // Normal、Motion与Deferred目标逐槽匹配真实RenderPass，并由草叶片元覆盖自己的表面输入。
                targets: gpu_grass_temporal_guide_targets(key),
            }),
            primitive: PrimitiveState {
                topology: key.primitive_topology(),
                strip_index_format: key.strip_index_format(),
                cull_mode: None::<Face>, // 单片草双面必须写入与主通道一致的深度覆盖。
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                // 草叶颜色会在主通道进入RR，因此最终深度和Deferred GBuffer也必须描述同一草叶表面；
                // 不进入TLAS只影响后续射线命中，不妨碍相机首表面由光栅GBuffer提供。
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState {
                    front: StencilFaceState::IGNORE,
                    back: StencilFaceState::IGNORE,
                    read_mask: 0,
                    write_mask: 0,
                },
                bias: DepthBiasState {
                    constant: 0,
                    slope_scale: 0.0,
                    clamp: 0.0,
                },
            }),
            multisample: MultisampleState {
                count: key.msaa_samples(),
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            ..default()
        })
    }
}

/// 构造与实际RenderPass完全兼容的目标列表；SSAO法线与Deferred GBuffer都由草叶自身覆盖。
#[inline]
fn gpu_grass_temporal_guide_targets(
    key: MeshPipelineKey, // 读取当前Normal、Motion与Deferred预通道组合，禁止硬编码附件槽。
) -> Vec<Option<ColorTargetState>> {
    let writes_motion_vectors = key.contains(MeshPipelineKey::MOTION_VECTOR_PREPASS); // FXAA+SSAO的location1为None，不能继续声明Rg16Float。
    let writes_normal_prepass = key.contains(MeshPipelineKey::NORMAL_PREPASS); // TAA+SSAO组合的location0为Rgb10a2Unorm，不能继续声明None。
    let writes_deferred_surface = key.contains(MeshPipelineKey::DEFERRED_PREPASS); // 四附件路径必须让草叶材质引导覆盖地下地形旧值。
    prepass_target_descriptors(
        writes_normal_prepass,
        writes_motion_vectors,
        writes_deferred_surface,
    ) // location0/1按真实视图声明，Deferred时再写2/3号GBuffer。
}

/// 为Deferred视图补入专用化位；普通前向预通道保留视图自身的Normal/Motion组合。
#[inline]
fn gpu_grass_temporal_guide_pipeline_key(
    view_key: MeshPipelineKey,
    has_deferred_prepass: bool,
) -> MeshPipelineKey {
    if has_deferred_prepass {
        view_key | MeshPipelineKey::DEFERRED_PREPASS
    } else {
        view_key
    }
}

/// 普通预通道不写草地 GPU 计时查询，避免与后续前向主颜色重复统计同一 profile/LOD 槽。
pub(super) type DrawGpuBladePrepass = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    SetMeshBindGroup<2>,
    SetGpuBladeInteractionBindGroup<3>,
    DrawGpuBladeTemporalInstanced<false>, // 普通预通道只补时间输入，不登记为最终颜色绘制。
);

/// Deferred阶段只提交Solari/RR所需的草叶表面和时间输入；最终颜色统计由后续Forward overlay唯一承担。
pub(super) type DrawGpuBladeDeferred = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    SetMeshBindGroup<2>,
    SetGpuBladeInteractionBindGroup<3>,
    DrawGpuBladeTemporalInstanced<false>, // 禁止GBuffer阶段重复占用主绘制计数与GPU查询，最终Forward颜色保持唯一统计所有者。
);

/// 复用 Compute 紧凑实例和八个深度桶，并按路径身份决定是否记录最终颜色绘制证据。
pub(super) struct DrawGpuBladeTemporalInstanced<const RECORD_MAIN_DRAW: bool>;

impl<P: PhaseItem, const RECORD_MAIN_DRAW: bool> RenderCommand<P>
    for DrawGpuBladeTemporalInstanced<RECORD_MAIN_DRAW>
{
    type Param = (
        SRes<RenderAssets<RenderMesh>>,
        SRes<RenderMeshInstances>,
        SRes<MeshAllocator>,
        SRes<GpuBladeRenderFrameCounters>, // 仅当该路径拥有最终颜色时才发布F3主绘制计数。
        SRes<GpuBladeTimingQueries>,       // 仅当该路径拥有最终颜色时才测量相同profile/LOD槽。
    );
    type ViewQuery = ();
    type ItemQuery = (Read<GpuBladePatch>, Read<GpuBladePreparedBuffers>); // 取得诊断身份和有效紧凑输出。

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        patch_and_buffers: Option<(&'w GpuBladePatch, &'w GpuBladePreparedBuffers)>,
        (meshes, render_mesh_instances, mesh_allocator, render_stats, timing_queries): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((patch, prepared_buffers)) = patch_and_buffers.filter(|(patch, buffers)| {
            super::output_readiness::draw_output_is_current(
                patch.instance_count(),
                buffers.output_valid,
            )
        })
        // 预通道同样只消费本帧完整有效输出。
        else {
            return RenderCommandResult::Skip;
        };
        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(item.main_entity())
        else {
            return RenderCommandResult::Skip;
        };
        let mesh_asset_id = mesh_instance.mesh_asset_id();
        let Some(gpu_mesh) = meshes.into_inner().get(mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };
        let mesh_allocator = mesh_allocator.into_inner();
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };
        pass.set_vertex_buffer(0, vertex_slice.buffer.slice(..));
        pass.set_vertex_buffer(1, prepared_buffers.instances.slice(..));
        let RenderMeshBufferInfo::Indexed { index_format, .. } = &gpu_mesh.buffer_info else {
            return RenderCommandResult::Skip; // 草地间接 ABI 固定为索引绘制，不允许静默换成直绘。
        };
        let Some(index_slice) = mesh_allocator.mesh_index_slice(&mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };
        pass.set_index_buffer(index_slice.buffer.slice(..), *index_format);
        let timing_queries = timing_queries.into_inner(); // 解开一次系统参数包装，供可选查询开始与结束共享同一资源。
        let timing_scope = RECORD_MAIN_DRAW.then(|| {
            crate::gpu_timing::begin_draw_measurement(
                timing_queries, // 最终颜色路径使用既有固定查询集，普通预通道与Deferred输入阶段不抢占。
                pass,           // 查询覆盖该路径真实八桶间接绘制成本。
                patch.profile_slot().index(), // 每种普通草、麦田或杂粮田保持独立诊断槽。
                patch.lod_level(), // 近、中、远、超远继续映射既有四级固定槽。
            ) // 返回必须在全部间接命令后配对结束的查询作用域。
        }); // 普通前向预通道保持None，避免抢占后续主颜色通道的本帧查询位。
        for depth_bucket_index in 0..GPU_GRASS_DEPTH_BUCKET_COUNT {
            pass.draw_indexed_indirect(
                &prepared_buffers.depth_bucket_draws,
                u64::from(depth_bucket_index) * GPU_GRASS_DEPTH_BUCKET_STATE_BYTES,
            );
        }
        if let Some(timing_scope) = timing_scope {
            crate::gpu_timing::end_draw_measurement(
                timing_queries, // 与开始阶段使用完全相同的固定查询资源。
                pass,           // 在同一Deferred草叶RenderPass内结束时间戳与管线统计。
                timing_scope,   // 只结束当前后端确实启用的查询类型。
            ); // 完成当前最终颜色路径草叶主绘制GPU测量。
        } // 普通前向预通道不产生重复测量。
        if RECORD_MAIN_DRAW {
            render_stats.into_inner().record_draw(
                patch.instance_count(),       // 保持候选实例统计与原前向主通道口径一致。
                GPU_GRASS_DEPTH_BUCKET_COUNT, // 当前窗口固定编码八条深度友好间接命令。
                patch.profile_slot().index(), // 区分普通草、麦田和杂粮田内容完整性。
                patch.lod_level(),            // 区分近中远及可选超远窗口。
            ); // 只有最终颜色路径才记录唯一主绘制证据。
        } // 普通预通道只负责时间输入，不重复增加主绘制统计。
        RenderCommandResult::Success
    }
}

/// 草地预通道排队所需的有界渲染世界状态。
#[derive(SystemParam)]
pub(super) struct GpuBladePrepassQueueParams<'w, 's> {
    prepass_draw_functions: Res<'w, DrawFunctions<AlphaMask3dPrepass>>,
    deferred_draw_functions: Res<'w, DrawFunctions<AlphaMask3dDeferred>>,
    grass_pipeline: Option<Res<'w, GpuBladePrepassPipeline>>,
    specialized_pipelines: ResMut<'w, SpecializedMeshPipelines<GpuBladePrepassPipeline>>,
    pipeline_cache: Res<'w, PipelineCache>,
    meshes: Res<'w, RenderAssets<RenderMesh>>,
    render_mesh_instances: Res<'w, RenderMeshInstances>,
    mesh_allocator: Res<'w, MeshAllocator>,
    grass_patches: Query<
        'w,
        's,
        (
            &'static GpuBladePatch,
            Option<&'static GpuBladePreparedBuffers>,
        ),
    >,
    render_stats: Res<'w, GpuBladeRenderFrameCounters>, // Deferred路径接管主颜色后发布排队与管线状态。
    view_key_cache: Res<'w, ViewKeyPrepassCache>,
    prepass_render_phases: ResMut<'w, ViewBinnedRenderPhases<AlphaMask3dPrepass>>,
    deferred_render_phases: ResMut<'w, ViewBinnedRenderPhases<AlphaMask3dDeferred>>,
    views: Query<
        'w,
        's,
        (
            &'static ExtractedView,
            &'static RenderVisibleEntities,
            Has<DeferredPrepass>,
        ),
        With<GpuBladeView>,
    >,
}

/// 只在玩法相机拥有Normal、Motion或Deferred表面附件时，把有效草窗加入对应预通道。
pub(super) fn queue_gpu_grass_prepass(params: GpuBladePrepassQueueParams) {
    let GpuBladePrepassQueueParams {
        prepass_draw_functions,
        deferred_draw_functions,
        grass_pipeline,
        mut specialized_pipelines,
        pipeline_cache,
        meshes,
        render_mesh_instances,
        mesh_allocator,
        grass_patches,
        render_stats,
        view_key_cache,
        mut prepass_render_phases,
        mut deferred_render_phases,
        views,
    } = params;
    let Some(grass_pipeline) = grass_pipeline else {
        return; // 第一个渲染帧等待 Bevy 预通道模板建立。
    };
    let draw_gpu_grass_prepass = prepass_draw_functions.read().id::<DrawGpuBladePrepass>();
    let draw_gpu_grass_deferred = deferred_draw_functions.read().id::<DrawGpuBladeDeferred>();
    for (view, visible_entities, has_deferred_prepass) in &views {
        let Some(view_key) = view_key_cache
            .get(&view.retained_view_entity)
            .copied()
            .filter(|key| {
                key.intersects(
                    MeshPipelineKey::NORMAL_PREPASS | MeshPipelineKey::MOTION_VECTOR_PREPASS,
                ) || has_deferred_prepass
            })
        else {
            continue; // 纯前向且没有SSAO/TAA时不增加草地预通道成本；Depth-only留给接触阴影专项评估。
        };
        let Some(visible_meshes) = visible_entities.get::<Mesh3d>() else {
            continue;
        };
        let mut prepass_phase = prepass_render_phases.get_mut(&view.retained_view_entity);
        let mut deferred_phase = deferred_render_phases.get_mut(&view.retained_view_entity);
        if has_deferred_prepass && deferred_phase.is_none() {
            render_stats.record_missing_phase_view(); // Solari/RR缺少草叶Deferred输入时必须暴露附件阶段阻塞。
            continue; // Deferred 阶段尚未为新视图建立时不退回会污染 Solari 输入的早期深度路径。
        }
        if !has_deferred_prepass && prepass_phase.is_none() {
            continue;
        }
        // Bevy 标准预通道已先统一清理共享分箱；下方只按 GpuBladePatch 所有权移除并迁移草窗，
        // 禁止在这里再次清理全体脏网格，避免覆盖刚排回的 TreeWindMaterial 时间输入表面。
        for (render_entity, main_entity) in visible_meshes.iter_visible() {
            let Ok((patch, prepared_buffers)) = grass_patches.get(*render_entity) else {
                continue;
            };
            // 当前可见草窗每帧先从普通预通道与Deferred输入阶段移除再加入目标阶段；窗口仅十二个，固定小工作量
            // 换来 DeferredPrepass 运行时切换时不依赖材质专用化脏标记。
            if let Some(phase) = prepass_phase.as_mut() {
                phase.remove(*main_entity);
            }
            if let Some(phase) = deferred_phase.as_mut() {
                phase.remove(*main_entity);
            }
            if !super::output_readiness::queue_output_is_available(
                patch.instance_count(),
                prepared_buffers.is_some(),
            )
            // Queue 早于 Prepare：只检查缓冲分配，最终 Draw 仍拒绝本帧过期输出。
            {
                continue;
            }
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                if has_deferred_prepass {
                    render_stats.record_missing_mesh_instance(); // Deferred输入路径等待主网格提取时保留F3阻塞诊断。
                } // 普通前向视图仍由主颜色排队系统统计同一等待。
                continue;
            };
            let mesh_asset_id = mesh_instance.mesh_asset_id();
            let Some(mesh) = meshes.get(mesh_asset_id) else {
                if has_deferred_prepass {
                    render_stats.record_missing_gpu_mesh(); // Deferred输入路径等待原型RenderMesh上传。
                } // 普通前向视图不重复增加同一缺失计数。
                continue;
            };
            let Some(mesh_slabs) = mesh_allocator.mesh_slabs(&mesh_asset_id) else {
                if has_deferred_prepass {
                    render_stats.record_missing_gpu_mesh(); // 共享缓冲页尚未发布时沿用GPU网格等待诊断。
                } // 普通前向视图由主通道负责同类统计。
                continue;
            };
            let pipeline_key =
                gpu_grass_temporal_guide_pipeline_key(view_key, has_deferred_prepass)
                    | MeshPipelineKey::from_primitive_topology_and_strip_index(
                        mesh.primitive_topology(),
                        mesh.index_format(),
                    );
            let Ok(pipeline) = specialized_pipelines.specialize(
                &pipeline_cache,
                &grass_pipeline,
                pipeline_key,
                &mesh.layout,
            ) else {
                if has_deferred_prepass {
                    render_stats.record_specialization_error(); // Deferred输入路径的布局错误必须在F3可见。
                } // 普通前向预通道错误不覆盖主颜色管线自身诊断。
                continue;
            };
            let batch_set_key = OpaqueNoLightmap3dBatchSetKey {
                pipeline,
                draw_function: if has_deferred_prepass {
                    draw_gpu_grass_deferred
                } else {
                    draw_gpu_grass_prepass
                },
                material_bind_group_index: None,
                slabs: mesh_slabs,
            };
            let bin_key = OpaqueNoLightmap3dBinKey {
                asset_id: mesh_asset_id.into(),
            };
            if has_deferred_prepass {
                deferred_phase.as_mut().unwrap().add(
                    batch_set_key,
                    bin_key,
                    (*render_entity, *main_entity),
                    mesh_instance.current_uniform_index,
                    BinnedRenderPhaseType::UnbatchableMesh,
                ); // Opaque地形之后覆盖可见草叶自己的GBuffer、深度、lighting id与运动矢量。
                match pipeline_cache.get_render_pipeline_state(pipeline) {
                    CachedPipelineState::Ok(_) => {} // Deferred输入已就绪但不重复计入唯一主颜色管线完整性。
                    CachedPipelineState::Queued | CachedPipelineState::Creating(_) => {
                        render_stats.record_pipeline_waiting(); // 完整GBuffer仍是活动Solari/RR的硬依赖，创建中必须阻止过早稳定。
                    } // 完成Deferred管线等待分支。
                    CachedPipelineState::Err(
                        ShaderCacheError::ShaderNotLoaded(_)
                        | ShaderCacheError::ShaderImportNotYetAvailable,
                    ) => render_stats.record_pipeline_waiting(), // Shader资产热加载未完成时下一帧安全重试。
                    CachedPipelineState::Err(_) => render_stats.record_pipeline_error(), // 编译或设备错误才归入永久失败。
                } // 完成Deferred输入管线状态诊断；主颜色计数只由后续Forward阶段发布。
            } else {
                prepass_phase.as_mut().unwrap().add(
                    batch_set_key,
                    bin_key,
                    (*render_entity, *main_entity),
                    mesh_instance.current_uniform_index,
                    BinnedRenderPhaseType::UnbatchableMesh,
                ); // 无 Deferred/Solari 的普通时间重建仍写草叶深度和运动矢量。
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_temporal_guide_writes_complete_grass_surface_targets() {
        let view_key = MeshPipelineKey::MOTION_VECTOR_PREPASS;
        let deferred_key = gpu_grass_temporal_guide_pipeline_key(view_key, true);
        let targets = gpu_grass_temporal_guide_targets(deferred_key); // 模拟真实Deferred RenderPass要求的四槽目标。

        assert!(deferred_key.contains(MeshPipelineKey::DEFERRED_PREPASS));
        assert_eq!(targets.len(), 4); // Bevy Deferred RenderPass固定绑定四个颜色槽。
        assert!(targets[0].is_none()); // 草叶时间输入不写法线预通道。
        assert_eq!(
            targets[1].as_ref().unwrap().format,
            bevy::core_pipeline::prepass::MOTION_VECTOR_PREPASS_FORMAT,
        ); // location1必须匹配真实Rg16Float运动矢量附件。
        assert_eq!(
            targets[2].as_ref().unwrap().format,
            bevy::core_pipeline::deferred::DEFERRED_PREPASS_FORMAT,
        ); // location2必须声明真实Rgba32Uint格式，不能再使用None触发wgpu验证错误。
        assert_eq!(
            targets[3].as_ref().unwrap().format,
            bevy::core_pipeline::deferred::DEFERRED_LIGHTING_PASS_ID_FORMAT,
        ); // location3必须声明真实R8Uint lighting pass id格式。
        assert!(!targets[2].as_ref().unwrap().write_mask.is_empty()); // 草叶颜色像素必须覆盖地下地形GBuffer，供Solari和RR解析一致材质。
        assert!(!targets[3].as_ref().unwrap().write_mask.is_empty()); // 草叶必须登记默认Deferred照明层，不能遗留地下地形id。
    }

    #[test]
    fn forward_temporal_guide_keeps_grass_depth_and_motion_path() {
        let view_key = MeshPipelineKey::MOTION_VECTOR_PREPASS;
        let forward_key = gpu_grass_temporal_guide_pipeline_key(view_key, false);
        let targets = gpu_grass_temporal_guide_targets(forward_key); // 模拟没有Deferred附件的普通预通道。

        assert!(!forward_key.contains(MeshPipelineKey::DEFERRED_PREPASS));
        assert!(targets[0].is_none()); // 未启用SSAO法线预通道时保持原有深度与运动最小路径。
        assert!(targets[2].is_none()); // 普通预通道不能声明RenderPass不存在的Deferred GBuffer附件。
        assert!(targets[3].is_none()); // 普通预通道不能声明RenderPass不存在的lighting pass id附件。
    }

    #[test]
    fn normal_only_prepass_matches_ssao_without_temporal_antialiasing() {
        let view_key = MeshPipelineKey::NORMAL_PREPASS;
        let targets = gpu_grass_temporal_guide_targets(view_key); // 复现FXAA或关闭AA并保留SSAO时的合法视图附件。

        assert_eq!(
            targets[0].as_ref().unwrap().format,
            bevy::core_pipeline::prepass::NORMAL_PREPASS_FORMAT,
        ); // 草叶必须覆盖自身Rgb10a2法线，禁止让地下地形法线穿过草叶深度。
        assert!(targets[1].is_none()); // 无TAA/RR时不得声明不存在的运动矢量附件。
        assert!(targets[2].is_none()); // 普通Forward视图没有Deferred GBuffer。
        assert!(targets[3].is_none()); // 普通Forward视图没有lighting pass id。
    }

    #[test]
    fn normal_and_motion_temporal_guide_matches_ssao_taa_render_pass() {
        let view_key = MeshPipelineKey::NORMAL_PREPASS | MeshPipelineKey::MOTION_VECTOR_PREPASS;
        let targets = gpu_grass_temporal_guide_targets(view_key); // 复现SSAO High与TAA并用时的真实early prepass附件。

        assert_eq!(targets.len(), 4); // Bevy以固定四槽列表表达可选Normal、Motion与Deferred目标。
        assert_eq!(
            targets[0].as_ref().unwrap().format,
            bevy::core_pipeline::prepass::NORMAL_PREPASS_FORMAT,
        ); // location0必须匹配RenderPass的Rgb10a2Unorm，避免set_pipeline验证崩溃。
        assert_eq!(
            targets[1].as_ref().unwrap().format,
            bevy::core_pipeline::prepass::MOTION_VECTOR_PREPASS_FORMAT,
        ); // location1继续匹配TAA所需的Rg16Float运动矢量。
        assert!(targets[2].is_none()); // 普通前向视图不声明Deferred GBuffer。
        assert!(targets[3].is_none()); // 普通前向视图不声明Deferred lighting id。
    }

    #[test]
    fn normal_motion_and_deferred_temporal_guide_matches_complete_render_pass() {
        let view_key = MeshPipelineKey::NORMAL_PREPASS
            | MeshPipelineKey::MOTION_VECTOR_PREPASS
            | MeshPipelineKey::DEFERRED_PREPASS;
        let targets = gpu_grass_temporal_guide_targets(view_key); // 锁定未来同时拥有Normal与Deferred附件时仍逐槽兼容。

        assert!(targets.iter().all(Option::is_some)); // 四个真实附件必须全部在pipeline中声明，禁止任一槽退回None。
        assert_eq!(
            targets[0].as_ref().unwrap().format,
            bevy::core_pipeline::prepass::NORMAL_PREPASS_FORMAT,
        ); // Normal附件继续使用Bevy固定Rgb10a2Unorm格式。
        assert_eq!(
            targets[2].as_ref().unwrap().format,
            bevy::core_pipeline::deferred::DEFERRED_PREPASS_FORMAT,
        ); // Deferred附件保持现有草叶GBuffer ABI。
    }

    #[test]
    fn prepass_targets_follow_every_supported_normal_motion_deferred_combination() {
        let combinations = [
            (MeshPipelineKey::NORMAL_PREPASS, [true, false, false, false]),
            (
                MeshPipelineKey::MOTION_VECTOR_PREPASS,
                [false, true, false, false],
            ),
            (
                MeshPipelineKey::NORMAL_PREPASS | MeshPipelineKey::MOTION_VECTOR_PREPASS,
                [true, true, false, false],
            ),
            (
                MeshPipelineKey::DEFERRED_PREPASS,
                [false, false, true, true],
            ),
            (
                MeshPipelineKey::NORMAL_PREPASS | MeshPipelineKey::DEFERRED_PREPASS,
                [true, false, true, true],
            ),
            (
                MeshPipelineKey::MOTION_VECTOR_PREPASS | MeshPipelineKey::DEFERRED_PREPASS,
                [false, true, true, true],
            ),
            (
                MeshPipelineKey::NORMAL_PREPASS
                    | MeshPipelineKey::MOTION_VECTOR_PREPASS
                    | MeshPipelineKey::DEFERRED_PREPASS,
                [true, true, true, true],
            ),
        ]; // 覆盖Bevy 0.19三个可选附件位的全部非空生产组合。

        for (key, expected_slots) in combinations {
            let targets = gpu_grass_temporal_guide_targets(key);
            assert_eq!(
                targets.iter().map(Option::is_some).collect::<Vec<_>>(),
                expected_slots,
                "GPU草叶pipeline targets必须逐槽匹配当前view key：{key:?}",
            );
        }
    }

    #[test] // 恢复Deferred视图Forward颜色后，两个预通道都只能作为输入阶段而不得重复登记主绘制。
    fn temporal_and_deferred_inputs_do_not_own_main_draw_statistics() {
        let source = include_str!("prepass.rs"); // 直接读取当前模块源码以锁定两个命令别名真实选择的泛型布尔值。
        let deferred_alias = source // 从Deferred命令声明开始截取固定别名主体。
            .split("pub(super) type DrawGpuBladeDeferred = (") // 定位唯一Deferred输入命令声明。
            .nth(1) // 取得声明之后的源码片段。
            .and_then(|suffix| suffix.split(");").next()) // 在别名结束前停止，避免命中后续实现里的其他泛型值。
            .expect("Deferred草叶绘制命令别名必须继续存在"); // 声明消失属于需要显式更新测试的架构变更。
        assert!(deferred_alias.contains("DrawGpuBladeTemporalInstanced<false>")); // Deferred只写输入，不得抢占Forward主绘制统计与查询。
        assert!(!deferred_alias.contains("DrawGpuBladeTemporalInstanced<true>")); // 永久阻止恢复双倍十二窗与九十六命令统计。
    } // 完成唯一Forward统计所有权的轻量源码级回归门。
}
