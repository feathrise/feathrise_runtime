// 游戏设置 — 纹理过滤提取与 GPU 应用系统。

use std::mem;

use bevy::{
    image::{Image, ImageSampler, ImageSamplerDescriptor},
    pbr::{MeshMaterial3d, StandardMaterial},
    prelude::*,
    render::{
        Extract,
        erased_render_asset::ExtractedAssets as ExtractedErasedAssets,
        render_asset::{ExtractedAssets as ExtractedRenderAssets, RenderAssets},
        render_resource::{DefaultImageSamplerDescriptor, Sampler},
        renderer::RenderDevice,
        texture::{DefaultImageSampler, GpuImage},
    },
};

use super::state::{
    MATERIAL_REBIND_BUDGET, SAMPLER_UPDATE_BUDGET, TextureFilteringState, effective_quality,
};
use crate::{RenderSettings, TextureFilteringQuality};

/// 在 Bevy 完成资产提取后维护 PBR 材质依赖，并暂存材质到采样器就绪后再交回准备阶段。
pub(super) fn extract_texture_filtering_state(
    settings: Extract<Res<RenderSettings>>,
    source_images: Extract<Res<Assets<Image>>>,
    mut state: ResMut<TextureFilteringState>,
    default_descriptor: Res<DefaultImageSamplerDescriptor>,
    mut extracted_materials: ResMut<ExtractedErasedAssets<MeshMaterial3d<StandardMaterial>>>,
    extracted_images: Res<ExtractedRenderAssets<GpuImage>>,
) {
    state.set_target(settings.graphics.texture_filtering);

    for id in extracted_images.removed.iter().copied() {
        state.image_removed(id);
    }
    for (id, image) in &extracted_images.extracted {
        state.image_source_changed(*id, image.sampler.clone());
    }

    for id in extracted_materials.removed.iter().copied() {
        state.remove_material(id);
    }
    // 依赖已经命中目标的材质沿用 Bevy 原生准备路径；只有真正等待采样器的材质才暂存分帧。
    let mut passthrough = Vec::new();
    for (id, material) in mem::take(&mut extracted_materials.extracted) {
        state.upsert_material(id, material.clone(), &source_images);
        let Some(index) = state.material_index(id) else {
            continue;
        };
        if state.material_dependencies_applied(index, &default_descriptor.0) {
            state.mark_material_applied(index);
            passthrough.push((id, material));
        }
    }
    extracted_materials.extracted = passthrough;
}

/// 分批更新 GPU 图片采样器，并在依赖全部就绪后重建对应 StandardMaterial 绑定。
pub(super) fn apply_texture_filtering(
    mut state: ResMut<TextureFilteringState>,
    render_device: Res<RenderDevice>,
    default_sampler: Res<DefaultImageSampler>,
    default_descriptor: Res<DefaultImageSamplerDescriptor>,
    mut gpu_images: ResMut<RenderAssets<GpuImage>>,
    mut extracted_materials: ResMut<ExtractedErasedAssets<MeshMaterial3d<StandardMaterial>>>,
) {
    state.discover_materials();

    // 候选材质先声明依赖，保证本帧采样器预算优先服务即将重建的绑定。
    let candidate_count = state.material_queue.len().min(MATERIAL_REBIND_BUDGET);
    let candidates = state
        .material_queue
        .iter()
        .take(candidate_count)
        .copied()
        .collect::<Vec<_>>();
    for material_id in candidates {
        if let Some(index) = state.material_index(material_id) {
            state.queue_material_images(index, &default_descriptor.0);
        }
    }

    apply_sampler_batch(
        &mut state,
        &render_device,
        &default_sampler,
        &default_descriptor.0,
        &mut gpu_images,
    );

    let attempts = state.material_queue.len().min(MATERIAL_REBIND_BUDGET);
    for _ in 0..attempts {
        let Some(material_id) = state.material_queue.pop_front() else {
            break;
        };
        let Some(index) = state.material_index(material_id) else {
            continue; // 材质在等待期间卸载，旧队列项无需额外清理。
        };
        if !state.material_needs_rebind(index) {
            state.materials[index].queued = false;
            continue; // 正常提取路径已完成当前代次，丢弃切档前遗留的队列项。
        }
        state.queue_material_images(index, &default_descriptor.0);
        if !material_is_ready(&state, index, &default_descriptor.0, &gpu_images) {
            state.requeue_material(index);
            continue;
        }
        let source = state.materials[index].source.clone();
        extracted_materials.extracted.push((material_id, source));
        extracted_materials.modified.insert(material_id);
        extracted_materials.added.insert(material_id); // 与 Bevy 自身提取语义一致，同时覆盖私有重试队列中的旧材质克隆。
        state.mark_material_applied(index);
    }
}

/// 在固定预算内创建/恢复采样器；暂未上传 GPU 的图片留在队尾等待后续帧。
fn apply_sampler_batch(
    state: &mut TextureFilteringState,
    render_device: &RenderDevice,
    default_sampler: &DefaultImageSampler,
    default_descriptor: &ImageSamplerDescriptor,
    gpu_images: &mut RenderAssets<GpuImage>,
) {
    let attempts = state.image_queue.len().min(SAMPLER_UPDATE_BUDGET);
    for _ in 0..attempts {
        let Some(image_id) = state.image_queue.pop_front() else {
            break;
        };
        let Some(image) = state.images.get_mut(&image_id) else {
            continue;
        };
        image.queued = false;
        let desired = effective_quality(image, state.target, default_descriptor);
        if image.applied_quality == Some(desired) {
            state.remove_restored_orphan_image(image_id);
            continue;
        }
        let Some(original_sampler) = image.original_sampler.as_ref() else {
            state.queue_image(image_id);
            continue;
        };
        let Some(gpu_image) = gpu_images.get_mut(image_id) else {
            state.queue_image(image_id);
            continue;
        };
        gpu_image.sampler = sampler_for_quality(
            original_sampler,
            desired,
            default_sampler,
            default_descriptor,
            render_device,
        );
        if let Some(image) = state.images.get_mut(&image_id) {
            image.applied_quality = Some(desired);
        }
        state.remove_restored_orphan_image(image_id);
    }
}

/// 只有全部图片 GPU 资产和目标采样器都就绪时，材质才可以安全重建绑定。
fn material_is_ready(
    state: &TextureFilteringState,
    material_index: usize,
    default_descriptor: &ImageSamplerDescriptor,
    gpu_images: &RenderAssets<GpuImage>,
) -> bool {
    state.materials[material_index]
        .image_ids
        .iter()
        .all(|image_id| {
            let Some(image) = state.images.get(image_id) else {
                return false;
            };
            gpu_images.get(*image_id).is_some()
                && image.applied_quality
                    == Some(effective_quality(image, state.target, default_descriptor))
        })
}

/// 关闭时精确恢复作者采样器；开启时只改各向异性倍数并保留寻址和 LOD 范围。
fn sampler_for_quality(
    original: &ImageSampler,
    quality: TextureFilteringQuality,
    default_sampler: &DefaultImageSampler,
    default_descriptor: &ImageSamplerDescriptor,
    render_device: &RenderDevice,
) -> Sampler {
    if quality == TextureFilteringQuality::Off && matches!(original, ImageSampler::Default) {
        return (**default_sampler).clone();
    }
    let mut descriptor = match original {
        ImageSampler::Default => default_descriptor.clone(),
        ImageSampler::Descriptor(descriptor) => descriptor.clone(),
    };
    if let Some(clamp) = quality.anisotropy_clamp() {
        descriptor.set_anisotropic_filter(clamp);
    }
    render_device.create_sampler(&descriptor.as_wgpu())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::image::{ImageAddressMode, ImageFilterMode};

    #[test]
    fn anisotropic_descriptor_preserves_author_addressing_and_lod_range() {
        let mut descriptor = ImageSamplerDescriptor::linear();
        descriptor.address_mode_u = ImageAddressMode::Repeat;
        descriptor.address_mode_v = ImageAddressMode::MirrorRepeat;
        descriptor.lod_min_clamp = 1.0;
        descriptor.lod_max_clamp = 7.0;
        let adjusted = descriptor_for_quality(
            &ImageSampler::Descriptor(descriptor.clone()),
            TextureFilteringQuality::Anisotropic16,
            &ImageSamplerDescriptor::linear(),
        );
        assert_eq!(adjusted.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(adjusted.address_mode_v, ImageAddressMode::MirrorRepeat);
        assert_eq!(adjusted.lod_min_clamp, 1.0);
        assert_eq!(adjusted.lod_max_clamp, 7.0);
        assert_eq!(adjusted.anisotropy_clamp, 16);
        assert_eq!(adjusted.mipmap_filter, ImageFilterMode::Linear);
    }

    /// 测试专用纯函数，与 GPU 创建路径共享相同描述符变换规则。
    fn descriptor_for_quality(
        original: &ImageSampler,
        quality: TextureFilteringQuality,
        default_descriptor: &ImageSamplerDescriptor,
    ) -> ImageSamplerDescriptor {
        let mut descriptor = match original {
            ImageSampler::Default => default_descriptor.clone(),
            ImageSampler::Descriptor(descriptor) => descriptor.clone(),
        };
        if let Some(clamp) = quality.anisotropy_clamp() {
            descriptor.set_anisotropic_filter(clamp);
        }
        descriptor
    }
}
