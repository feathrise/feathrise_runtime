//! 固定四阶段的精确缓存状态；只读四个已登记ID，不扫描全局管线或触发同步编译。

use bevy::{prelude::World, render::render_resource::{CachedPipelineState,PipelineCache}, shader::ShaderCacheError};
use crate::resources::GpuBladeComputePipeline;

/// RenderApp只读诊断，顺序为cull_tiles/finalize_tiles/classify_and_compact/finalize_indirect。
/// 状态码：0未安装、1排队、2创建中、3可用、4缺Shader、5缺import、6永久错误。
/// 这些状态仅解释等待原因，不能代替四阶段完整就绪及真实GPU回读验收。
pub fn gpu_blade_compute_pipeline_states(world:&World)->[u8;4] {
    let (Some(pipeline),Some(cache))=(world.get_resource::<GpuBladeComputePipeline>(),world.get_resource::<PipelineCache>())
        else {return [0;4];};
    [pipeline.cull_tiles_pipeline,pipeline.finalize_tiles_pipeline,pipeline.classify_pipeline,pipeline.finalize_pipeline]
        .map(|id|state_code(cache.get_compute_pipeline_state(id)))
}

fn state_code(state:&CachedPipelineState)->u8 {
    match state {
        CachedPipelineState::Queued=>1,
        CachedPipelineState::Creating(_)=>2,
        CachedPipelineState::Ok(_)=>3,
        CachedPipelineState::Err(ShaderCacheError::ShaderNotLoaded(_))=>4,
        CachedPipelineState::Err(ShaderCacheError::ShaderImportNotYetAvailable)=>5,
        CachedPipelineState::Err(_)=>6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_pipeline_and_retryable_shader_waits_are_distinct_from_permanent_failure() {
        assert_eq!(gpu_blade_compute_pipeline_states(&World::new()),[0;4]);
        assert_eq!(state_code(&CachedPipelineState::Queued),1);
        assert_eq!(state_code(&CachedPipelineState::Err(ShaderCacheError::ShaderNotLoaded(bevy::prelude::Handle::<bevy::prelude::Shader>::default().id()))),4);
        assert_eq!(state_code(&CachedPipelineState::Err(ShaderCacheError::ShaderImportNotYetAvailable)),5);
        assert_eq!(state_code(&CachedPipelineState::Err(ShaderCacheError::CreateShaderModule("diagnostic fixture".into()))),6);
    }
}
