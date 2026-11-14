//! 固定烘焙槽位的真实渲染回执：字形、图集与最终输出都绘制并提交后，主世界才能释放源。

use std::sync::atomic::Ordering;

use bevy::{
    camera::{CameraOutputMode, NormalizedRenderTarget},
    core_pipeline::{
        blit::{BlitPipeline, BlitPipelineKey},
        core_2d::{Opaque2d, Transparent2d},
        schedule::Core2d,
        upscaling::{ViewUpscalingPipeline, upscaling},
    },
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        camera::ExtractedCamera,
        render_asset::RenderAssets,
        render_phase::{ViewBinnedRenderPhases, ViewSortedRenderPhases},
        render_resource::{BlendState, PipelineCache, SpecializedRenderPipelines},
        renderer::ViewQuery,
        sync_world::{MainEntity, RenderEntity},
        texture::GpuImage,
        view::{
            ExtractedView, ResolvedCompositingSpace, ViewDepthStencilTexture, ViewTarget,
            ViewUniformOffset,
        },
    },
    sprite::Text2dShadow,
    sprite_render::{ExtractedSpriteKind, ExtractedSprites, SpriteBatches, SpriteViewBindGroup},
    text::TextLayoutInfo,
};

use crate::{
    components::BillboardTextLink,
    resources::{BillboardBakePool, BillboardBakeRenderJobs},
    types::BillboardBakeRenderJob,
};

pub(crate) fn configure_bake_render_receipts(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<BillboardBakeRenderJobs>()
        .add_systems(ExtractSchedule, extract_bake_jobs)
        .add_systems(Core2d, record_bake_draw.after(upscaling))
        .add_systems(
            Render,
            publish_bake_receipts
                .after(RenderSystems::Render)
                .before(RenderSystems::Cleanup),
        );
}

/// 只复制最多四个活跃任务的身份与源清单；已完成标签不参与稳定帧检查。
fn extract_bake_jobs(
    mut jobs: ResMut<BillboardBakeRenderJobs>,
    pool: Extract<Res<BillboardBakePool>>,
    links: Extract<Query<&BillboardTextLink>>,
    cameras: Extract<Query<&RenderEntity>>,
    layouts: Extract<Query<(&TextLayoutInfo, Has<Text2dShadow>)>>,
) {
    jobs.0.clear();
    for slot in &pool.slots {
        let Some(billboard) = slot.billboard else {
            continue;
        };
        let Ok(link) = links.get(billboard) else {
            continue;
        };
        if link.needs_layout_sync || link.bake_ready {
            continue;
        }
        let (Some(text), Some(spec)) = (link.text_entity, link.bake_spec.as_ref()) else {
            continue;
        };
        let Ok(camera) = cameras.get(slot.camera_entity) else {
            continue;
        };
        let text_sources: Option<Vec<_>> = std::iter::once(text)
            .chain(link.outline_entities.iter().copied())
            .map(|entity| {
                let (layout, shadow) = layouts.get(entity).ok()?;
                // 不裁切的标签也必须等待字体排版；不能把尚未加载的非空文字当作空任务提交。
                if layout.size.y <= 0.0 && !spec.text.trim().is_empty() {
                    return None;
                }
                Some((entity, layout.glyphs.len() * if shadow { 2 } else { 1 }))
            })
            .collect();
        let Some(text_sources) = text_sources else {
            continue;
        };
        jobs.0.push(BillboardBakeRenderJob {
            camera_entity: camera.id(),
            image: link.image.id(),
            generation: slot.generation,
            rendered_generation: slot.rendered_generation.clone(),
            text_sources,
            decorations: link.decoration_entities.clone(),
            draw_encoded: false,
        });
    }
}

/// 在该相机实际执行Core2d并完成输出之后核验；仅有排版或排队结果不能证明已经画出字。
fn record_bake_draw(
    view: ViewQuery<(
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewUpscalingPipeline,
        &ViewDepthStencilTexture,
        &ViewUniformOffset,
        &SpriteViewBindGroup,
        Option<&ResolvedCompositingSpace>,
    )>,
    mut jobs: ResMut<BillboardBakeRenderJobs>,
    transparent: Res<ViewSortedRenderPhases<Transparent2d>>,
    opaque: Res<ViewBinnedRenderPhases<Opaque2d>>,
    cache: Res<PipelineCache>,
    mut blit_pipelines: ResMut<SpecializedRenderPipelines<BlitPipeline>>,
    blit: Res<BlitPipeline>,
    sprites: Res<ExtractedSprites>,
    batches: Res<SpriteBatches>,
    images: Res<RenderAssets<GpuImage>>,
) {
    let Some(job) = jobs
        .0
        .iter_mut()
        .find(|job| job.camera_entity == view.entity())
    else {
        return;
    };
    let (camera, extracted_view, target, _, _, _, _, space) = view.into_inner();
    let Some(NormalizedRenderTarget::Image(image)) = &camera.target else {
        return;
    };
    if image.handle.id() != job.image || images.get(job.image).is_none() || camera.hdr {
        return;
    }
    let retained = extracted_view.retained_view_entity;
    let Some(phase) = transparent.get(&retained) else {
        return;
    };

    // Image目标使用公开的普通blit专用化键；不依赖工程私有的窗口输出扩展API。
    // 字形可画不代表最终Image已写入，首次blit管线未就绪时同样不能发出成功回执。
    let Some(format) = target.out_texture_view_format() else {
        return;
    };
    let CameraOutputMode::Write { blend_state, .. } = camera.output_mode else {
        return;
    };
    let output_pipeline = blit_pipelines.specialize(
        &cache,
        &blit,
        BlitPipelineKey {
            target_format: format,
            blend_state: blend_state.or_else(|| {
                (camera.sorted_camera_index_for_target > 0).then_some(BlendState::ALPHA_BLENDING)
            }),
            samples: 1,
            source_space: ResolvedCompositingSpace::space(space),
        },
    );
    if cache.get_render_pipeline(output_pipeline).is_none()
        || phase
            .items
            .values()
            .any(|item| cache.get_render_pipeline(item.pipeline).is_none())
    {
        return;
    }

    // 与Bevy的sprite合批边界一致：后续字形可由前一项的batch绘制，不能要求每项独立有batch。
    let mut batch_image = None;
    for item in phase.items.values() {
        let Some(sprite) = sprites
            .sprites
            .get(item.extracted_index)
            .filter(|sprite| sprite.render_entity == item.entity.0)
        else {
            batch_image = None;
            continue;
        };
        if images.get(sprite.image_handle_id).is_none() {
            return;
        }
        if batches.contains_key(&(retained, item.entity.0)) && !item.batch_range.is_empty() {
            batch_image = Some(sprite.image_handle_id);
        }
        if batch_image != Some(sprite.image_handle_id) {
            return;
        }
    }
    for &(source, expected_glyphs) in &job.text_sources {
        let actual_glyphs: usize = phase
            .items
            .values()
            .filter(|item| item.entity.1 == source.into())
            .filter_map(|item| sprites.sprites.get(item.extracted_index))
            .map(|sprite| match &sprite.kind {
                ExtractedSpriteKind::Slices { indices } => indices.len(),
                _ => 0,
            })
            .sum();
        if actual_glyphs < expected_glyphs {
            return;
        }
    }
    // 装饰可能进入透明或不透明2D阶段；必须连同文字完整提交，不能提前发布半枚徽标。
    for &decoration in &job.decorations {
        let decoration = MainEntity::from(decoration);
        let ready_transparent = phase.items.values().any(|item| item.entity.1 == decoration);
        let ready_opaque = opaque.get(&retained).is_some_and(|phase| {
            phase.batchable_meshes.iter().any(|((_, key), bin)| {
                bin.entities().contains_key(&decoration)
                    && cache.get_render_pipeline(key.pipeline).is_some()
            }) || phase.unbatchable_meshes.iter().any(|((_, key), bin)| {
                bin.entities.contains_key(&decoration)
                    && cache.get_render_pipeline(key.pipeline).is_some()
            })
        });
        if !ready_transparent && !ready_opaque {
            return;
        }
    }
    job.draw_encoded = true;
}

/// Render系统完成队列提交之后才发布代次；主世界Acquire读取，无GPU等待、回读或像素扫描。
fn publish_bake_receipts(jobs: Res<BillboardBakeRenderJobs>) {
    for job in &jobs.0 {
        if job.draw_encoded {
            job.rendered_generation
                .store(job.generation, Ordering::Release);
        }
    }
}
