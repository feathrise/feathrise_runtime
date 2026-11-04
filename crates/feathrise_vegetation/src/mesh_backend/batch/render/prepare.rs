//! 把当前变化修订的逐实例矩阵上传到只读顶点流，并维护批次节点共享 MeshUniform 表。

use bevy::pbr::{
    MeshFlags, MeshPipeline, MeshTransforms, MeshUniform, PreparedMaterial,
    RenderMaterialInstances, RenderMeshInstances,
};
use bevy::prelude::*;
use bevy::render::erased_render_asset::ErasedRenderAssets;
use bevy::render::mesh::{RenderMesh, allocator::MeshAllocator};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    BindGroup, Buffer, BufferInitDescriptor, BufferUsages, GpuArrayBuffer, PipelineCache,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct VolumeUploads {
    pending: HashMap<Entity, VolumeUpload>,
}
struct VolumeUpload {
    identity: (usize, u32, u32, u64),
    task: Option<Task<Vec<VegetationMeshBatchGpuInstance>>>,
    data: Option<Vec<VegetationMeshBatchGpuInstance>>,
    buffer: Option<Buffer>,
    offset: usize,
}

use crate::mesh_backend::batch::components::{VegetationMeshBatchInstances, VegetationMeshBatchFrame};
use crate::mesh_backend::types::VegetationMeshBatchVertexMode;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct VegetationMeshBatchGpuInstance {
    world_from_local: [[f32; 4]; 4],
    lod_range: [f32; 4],
    metadata: [u32; 4],
}

/// RenderApp 中一个批次节点当前可直接绘制的 GPU 前缀。
#[derive(Component)]
pub(super) struct VegetationMeshBatchPrepared {
    pub(super) buffer: Buffer,
    pub(super) instance_count: u32,
    pub(super) source_revision: u64,
    pub(super) mesh_uniform_index: u32,
    pub(super) mesh_dynamic_offset: Option<u32>,
    pub(super) vertex_mode: VegetationMeshBatchVertexMode,
}

/// 全部批次节点共享一份与 Bevy `mesh_bindings.wgsl` ABI 相同的模型表。
#[derive(Resource)]
pub(super) struct VegetationMeshBatchUniforms {
    values: GpuArrayBuffer<MeshUniform>,
    pub(super) bind_group: Option<BindGroup>,
}

pub(super) fn init_vegetation_mesh_batch_uniforms(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
) {
    commands.insert_resource(VegetationMeshBatchUniforms {
        values: GpuArrayBuffer::new(&render_device.limits()),
        bind_group: None,
    });
}

fn gpu_instances(
    source: &VegetationMeshBatchInstances,
    mesh_uniform_index: u32,
) -> Vec<VegetationMeshBatchGpuInstance> {
    source
        .instances
        .iter()
        .map(|instance| VegetationMeshBatchGpuInstance {
            world_from_local: instance.world_from_local.to_cols_array_2d(),
            lod_range: instance.lod_range.to_array(),
            metadata: [
                mesh_uniform_index,
                u32::from(instance.determinant_positive),
                0,
                0,
            ],
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_vegetation_mesh_batches(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    mesh_pipeline: Res<MeshPipeline>,
    render_mesh_instances: Res<RenderMeshInstances>,
    render_material_instances: Res<RenderMaterialInstances>,
    render_materials: Res<ErasedRenderAssets<PreparedMaterial>>,
    meshes: Res<RenderAssets<RenderMesh>>,
    mesh_allocator: Res<MeshAllocator>,
    mut uniforms: ResMut<VegetationMeshBatchUniforms>,
    mut volume_uploads: Local<VolumeUploads>,
    batches: Query<(
        Entity,
        &MainEntity,
        &VegetationMeshBatchInstances,
        &VegetationMeshBatchFrame,
        Option<&VegetationMeshBatchPrepared>,
        &super::super::components::VegetationMeshBatchNode,
    )>,
) {
    uniforms.values.clear();
    volume_uploads
        .pending
        .retain(|entity, _| batches.get(*entity).is_ok());
    let mut upload_budget = 8 * 1024 * 1024usize; // 同帧所有新增三维Mesh节点共享8MiB上传信用。
    for (render_entity, main_entity, source, frame, prepared, node) in &batches {
        if source.instances.is_empty() && source.volume.as_ref().is_none_or(|v| v.cells.is_empty())
        {
            if prepared.is_some() {
                commands
                    .entity(render_entity)
                    .remove::<VegetationMeshBatchPrepared>();
            }
            continue;
        }
        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity) else {
            continue;
        };
        let mesh_asset_id = mesh_instance.mesh_asset_id();
        let Some(gpu_mesh) = meshes.get(mesh_asset_id) else {
            continue;
        };
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
            continue;
        };
        let Some(material_instance) = render_material_instances.instances.get(main_entity) else {
            continue;
        };
        let Some(material) = render_materials.get(material_instance.asset_id) else {
            continue;
        };
        let flags = (MeshFlags::LOD_INDEX_MASK
            | MeshFlags::SHADOW_RECEIVER
            | MeshFlags::SIGN_DETERMINANT_MODEL_3X3)
            .bits();
        let mesh_uniform = MeshUniform::new(
            &MeshTransforms {
                world_from_local: frame.0,
                previous_world_from_local: frame.0,
                flags,
            },
            vertex_slice.range.start,
            material.binding.slot,
            None,
            None,
            None,
            None,
        );
        let mut uniform_index = uniforms.values.push(mesh_uniform.clone());
        let mut part_uniform_index = 0;
        if source.volume.is_some() {
            let part_frame = node.local_transform.compute_affine().into();
            let part_uniform = MeshUniform::new(
                &MeshTransforms {
                    world_from_local: part_frame,
                    previous_world_from_local: part_frame,
                    flags,
                },
                vertex_slice.range.start,
                material.binding.slot,
                None,
                None,
                None,
                None,
            );
            let part_index = uniforms.values.push(part_uniform);
            part_uniform_index = part_index.index;
            if part_index.dynamic_offset != uniform_index.dynamic_offset {
                // Uniform后端跨块时把根复制到新块，保证两份矩阵能由同一次绑定读取。
                uniform_index = uniforms.values.push(mesh_uniform);
                debug_assert_eq!(part_index.dynamic_offset, uniform_index.dynamic_offset);
            }
        }
        let mesh_uniform_index = uniform_index.index;
        let mesh_dynamic_offset = uniform_index.dynamic_offset.map(|offset| offset.get());
        let needs_upload = prepared.is_none_or(|prepared| {
            prepared.source_revision != source.revision
                || prepared.mesh_uniform_index != mesh_uniform_index
                || prepared.mesh_dynamic_offset != mesh_dynamic_offset
                || prepared.vertex_mode != source.vertex_mode
        });
        if !needs_upload {
            continue;
        }
        if let Some(volume) = &source.volume {
            if prepared.is_some() {
                commands
                    .entity(render_entity)
                    .remove::<VegetationMeshBatchPrepared>();
            } // Uniform索引换代时旧顶点ABI不得继续引用已复用的实例槽。
            let identity = (
                std::sync::Arc::as_ptr(&volume.cells) as *const () as usize,
                mesh_uniform_index,
                part_uniform_index,
                source.revision,
            );
            if volume_uploads
                .pending
                .get(&render_entity)
                .is_some_and(|p| p.identity != identity)
            {
                volume_uploads.pending.remove(&render_entity);
            }
            if !volume_uploads.pending.contains_key(&render_entity)
                && volume_uploads.pending.len() < 2
            {
                let volume = volume.clone();
                let range = node.authored_range.clone();
                let positive = node.local_transform.scale.element_product() > 0.0;
                // 这里只冻结作者格；完整上传ABI打包放在两个有界计算槽。旋转/散布仍由GPU完成。
                let task = AsyncComputeTaskPool::get().spawn(async move {
                    volume
                        .cells
                        .iter()
                        .map(|cell| VegetationMeshBatchGpuInstance {
                            world_from_local: [
                                cell[0],
                                cell[1],
                                [
                                    volume.spacing,
                                    volume.scale_range[0],
                                    volume.scale_range[1],
                                    volume.ground_embed_depth,
                                ],
                                cell[2],
                            ],
                            lod_range: [
                                range.start_margin.start,
                                range.start_margin.end,
                                range.end_margin.start,
                                range.end_margin.end,
                            ],
                            metadata: [
                                mesh_uniform_index,
                                u32::from(positive),
                                0x80000000 | part_uniform_index,
                                volume.seed,
                            ],
                        })
                        .collect()
                });
                volume_uploads.pending.insert(
                    render_entity,
                    VolumeUpload {
                        identity,
                        task: Some(task),
                        data: None,
                        buffer: None,
                        offset: 0,
                    },
                );
            }
            let Some(pending) = volume_uploads.pending.get_mut(&render_entity) else {
                continue;
            };
            if let Some(data) = pending.task.as_mut().and_then(check_ready) {
                pending.task = None;
                pending.data = Some(data);
            }
            let Some(data) = pending.data.as_ref() else {
                continue;
            };
            let bytes = bytemuck::cast_slice(data);
            let buffer = pending.buffer.get_or_insert_with(|| {
                render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
                    label: Some("volume_mesh_author_cells"),
                    size: bytes.len() as u64,
                    usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            let count = (bytes.len() - pending.offset).min(upload_budget);
            if count > 0 {
                render_queue.write_buffer(
                    buffer,
                    pending.offset as u64,
                    &bytes[pending.offset..pending.offset + count],
                );
                pending.offset += count;
                upload_budget -= count;
            }
            if pending.offset == bytes.len() {
                commands
                    .entity(render_entity)
                    .insert(VegetationMeshBatchPrepared {
                        buffer: buffer.clone(),
                        instance_count: data.len() as u32,
                        source_revision: source.revision,
                        mesh_uniform_index,
                        mesh_dynamic_offset,
                        vertex_mode: source.vertex_mode,
                    });
                volume_uploads.pending.remove(&render_entity);
            } else if prepared.is_some() {
                commands
                    .entity(render_entity)
                    .remove::<VegetationMeshBatchPrepared>();
            }
            continue;
        }
        let instances = gpu_instances(source, mesh_uniform_index);
        debug_assert_eq!(
            instances.len(),
            source
                .volume
                .as_ref()
                .map_or(source.instances.len(), |v| v.cells.len())
        );
        let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("vegetation_mesh_batch_instances"),
            contents: bytemuck::cast_slice(&instances),
            usage: BufferUsages::VERTEX,
        });
        commands
            .entity(render_entity)
            .insert(VegetationMeshBatchPrepared {
                buffer,
                instance_count: instances.len().min(u32::MAX as usize) as u32,
                source_revision: source.revision,
                mesh_uniform_index,
                mesh_dynamic_offset,
                vertex_mode: source.vertex_mode,
            });
        let _ = gpu_mesh; // GPU资产存在性是上传与后续绘制的共同门。
    }
    uniforms.values.write_buffer(&render_device, &render_queue);
    uniforms.bind_group = uniforms.values.binding().map(|binding| {
        mesh_pipeline
            .mesh_layouts
            .model_only(&render_device, &pipeline_cache, &binding)
    });
}
