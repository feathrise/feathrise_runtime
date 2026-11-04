//! 批次 PBR、预通道与阴影阶段共用的绑定和实例绘制命令。

use bevy::ecs::system::{SystemParamItem, lifetimeless::*};
use bevy::pbr::{
    RenderMeshInstances, SetMaterialBindGroup, SetMeshViewBindGroup,
    SetMeshViewBindingArrayBindGroup, SetPrepassEmptyMaterialBindGroup, SetPrepassViewBindGroup,
    SetPrepassViewEmptyBindGroup,
};
use bevy::render::mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    PhaseItem, RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
};

use super::prepare::{VegetationMeshBatchPrepared, VegetationMeshBatchUniforms};

pub(super) type DrawVegetationMeshBatch = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetVegetationMeshBatchUniforms<2>,
    SetMaterialBindGroup<3>,
    DrawVegetationMeshBatchInstances,
);

pub(super) type DrawVegetationMeshBatchPrepass = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    SetVegetationMeshBatchUniforms<2>,
    SetMaterialBindGroup<3>,
    DrawVegetationMeshBatchInstances,
);

pub(super) type DrawVegetationMeshBatchPrepassDepthOnly = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    SetVegetationMeshBatchUniforms<2>,
    SetPrepassEmptyMaterialBindGroup<3>,
    DrawVegetationMeshBatchInstances,
);

pub(super) struct SetVegetationMeshBatchUniforms<const I: usize>;

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetVegetationMeshBatchUniforms<I> {
    type Param = SRes<VegetationMeshBatchUniforms>;
    type ViewQuery = ();
    type ItemQuery = Read<VegetationMeshBatchPrepared>;

    fn render<'w>(
        _item: &P,
        _view: (),
        prepared: Option<&'w VegetationMeshBatchPrepared>,
        uniforms: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(prepared), Some(bind_group)) =
            (prepared, uniforms.into_inner().bind_group.as_ref())
        else {
            return RenderCommandResult::Skip;
        };
        if let Some(dynamic_offset) = prepared.mesh_dynamic_offset {
            pass.set_bind_group(I, bind_group, &[dynamic_offset]);
        } else {
            pass.set_bind_group(I, bind_group, &[]);
        }
        RenderCommandResult::Success
    }
}

pub(super) struct DrawVegetationMeshBatchInstances;

impl<P: PhaseItem> RenderCommand<P> for DrawVegetationMeshBatchInstances {
    type Param = (
        SRes<RenderAssets<RenderMesh>>,
        SRes<RenderMeshInstances>,
        SRes<MeshAllocator>,
    );
    type ViewQuery = ();
    type ItemQuery = Read<VegetationMeshBatchPrepared>;

    fn render<'w>(
        item: &P,
        _view: (),
        prepared: Option<&'w VegetationMeshBatchPrepared>,
        (meshes, mesh_instances, mesh_allocator): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(prepared) = prepared else {
            return RenderCommandResult::Skip;
        };
        let Some(mesh_instance) = mesh_instances
            .into_inner()
            .render_mesh_queue_data(item.main_entity())
        else {
            return RenderCommandResult::Skip;
        };
        let mesh_asset_id = mesh_instance.mesh_asset_id();
        let Some(mesh) = meshes.into_inner().get(mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };
        let mesh_allocator = mesh_allocator.into_inner();
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };
        pass.set_vertex_buffer(0, vertex_slice.buffer.slice(..));
        pass.set_vertex_buffer(1, prepared.buffer.slice(..));
        match &mesh.buffer_info {
            RenderMeshBufferInfo::Indexed { index_format, .. } => {
                let Some(index_slice) = mesh_allocator.mesh_index_slice(&mesh_asset_id) else {
                    return RenderCommandResult::Skip;
                };
                pass.set_index_buffer(index_slice.buffer.slice(..), *index_format);
                pass.draw_indexed(
                    index_slice.range.clone(),
                    vertex_slice.range.start as i32,
                    0..prepared.instance_count,
                );
            }
            RenderMeshBufferInfo::NonIndexed => {
                pass.draw(vertex_slice.range.clone(), 0..prepared.instance_count);
            }
        }
        RenderCommandResult::Success
    }
}
