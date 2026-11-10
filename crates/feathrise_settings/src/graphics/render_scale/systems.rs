//! 主世界相机接管与渲染世界低分辨率视图准备。

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    camera::{MainPassResolutionOverride, RenderTarget},
    prelude::*,
    render::{
        render_resource::{CachedRenderPipelineId, PipelineCache, SpecializedRenderPipelines},
        view::ExtractedView,
    },
};

use super::pipeline::RenderScalePipeline;
use crate::{
    RenderSettings, GraphicsRuntimeLeaseRegistry, RenderScaleQuality, components::RenderScaleView,
};

/// 管线异步编译完成后附着到渲染视图；缺少该组件时空间放大系统不会翻转 ViewTarget。
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) struct RenderScalePipelineId(pub(super) CachedRenderPipelineId);

/// 将固定内部渲染比例应用到完整窗口的 3D 相机，纹理目标和自定义视口保持作者配置。
#[allow(clippy::type_complexity)] // Bevy Query 元组逐项表达相机所有权快照，拆散会隐藏同帧一致性边界。
pub(crate) fn apply_render_scale(
    mut commands: Commands,
    settings: Res<RenderSettings>,
    runtime_leases: Option<Res<GraphicsRuntimeLeaseRegistry>>, // 独立系统测试未装完整插件时等价于无租约。
    mut cameras: Query<
        (
            Entity,
            &Camera,
            &RenderTarget,
            Option<&RenderScaleView>,
            Option<&mut TemporalAntiAliasing>,
        ),
        With<Camera3d>,
    >,
) {
    let quality = settings.graphics.render_scale; // 同一帧全部窗口 3D 主视图使用统一固定比例。
    for (entity, camera, target, current, mut taa) in &mut cameras {
        let effective_quality = if runtime_leases
            .as_deref()
            .and_then(|registry| registry.requirements_for_camera(entity))
            .is_some_and(|requirements| requirements.force_native_render_scale)
        {
            RenderScaleQuality::Native // 租约期移除项目空间放大，固定使用目标完整物理尺寸。
        } else {
            quality // 非目标相机和释放帧使用最新玩家档位。
        };
        let desired = desired_view_scale(camera, target, effective_quality); // 缺窗口、最小化或专用视口时安全返回 None。
        if desired == current.copied() {
            continue; // 稳定帧不写组件、不清历史，也不触发任何视图资源重建。
        }

        match desired {
            Some(value) => {
                commands.entity(entity).insert(value); // 新相机、窗口缩放或档位变化时才更新尺寸。
            }
            None if current.is_some() => {
                commands.entity(entity).remove::<RenderScaleView>(); // 返回 100% 或改为纹理目标时归还原生路径。
            }
            None => {}
        }
        if let Some(taa) = taa.as_deref_mut()
            && !taa.reset
        {
            taa.reset = true; // 防御外部作者 TAA：尺寸变化不得复用上一分辨率历史。
        }
    }
}

/// 根据相机目标和设置推导本帧实际接管状态。
fn desired_view_scale(
    camera: &Camera,
    target: &RenderTarget,
    quality: RenderScaleQuality,
) -> Option<RenderScaleView> {
    if quality == RenderScaleQuality::Native
        || !matches!(target, RenderTarget::Window(_))
        || camera.viewport.is_some()
    {
        return None; // UI 预览、小场景和分屏/编辑器自定义视口都不进入全窗口空间放大路径。
    }
    let output_size = camera.physical_target_size()?; // 相机刚生成或窗口最小化时等待 CameraUpdateSystems 完成。
    let render_size = scaled_physical_size(output_size, quality.scale_factor())?;
    Some(RenderScaleView {
        render_size,
        output_size,
    })
}

/// 按固定比例计算最近的偶数物理像素尺寸，并严格小于完整输出尺寸。
fn scaled_physical_size(output_size: UVec2, scale: f32) -> Option<UVec2> {
    let width = scaled_dimension(output_size.x, scale)?;
    let height = scaled_dimension(output_size.y, scale)?;
    Some(UVec2::new(width, height))
}

/// 单轴采用最近偶数像素，避免奇数尺寸在 MSAA、体积效果和后处理链中产生半像素漂移。
fn scaled_dimension(output: u32, scale: f32) -> Option<u32> {
    if output < 3 || !(0.0..1.0).contains(&scale) {
        return None; // 极小窗口或 100%/非法比例继续使用原生路径。
    }
    let nearest_even = ((output as f32 * scale * 0.5).round() as u32).saturating_mul(2);
    let largest_even_below_output = if output.is_multiple_of(2) {
        output - 2
    } else {
        output - 1
    };
    Some(nearest_even.clamp(2, largest_even_below_output))
}

/// 异步管线就绪后才把主通道尺寸覆盖和放大节点同时挂到 RenderApp 视图。
#[allow(clippy::type_complexity)] // RenderWorld Query 必须原子读取缩放状态、附件覆盖与管线身份。
pub(super) fn prepare_render_scale_views(
    mut commands: Commands,
    pipeline: Option<Res<RenderScalePipeline>>,
    pipeline_cache: Res<PipelineCache>,
    mut specialized_pipelines: ResMut<SpecializedRenderPipelines<RenderScalePipeline>>,
    mut views: Query<(
        Entity,
        &ExtractedView,
        Option<&RenderScaleView>,
        Option<&mut MainPassResolutionOverride>,
        Option<&RenderScalePipelineId>,
    )>,
) {
    for (entity, extracted_view, scale, resolution_override, pipeline_id) in &mut views {
        let Some(scale) = scale else {
            remove_owned_render_scale_state(&mut commands, entity, pipeline_id.is_some());
            continue;
        };
        let Some(pipeline) = pipeline.as_deref() else {
            remove_owned_render_scale_state(&mut commands, entity, pipeline_id.is_some());
            continue; // RenderStartup 尚未建立模板时保持原生尺寸，不读取未初始化输出。
        };

        let desired_pipeline = specialized_pipelines.specialize(
            &pipeline_cache,
            pipeline,
            extracted_view.target_format,
        );
        if pipeline_cache
            .get_render_pipeline(desired_pipeline)
            .is_none()
        {
            remove_owned_render_scale_state(&mut commands, entity, pipeline_id.is_some());
            continue; // LDR/HDR 或设备变体仍在异步编译时完整渲染本帧，绝不阻塞主线程。
        }
        if resolution_override.is_some() && pipeline_id.is_none() {
            continue; // 其他渲染功能已拥有主通道尺寸覆盖时不抢占或删除其 RenderApp 状态。
        }

        match resolution_override {
            Some(mut current) if current.0 != scale.render_size => {
                current.0 = scale.render_size; // 窗口改尺寸时同帧更新主通道和预通道视口。
            }
            None => {
                commands
                    .entity(entity)
                    .insert(MainPassResolutionOverride(scale.render_size));
            }
            Some(_) => {}
        }
        if pipeline_id.map(|id| id.0) != Some(desired_pipeline) {
            commands
                .entity(entity)
                .insert(RenderScalePipelineId(desired_pipeline)); // 目标格式变化时只切换已就绪的专用化管线。
        }
    }
}

/// 以私有管线标记作为所有权凭据，只移除本功能成对附加的 RenderApp 组件。
fn remove_owned_render_scale_state(commands: &mut Commands, entity: Entity, owns_state: bool) {
    if !owns_state {
        return; // 外部 DLSS 或其他渲染功能的尺寸覆盖不得被固定比例设置误删。
    }
    commands
        .entity(entity)
        .remove::<MainPassResolutionOverride>()
        .remove::<RenderScalePipelineId>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_tiers_scene_1080p_to_even_internal_resolutions() {
        let output = UVec2::new(1920, 1080);
        assert_eq!(
            scaled_physical_size(output, RenderScaleQuality::Performance.scale_factor()),
            Some(UVec2::new(1286, 724)),
        );
        assert_eq!(
            scaled_physical_size(output, RenderScaleQuality::Balanced.scale_factor()),
            Some(UVec2::new(1440, 810)),
        );
        assert_eq!(
            scaled_physical_size(output, RenderScaleQuality::Quality.scale_factor()),
            Some(UVec2::new(1632, 918)),
        );
        assert_eq!(
            scaled_physical_size(output, RenderScaleQuality::Native.scale_factor()),
            None,
        );
    }

    #[test]
    fn tiny_or_invalid_targets_fall_back_to_native_rendering() {
        assert_eq!(scaled_physical_size(UVec2::new(2, 1080), 0.75), None);
        assert_eq!(scaled_physical_size(UVec2::new(1920, 1), 0.75), None);
        assert_eq!(scaled_physical_size(UVec2::new(1920, 1080), 1.0), None);
    }
}
