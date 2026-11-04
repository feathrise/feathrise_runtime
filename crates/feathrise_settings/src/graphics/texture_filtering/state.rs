// 游戏设置 — 纹理过滤的有界材质/图片注册表。

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::{
    asset::{AssetId, Assets},
    image::{Image, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::StandardMaterial,
    prelude::Resource,
};

use crate::TextureFilteringQuality;

/// 每帧最多发现的待刷新材质数，避免画质切换随场景资产规模产生单帧尖峰。
pub(super) const MATERIAL_DISCOVERY_BUDGET: usize = 16;
/// 每帧最多实际重建的材质绑定数；未就绪材质留到后续帧继续轮询。
pub(super) const MATERIAL_REBIND_BUDGET: usize = 16;
/// 每帧最多创建或恢复的 GPU 采样器数，限制驱动对象创建开销。
pub(super) const SAMPLER_UPDATE_BUDGET: usize = 64;
/// 每帧最多检查的注册表槽位数；即使仅剩一个待处理项也不会全表扫描。
const MATERIAL_SCAN_BUDGET: usize = 64;

/// 一个已加载 StandardMaterial 的长期跟踪状态。
pub(super) struct TrackedMaterial {
    pub(super) id: AssetId<StandardMaterial>,
    pub(super) source: StandardMaterial,
    pub(super) image_ids: Vec<AssetId<Image>>,
    pub(super) applied_generation: u64,
    pub(super) queued: bool,
}

/// 一个被 StandardMaterial 引用的图片采样器状态。
pub(super) struct TrackedImage {
    pub(super) original_sampler: Option<ImageSampler>,
    pub(super) material_users: HashSet<AssetId<StandardMaterial>>,
    pub(super) applied_quality: Option<TextureFilteringQuality>,
    pub(super) queued: bool,
}

impl TrackedImage {
    fn new(original_sampler: Option<ImageSampler>, target: TextureFilteringQuality) -> Self {
        Self {
            original_sampler,
            material_users: HashSet::new(),
            applied_quality: (target == TextureFilteringQuality::Off)
                .then_some(TextureFilteringQuality::Off), // 初始关闭时 GPU 图片沿用 Bevy 刚准备的作者采样器。
            queued: false,
        }
    }
}

/// RenderWorld 中唯一的纹理过滤过渡状态；稳定帧没有队列时只执行常数级返回。
#[derive(Resource)]
pub(super) struct TextureFilteringState {
    pub(super) target: TextureFilteringQuality,
    pub(super) generation: u64,
    pub(super) materials: Vec<TrackedMaterial>,
    material_indices: HashMap<AssetId<StandardMaterial>, usize>,
    pub(super) images: HashMap<AssetId<Image>, TrackedImage>,
    material_scan_cursor: usize,
    pending_material_count: usize,
    pub(super) material_queue: VecDeque<AssetId<StandardMaterial>>,
    pub(super) image_queue: VecDeque<AssetId<Image>>,
}

impl Default for TextureFilteringState {
    fn default() -> Self {
        Self {
            target: TextureFilteringQuality::Off,
            generation: 1,
            materials: Vec::new(),
            material_indices: HashMap::new(),
            images: HashMap::new(),
            material_scan_cursor: 0,
            pending_material_count: 0,
            material_queue: VecDeque::new(),
            image_queue: VecDeque::new(),
        }
    }
}

impl TextureFilteringState {
    /// 切换目标档位时只推进代次；现有材质由后续帧按游标惰性加入队列。
    pub(super) fn set_target(&mut self, target: TextureFilteringQuality) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.generation = self.generation.wrapping_add(1).max(1);
        self.pending_material_count = self.materials.len();
        self.material_scan_cursor = 0;
    }

    /// 登记新增或修改材质，并精确维护材质到图片的反向引用。
    pub(super) fn upsert_material(
        &mut self,
        id: AssetId<StandardMaterial>,
        source: StandardMaterial,
        images: &Assets<Image>,
    ) {
        let image_ids = standard_material_image_ids(&source);
        if let Some(index) = self.material_indices.get(&id).copied() {
            let old_image_ids = self.materials[index].image_ids.clone();
            for image_id in old_image_ids {
                self.detach_image_user(image_id, id);
            }
            let was_current = self.materials[index].applied_generation == self.generation;
            let queued = self.materials[index].queued;
            self.materials[index] = TrackedMaterial {
                id,
                source,
                image_ids: image_ids.clone(),
                applied_generation: self.generation.wrapping_sub(1),
                queued,
            };
            if was_current {
                self.pending_material_count += 1;
            }
        } else {
            let index = self.materials.len();
            self.material_indices.insert(id, index);
            self.materials.push(TrackedMaterial {
                id,
                source,
                image_ids: image_ids.clone(),
                applied_generation: self.generation.wrapping_sub(1),
                queued: false,
            });
            self.pending_material_count += 1;
        }

        for image_id in image_ids {
            let original = images.get(image_id).map(|image| image.sampler.clone());
            let image = self
                .images
                .entry(image_id)
                .or_insert_with(|| TrackedImage::new(original.clone(), self.target));
            if image.original_sampler.is_none() {
                image.original_sampler = original;
            }
            image.material_users.insert(id);
        }
    }

    /// 移除不再存活的材质；无其他 PBR 使用者的图片会恢复作者采样器后退出注册表。
    pub(super) fn remove_material(&mut self, id: AssetId<StandardMaterial>) {
        let Some(index) = self.material_indices.remove(&id) else {
            return;
        };
        let removed = self.materials.swap_remove(index);
        if removed.applied_generation != self.generation {
            self.pending_material_count = self.pending_material_count.saturating_sub(1);
        }
        if index < self.materials.len() {
            self.material_indices
                .insert(self.materials[index].id, index);
        }
        self.material_scan_cursor = self.material_scan_cursor.min(self.materials.len());
        for image_id in removed.image_ids {
            self.detach_image_user(image_id, id);
        }
    }

    /// 图片重新提取后更新作者采样器，并让所有依赖材质等待新的 GPU 句柄。
    pub(super) fn image_source_changed(&mut self, id: AssetId<Image>, sampler: ImageSampler) {
        let Some(image) = self.images.get_mut(&id) else {
            return; // 非 StandardMaterial 图片（UI、密度图、风场等）不进入全局接管。
        };
        image.original_sampler = Some(sampler);
        image.applied_quality =
            (self.target == TextureFilteringQuality::Off).then_some(TextureFilteringQuality::Off); // 关闭档由本帧 Bevy 图片准备直接恢复作者采样器。
        let users = image.material_users.iter().copied().collect::<Vec<_>>();
        if self.target != TextureFilteringQuality::Off {
            self.queue_image(id);
        }
        for material_id in users {
            self.mark_material_stale(material_id);
        }
    }

    /// 图片 GPU 资产卸载时清除就绪标记；若仍有材质引用，后续重载会自动继承当前档位。
    pub(super) fn image_removed(&mut self, id: AssetId<Image>) {
        let Some(image) = self.images.get_mut(&id) else {
            return;
        };
        image.original_sampler = None;
        image.applied_quality = None;
        image.queued = false;
        let users = image.material_users.iter().copied().collect::<Vec<_>>();
        for material_id in users {
            self.mark_material_stale(material_id);
        }
        if self
            .images
            .get(&id)
            .is_some_and(|image| image.material_users.is_empty())
        {
            self.images.remove(&id);
        }
    }

    /// 从注册表中按固定扫描预算发现下一批需要重建的材质。
    pub(super) fn discover_materials(&mut self) {
        if self.pending_material_count == 0 || self.materials.is_empty() {
            return;
        }
        let mut inspected = 0;
        let mut discovered = 0;
        while inspected < MATERIAL_SCAN_BUDGET && discovered < MATERIAL_DISCOVERY_BUDGET {
            if self.materials.is_empty() {
                break;
            }
            let index = self.material_scan_cursor % self.materials.len();
            self.material_scan_cursor = (index + 1) % self.materials.len();
            inspected += 1;
            let material = &mut self.materials[index];
            if material.applied_generation == self.generation || material.queued {
                continue;
            }
            material.queued = true;
            self.material_queue.push_back(material.id);
            discovered += 1;
        }
    }

    /// 返回材质的注册表索引；队列中的已移除材质会得到 None 并被安全跳过。
    pub(super) fn material_index(&self, id: AssetId<StandardMaterial>) -> Option<usize> {
        self.material_indices.get(&id).copied()
    }

    /// 判断材质的全部图片是否已经处于当前目标；满足时可沿用 Bevy 本帧正常提取路径。
    pub(super) fn material_dependencies_applied(
        &self,
        material_index: usize,
        default_descriptor: &ImageSamplerDescriptor,
    ) -> bool {
        self.materials[material_index]
            .image_ids
            .iter()
            .all(|image_id| {
                self.images.get(image_id).is_some_and(|image| {
                    image.applied_quality
                        == Some(effective_quality(image, self.target, default_descriptor))
                })
            })
    }

    /// 返回材质是否仍需当前代次的绑定刷新，用于丢弃队列中的过期 ID。
    pub(super) fn material_needs_rebind(&self, material_index: usize) -> bool {
        self.materials[material_index].applied_generation != self.generation
    }

    /// 为一个待重建材质排入所有尚未达到目标状态的图片。
    pub(super) fn queue_material_images(
        &mut self,
        material_index: usize,
        default_descriptor: &ImageSamplerDescriptor,
    ) {
        let image_ids = self.materials[material_index].image_ids.clone();
        for image_id in image_ids {
            let needs_update = self.images.get(&image_id).is_some_and(|image| {
                image.applied_quality
                    != Some(effective_quality(image, self.target, default_descriptor))
            });
            if needs_update {
                self.queue_image(image_id);
            }
        }
    }

    /// 标记一个材质已经使用当前代次的全部采样器完成绑定。
    pub(super) fn mark_material_applied(&mut self, material_index: usize) {
        let material = &mut self.materials[material_index];
        if material.applied_generation != self.generation {
            material.applied_generation = self.generation;
            self.pending_material_count = self.pending_material_count.saturating_sub(1);
        }
        material.queued = false;
    }

    /// 待就绪材质继续留在有界队列中，下一帧只做非阻塞轮询。
    pub(super) fn requeue_material(&mut self, material_index: usize) {
        self.material_queue
            .push_back(self.materials[material_index].id);
    }

    /// 将图片 ID 至多加入一次采样器更新队列。
    pub(super) fn queue_image(&mut self, id: AssetId<Image>) {
        let Some(image) = self.images.get_mut(&id) else {
            return;
        };
        if image.queued {
            return;
        }
        image.queued = true;
        self.image_queue.push_back(id);
    }

    /// 无 PBR 使用者且已经恢复作者状态的图片无需继续占用注册表。
    pub(super) fn remove_restored_orphan_image(&mut self, id: AssetId<Image>) {
        if self.images.get(&id).is_some_and(|image| {
            image.material_users.is_empty()
                && image.applied_quality == Some(TextureFilteringQuality::Off)
        }) {
            self.images.remove(&id);
        }
    }

    fn detach_image_user(
        &mut self,
        image_id: AssetId<Image>,
        material_id: AssetId<StandardMaterial>,
    ) {
        let Some(image) = self.images.get_mut(&image_id) else {
            return;
        };
        image.material_users.remove(&material_id);
        if !image.material_users.is_empty() {
            return;
        }
        if image.applied_quality == Some(TextureFilteringQuality::Off) {
            self.images.remove(&image_id);
        } else {
            self.queue_image(image_id); // 最后一个 PBR 使用者消失后恢复作者采样器。
        }
    }

    fn mark_material_stale(&mut self, id: AssetId<StandardMaterial>) {
        let Some(index) = self.material_indices.get(&id).copied() else {
            return;
        };
        if self.materials[index].applied_generation == self.generation {
            self.materials[index].applied_generation = self.generation.wrapping_sub(1);
            self.pending_material_count += 1;
        }
    }
}

/// 返回一个 StandardMaterial 使用的去重图片 ID；只接管正式 PBR 材质依赖。
pub(super) fn standard_material_image_ids(material: &StandardMaterial) -> Vec<AssetId<Image>> {
    let handles = [
        material.base_color_texture.as_ref(),
        material.emissive_texture.as_ref(),
        material.metallic_roughness_texture.as_ref(),
        material.normal_map_texture.as_ref(),
        material.occlusion_texture.as_ref(),
        material.specular_texture.as_ref(),
        material.specular_tint_texture.as_ref(),
    ]; // 仅列出当前工作区启用的核心与 pbr_specular_textures 槽位；未启用的透射/清漆字段不存在于本构建类型中。
    let mut ids = Vec::with_capacity(handles.len());
    for id in handles.into_iter().flatten().map(|handle| handle.id()) {
        if !ids.contains(&id) {
            ids.push(id); // 同一 ORM 等复用纹理只创建一次采样器并只计一个依赖。
        }
    }
    ids
}

/// 只允许原本完整线性且非比较采样的图片启用各向异性，保留数据纹理作者语义。
pub(super) fn sampler_supports_anisotropy(
    sampler: &ImageSampler,
    default_descriptor: &ImageSamplerDescriptor,
) -> bool {
    let descriptor = match sampler {
        ImageSampler::Default => default_descriptor,
        ImageSampler::Descriptor(descriptor) => descriptor,
    };
    descriptor.compare.is_none()
        && descriptor.mag_filter == ImageFilterMode::Linear
        && descriptor.min_filter == ImageFilterMode::Linear
        && descriptor.mipmap_filter == ImageFilterMode::Linear
}

/// 计算图片当前真正应使用的档位；无使用者或不兼容图片始终恢复作者采样器。
pub(super) fn effective_quality(
    image: &TrackedImage,
    target: TextureFilteringQuality,
    default_descriptor: &ImageSamplerDescriptor,
) -> TextureFilteringQuality {
    match image.original_sampler.as_ref() {
        Some(sampler)
            if !image.material_users.is_empty()
                && sampler_supports_anisotropy(sampler, default_descriptor) =>
        {
            target
        }
        _ => TextureFilteringQuality::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::image::ImageAddressMode;

    #[test]
    fn standard_material_dependencies_are_complete_and_deduplicated() {
        let mut images = Assets::<Image>::default();
        let shared = images.add(Image::default());
        let normal = images.add(Image::default());
        let material = StandardMaterial {
            base_color_texture: Some(shared.clone()),
            metallic_roughness_texture: Some(shared.clone()),
            normal_map_texture: Some(normal.clone()),
            ..Default::default()
        };
        assert_eq!(
            standard_material_image_ids(&material),
            vec![shared.id(), normal.id()]
        ); // 重复 ORM/颜色句柄不得造成重复采样器工作。
    }

    #[test]
    fn only_complete_linear_non_comparison_samplers_are_eligible() {
        let linear = ImageSamplerDescriptor::linear();
        assert!(sampler_supports_anisotropy(&ImageSampler::Default, &linear));
        assert!(sampler_supports_anisotropy(
            &ImageSampler::Descriptor(linear.clone()),
            &linear,
        ));
        assert!(!sampler_supports_anisotropy(
            &ImageSampler::nearest(),
            &linear,
        )); // 密度图和像素语义纹理的显式最近点采样必须保持不变。

        let mut mixed = linear.clone();
        mixed.mipmap_filter = ImageFilterMode::Nearest;
        mixed.address_mode_u = ImageAddressMode::Repeat;
        assert!(!sampler_supports_anisotropy(
            &ImageSampler::Descriptor(mixed),
            &linear,
        )); // 不擅自把作者选择的 mip 过滤方式改为线性。
    }

    #[test]
    fn material_discovery_is_bounded_and_stable_after_current_generation_applies() {
        let images = Assets::<Image>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut state = TextureFilteringState::default();
        for _ in 0..(MATERIAL_DISCOVERY_BUDGET + 5) {
            let handle = materials.add(StandardMaterial::default());
            state.upsert_material(handle.id(), StandardMaterial::default(), &images);
        }

        state.discover_materials();
        assert_eq!(state.material_queue.len(), MATERIAL_DISCOVERY_BUDGET); // 首帧不得把全部资产塞入重建队列。
        while let Some(id) = state.material_queue.pop_front() {
            let index = state.material_index(id).expect("登记材质应仍然存活");
            state.mark_material_applied(index);
        }
        state.discover_materials();
        assert_eq!(state.material_queue.len(), 5); // 剩余材质在下一批继续推进。
        while let Some(id) = state.material_queue.pop_front() {
            let index = state.material_index(id).expect("登记材质应仍然存活");
            state.mark_material_applied(index);
        }
        state.discover_materials();
        assert!(state.material_queue.is_empty()); // 稳定代次不得重新排入已应用材质。
    }

    #[test]
    fn shared_image_is_retained_until_the_last_standard_material_is_removed() {
        let mut images = Assets::<Image>::default();
        let shared = images.add(Image::default());
        let mut materials = Assets::<StandardMaterial>::default();
        let first = materials.add(StandardMaterial::default());
        let second = materials.add(StandardMaterial::default());
        let source = StandardMaterial {
            base_color_texture: Some(shared.clone()),
            ..Default::default()
        };
        let mut state = TextureFilteringState::default();
        state.upsert_material(first.id(), source.clone(), &images);
        state.upsert_material(second.id(), source, &images);
        assert_eq!(state.images[&shared.id()].material_users.len(), 2);

        state.remove_material(first.id());
        assert_eq!(state.images[&shared.id()].material_users.len(), 1); // 一个使用者卸载不能提前恢复共享采样器。
        state.remove_material(second.id());
        assert!(!state.images.contains_key(&shared.id())); // 默认关闭状态下最后一个使用者卸载即可 O(1) 注销。
    }
}
