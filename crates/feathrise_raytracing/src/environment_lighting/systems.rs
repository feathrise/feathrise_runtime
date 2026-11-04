//! 活动Solari视图的环境IBL管线准备。

use bevy::{
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass}, // 环境节点必须同时拥有有效深度和GBuffer。
    pbr::{MeshPipelineKey, MeshPipelineViewLayoutKey, ViewKeyCache}, // 从真实视图键恢复两组MeshView布局。
    prelude::*,                                                      // Commands、Query与资源参数。
    render::{
        render_resource::{PipelineCache, SpecializedRenderPipelines}, // 异步专用化环境管线。
        view::ExtractedView, // 读取当前HDR目标格式和保留视图身份。
    },
};
use bevy_solari::prelude::SolariLighting; // 只处理真正启用Solari的玩法相机。

use super::pipeline::{
    SolariEnvironmentPipeline,
    SolariEnvironmentPipelineId,
    SolariEnvironmentPipelineKey, // 环境管线模板、组件与小键。
};

/// 为唯一活动Solari环境视图准备缓存管线；环境移除或资格变化时精确移除旧ID。
#[allow(clippy::type_complexity)] // RenderWorld Query同时锁定Solari、深度与Deferred三项附件资格。
pub(super) fn prepare_solari_environment_pipelines(
    mut commands: Commands,             // 只在管线身份变化时写视图组件。
    pipeline_cache: Res<PipelineCache>, // 查询或登记异步GPU管线。
    view_key_cache: Res<ViewKeyCache>,  // 使用Bevy已经结算的真实逐视图PBR键。
    pipeline: Option<Res<SolariEnvironmentPipeline>>, // 无GPU或启动尚未完成时安全跳过。
    mut pipelines: ResMut<SpecializedRenderPipelines<SolariEnvironmentPipeline>>, // 复用相同布局和格式的缓存ID。
    views: Query<
        (
            Entity,                               // RenderWorld视图实体。
            &ExtractedView,                       // HDR目标格式与保留身份。
            Option<&SolariEnvironmentPipelineId>, // 当前已安装ID用于稳定帧免写。
        ),
        (
            With<SolariLighting>,  // 无Solari辅助相机绝不进入本层。
            With<DepthPrepass>,    // 世界位置恢复需要深度预通道。
            With<DeferredPrepass>, // 材质、法线和类别标记来自GBuffer。
        ),
    >,
) {
    let Some(pipeline) = pipeline.as_deref() else {
        return; // RenderStartup资源未就绪时不制造残缺组件。
    };
    for (entity, extracted_view, current) in &views {
        let Some(view_key) = view_key_cache
            .get(&extracted_view.retained_view_entity)
            .copied()
        else {
            if current.is_some() {
                commands
                    .entity(entity)
                    .remove::<SolariEnvironmentPipelineId>(); // 视图键失效时撤销旧管线资格。
            }
            continue; // 等待Bevy下一帧完成视图准备。
        };
        let required = MeshPipelineKey::DEPTH_PREPASS
            | MeshPipelineKey::DEFERRED_PREPASS
            | MeshPipelineKey::ENVIRONMENT_MAP; // 三项缺一都不能安全采样环境GBuffer。
        let mesh_view_layout_key: MeshPipelineViewLayoutKey = view_key.into(); // 与MeshView绑定组取得相同布局小键。
        if !view_key.contains(required)
            || mesh_view_layout_key.contains(MeshPipelineViewLayoutKey::MULTISAMPLED)
        {
            if current.is_some() {
                commands
                    .entity(entity)
                    .remove::<SolariEnvironmentPipelineId>(); // 环境清除、预通道丢失或错误MSAA时停止执行。
            }
            continue; // Solari正式租约必须单采样，防御旧ID触发wgpu校验。
        }
        let desired = pipelines.specialize(
            &pipeline_cache,
            pipeline,
            SolariEnvironmentPipelineKey {
                mesh_view_layout_key,                        // 两组View布局完整进入缓存身份。
                target_format: extracted_view.target_format, // 精确匹配当前HDR附件。
            },
        );
        if current.map(|id| id.0) != Some(desired) {
            commands
                .entity(entity)
                .insert(SolariEnvironmentPipelineId(desired)); // 只在身份改变时更新视图组件。
        }
    }
}
