//! 分块 Mesh 实例后端的 RenderApp 装配。

mod commands;
mod pipeline;
mod prepare;
mod queue;
mod shaders;

use bevy::app::SubApp;
use bevy::core_pipeline::{
    core_3d::{AlphaMask3d, Opaque3d},
    deferred::{AlphaMask3dDeferred, Opaque3dDeferred},
    prepass::{AlphaMask3dPrepass, Opaque3dPrepass},
};
use bevy::pbr::{
    MeshPipelineSystems, Shadow, queue_material_meshes, queue_prepass_material_meshes,
    queue_shadows,
};
use bevy::prelude::IntoScheduleConfigs;
use bevy::render::{Render, RenderStartup, RenderSystems, render_phase::AddRenderCommand};

pub(super) use shaders::register_batch_shaders;

use commands::{
    DrawVegetationMeshBatch, DrawVegetationMeshBatchPrepass,
    DrawVegetationMeshBatchPrepassDepthOnly,
};
use pipeline::VegetationMeshBatchPipelines;
use prepare::{init_vegetation_mesh_batch_uniforms, prepare_vegetation_mesh_batches};
use queue::{
    queue_vegetation_mesh_batch_prepasses, queue_vegetation_mesh_batch_shadows,
    queue_vegetation_mesh_batches,
};

pub(super) fn install_batch_render_app(render_app: &mut SubApp, standard_prepass_shader: Option<bevy::prelude::Handle<bevy::prelude::Shader>>) {
    render_app
        .add_render_command::<Opaque3d, DrawVegetationMeshBatch>()
        .add_render_command::<AlphaMask3d, DrawVegetationMeshBatch>()
        .add_render_command::<Opaque3dPrepass, DrawVegetationMeshBatchPrepass>()
        .add_render_command::<Opaque3dPrepass, DrawVegetationMeshBatchPrepassDepthOnly>()
        .add_render_command::<AlphaMask3dPrepass, DrawVegetationMeshBatchPrepass>()
        .add_render_command::<AlphaMask3dPrepass, DrawVegetationMeshBatchPrepassDepthOnly>()
        .add_render_command::<Opaque3dDeferred, DrawVegetationMeshBatchPrepass>()
        .add_render_command::<Opaque3dDeferred, DrawVegetationMeshBatchPrepassDepthOnly>()
        .add_render_command::<AlphaMask3dDeferred, DrawVegetationMeshBatchPrepass>()
        .add_render_command::<AlphaMask3dDeferred, DrawVegetationMeshBatchPrepassDepthOnly>()
        .add_render_command::<Shadow, DrawVegetationMeshBatchPrepass>()
        .add_render_command::<Shadow, DrawVegetationMeshBatchPrepassDepthOnly>()
        .insert_resource(VegetationMeshBatchPipelines::new(standard_prepass_shader))
        .add_systems(
            RenderStartup,
            init_vegetation_mesh_batch_uniforms.after(MeshPipelineSystems),
        )
        .add_systems(
            Render,
            (
                prepare_vegetation_mesh_batches.in_set(RenderSystems::PrepareBindGroups),
                queue_vegetation_mesh_batches
                    .in_set(RenderSystems::QueueMeshes)
                    .after(queue_material_meshes),
                queue_vegetation_mesh_batch_prepasses
                    .in_set(RenderSystems::QueueMeshes)
                    .after(queue_prepass_material_meshes),
                queue_vegetation_mesh_batch_shadows
                    .in_set(RenderSystems::QueueMeshes)
                    .after(queue_shadows),
            ),
        );
}
