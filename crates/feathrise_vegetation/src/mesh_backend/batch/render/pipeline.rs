//! 从原具体材质已经专用化的 PBR 管线克隆批次顶点变体。

use std::collections::HashMap;

use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::render_resource::{
    CachedPipelineState, CachedRenderPipelineId, FragmentState, PipelineCache, RenderPipelineDescriptor,
    VertexAttribute, VertexFormat, VertexStepMode,
};

use crate::mesh_backend::types::VegetationMeshBatchVertexMode;

use super::shaders::{VEGETATION_MESH_BATCH_DEPTH_SHADER_HANDLE, VEGETATION_MESH_BATCH_SHADER_HANDLE};

pub(super) const VEGETATION_MESH_BATCH_INSTANCE_BYTES: u64 = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum VegetationMeshBatchPass {
    Main,
    Prepass,
    Shadow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct VegetationMeshBatchPipelineKey {
    base: CachedRenderPipelineId,
    pass: VegetationMeshBatchPass,
    vertex_mode: VegetationMeshBatchVertexMode,
}

#[derive(Default, Resource)]
pub(super) struct VegetationMeshBatchPipelines {
    pipelines: HashMap<VegetationMeshBatchPipelineKey, CachedRenderPipelineId>,
    standard_prepass_shader: Option<Handle<Shader>>,
}

fn instance_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: VEGETATION_MESH_BATCH_INSTANCE_BYTES,
        step_mode: VertexStepMode::Instance,
        attributes: vec![
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 0,
                shader_location: 8,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 16,
                shader_location: 9,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 32,
                shader_location: 10,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 48,
                shader_location: 11,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 64,
                shader_location: 12,
            },
            VertexAttribute {
                format: VertexFormat::Uint32x4,
                offset: 80,
                shader_location: 13,
            },
        ],
    }
}

fn batch_descriptor(
    mut descriptor: RenderPipelineDescriptor,
    pass: VegetationMeshBatchPass,
    vertex_mode: VegetationMeshBatchVertexMode,
    standard_prepass_shader: Option<&Handle<Shader>>,
) -> Option<RenderPipelineDescriptor> {
    if descriptor.vertex.buffers.len() != 1 {
        return None; // 批次正式原型只接受普通非蒙皮、非变形目标 Mesh。
    }
    descriptor.label = Some(match pass {
        VegetationMeshBatchPass::Main => "vegetation_mesh_batch_main".into(),
        VegetationMeshBatchPass::Prepass => "vegetation_mesh_batch_prepass".into(),
        VegetationMeshBatchPass::Shadow => "vegetation_mesh_batch_shadow".into(),
    });
    descriptor.vertex.shader = VEGETATION_MESH_BATCH_SHADER_HANDLE.clone();
    descriptor
        .vertex
        .shader_defs
        .push("VEGETATION_MESH_BATCH".into());
    if pass != VegetationMeshBatchPass::Main {
        descriptor
            .vertex
            .shader_defs
            .push("VEGETATION_BATCH_PREPASS".into());
        let dither = descriptor.vertex.shader_defs.contains(&"VISIBILITY_RANGE_DITHER".into());
        let depth_only = descriptor.fragment.as_ref().is_none_or(|fragment| {
            !fragment.shader_defs.contains(&"PREPASS_FRAGMENT".into())
                && standard_prepass_shader == Some(&fragment.shader)
        });
        if dither && depth_only {
            // Bevy 0.19.1纯深度PBR分支只做AlphaMask，Opaque甚至没有片元阶段。
            // 为这两种官方分支补齐逐株LOD裁切，避免颜色透明后仍用完整叶片深度挡住背景。
            let fragment = descriptor.fragment.get_or_insert_with(|| FragmentState {
                shader: VEGETATION_MESH_BATCH_DEPTH_SHADER_HANDLE.clone(),
                shader_defs: descriptor.vertex.shader_defs.clone(),
                targets: Vec::new(),
                ..default()
            });
            fragment.shader = VEGETATION_MESH_BATCH_DEPTH_SHADER_HANDLE.clone();
        } // 法线、运动矢量、Deferred、深度裁面模拟和任意自定义片元继续复用原管线。
    }
    match vertex_mode {
        VegetationMeshBatchVertexMode::Static => {}
        VegetationMeshBatchVertexMode::Wind => descriptor
            .vertex
            .shader_defs
            .push("VEGETATION_BATCH_WIND".into()),
        VegetationMeshBatchVertexMode::Billboard => descriptor
            .vertex
            .shader_defs
            .push("VEGETATION_BATCH_BILLBOARD".into()),
    }
    descriptor.vertex.buffers.push(instance_layout());
    Some(descriptor)
}

impl VegetationMeshBatchPipelines {
    pub(super) fn new(standard_prepass_shader: Option<Handle<Shader>>) -> Self {
        Self { standard_prepass_shader, ..default() }
    }
    pub(super) fn get_or_queue(
        &mut self,
        pipeline_cache: &PipelineCache,
        base: CachedRenderPipelineId,
        pass: VegetationMeshBatchPass,
        vertex_mode: VegetationMeshBatchVertexMode,
    ) -> Option<CachedRenderPipelineId> {
        let key = VegetationMeshBatchPipelineKey {
            base,
            pass,
            vertex_mode,
        };
        if let Some(pipeline) = self.pipelines.get(&key) {
            return Some(*pipeline);
        }
        if matches!(
            pipeline_cache.get_render_pipeline_state(base),
            CachedPipelineState::Queued
        ) {
            return None; // 基础材质描述符尚未进入可读缓存，下一帧再克隆。
        }
        let descriptor = batch_descriptor(
            pipeline_cache.get_render_pipeline_descriptor(base).clone(),
            pass,
            vertex_mode,
            self.standard_prepass_shader.as_ref(),
        )?;
        let pipeline = pipeline_cache.queue_render_pipeline(descriptor);
        self.pipelines.insert(key, pipeline);
        Some(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::uuid_handle;

    const STANDARD: Handle<Shader> = uuid_handle!("f422e884-5b0f-4405-9d36-1e0b8a06a7c1");
    const CUSTOM: Handle<Shader> = uuid_handle!("91bd2811-2c50-4327-83c4-a38052f8aefa");

    fn descriptor(fragment: Option<FragmentState>) -> RenderPipelineDescriptor {
        let mut descriptor = RenderPipelineDescriptor { fragment, ..default() };
        descriptor.vertex.buffers.push(VertexBufferLayout {
            array_stride: 12, step_mode: VertexStepMode::Vertex, attributes: vec![],
        });
        descriptor.vertex.shader_defs.push("VISIBILITY_RANGE_DITHER".into());
        descriptor
    }

    #[test]
    fn pure_depth_fade_discards_both_opaque_and_alpha_mask_including_shadows() {
        for pass in [VegetationMeshBatchPass::Prepass, VegetationMeshBatchPass::Shadow] {
            for alpha_mask in [false, true] {
                let original = alpha_mask.then(|| FragmentState { shader: STANDARD,
                    shader_defs: vec!["VISIBILITY_RANGE_DITHER".into(), "MAY_DISCARD".into()], ..default() });
                let repaired = batch_descriptor(descriptor(original), pass,
                    VegetationMeshBatchVertexMode::Static, Some(&STANDARD)).unwrap();
                let fragment = repaired.fragment.expect("纯深度渐变也必须执行片元裁切");
                assert_eq!(fragment.shader, VEGETATION_MESH_BATCH_DEPTH_SHADER_HANDLE);
                assert!(fragment.shader_defs.contains(&"VISIBILITY_RANGE_DITHER".into()));
                assert_eq!(fragment.shader_defs.contains(&"MAY_DISCARD".into()), alpha_mask);
                assert!(fragment.targets.is_empty()); // 不为纯深度新增颜色、法线或运动附件。
            }
        }
    }

    #[test]
    fn full_prepass_custom_fragment_and_main_keep_their_original_material_outputs() {
        for (pass, shader, full_prepass) in [
            (VegetationMeshBatchPass::Prepass, STANDARD, true),
            (VegetationMeshBatchPass::Shadow, CUSTOM, false),
            (VegetationMeshBatchPass::Main, STANDARD, false),
        ] {
            let mut original = FragmentState { shader: shader.clone(), ..default() };
            if full_prepass { original.shader_defs.push("PREPASS_FRAGMENT".into()); }
            let repaired = batch_descriptor(descriptor(Some(original)), pass,
                VegetationMeshBatchVertexMode::Wind, Some(&STANDARD)).unwrap();
            assert_eq!(repaired.fragment.unwrap().shader, shader); // 自定义裁切和GBuffer/历史输出不能被兼容补丁覆盖。
        }
    }
}
