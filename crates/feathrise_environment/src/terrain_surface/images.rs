//! 通用地表图片加载设置与完整状态判定。

use bevy::asset::{
    AssetEvent, AssetEventSystems, Assets, DependencyLoadState, LoadState,
    RecursiveDependencyLoadState,
};
use bevy::ecs::message::{MessageCursor, Messages};
use bevy::image::{
    ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor,
};
use bevy::prelude::{
    App, AssetServer, Handle, Image, IntoScheduleConfigs, Local, PostUpdate, Res, ResMut,
};
use bevy::render::render_resource::{
    TextureDimension, TextureViewDescriptor, TextureViewDimension,
};

// ImageLoaderSettings没有维度覆写；内部采样器标签把本入口的明确绑定合同带到异步完成值。
// 标签仅是加载请求元数据，不按文件名/像素尺寸猜测，也不影响像素、采样或资产持久格式。
const IMAGE_2D_TAG: &str = "rpg_surface_2d:";
const IMAGE_ARRAY_TAG: &str = "rpg_surface_2d_array:";

#[derive(Clone, Copy)]
enum ImageShape {
    D2,
    Array,
}

impl ImageShape {
    fn tag(self) -> &'static str {
        match self {
            Self::D2 => IMAGE_2D_TAG,
            Self::Array => IMAGE_ARRAY_TAG,
        }
    }
}

/// Added/Modified已进入消息流、RenderApp尚未抽取的同帧只纠正受本入口请求的图片。
pub(super) fn install_shape_normalization(app: &mut App) {
    app.add_systems(PostUpdate, normalize_loaded_shapes.after(AssetEventSystems));
}

fn normalize_loaded_shapes(
    events: Option<Res<Messages<AssetEvent<Image>>>>,
    mut reader: Local<MessageCursor<AssetEvent<Image>>>,
    mut images: ResMut<Assets<Image>>,
) {
    // 无AssetPlugin的纯逻辑World不加载磁盘图片，也不强迫其补装资产消息系统。
    let Some(events) = events else {
        return;
    };
    for event in reader.read(&events) {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let Some(view) = images.get(*id).and_then(expected_view) else {
            continue;
        };
        let image = images
            .get_mut_untracked(*id)
            .expect("同系统已经确认图片存在");
        // 当前Added/Modified已经通知抽取；不再发Modified，避免每次修正触发热更循环。
        normalize_shape(image, view);
    }
}

fn expected_view(image: &Image) -> Option<TextureViewDimension> {
    let ImageSampler::Descriptor(sampler) = &image.sampler else {
        return None;
    };
    let label = sampler.label.as_deref()?;
    if label.starts_with(IMAGE_ARRAY_TAG) {
        Some(TextureViewDimension::D2Array)
    } else if label.starts_with(IMAGE_2D_TAG) {
        Some(TextureViewDimension::D2)
    } else {
        None
    }
}

/// Bevy0.19 KTX2按height>1判断D2，错把合法1×1二维数组解为D1；单切片还丢失数组view。
/// 这里只恢复调用者已声明的二维绑定合同；三维/Cube及普通外部图片不取得改写权限。
fn normalize_shape(image: &mut Image, view: TextureViewDimension) -> bool {
    if !matches!(
        image.texture_descriptor.dimension,
        TextureDimension::D1 | TextureDimension::D2
    ) || (view == TextureViewDimension::D2
        && image.texture_descriptor.size.depth_or_array_layers != 1)
        || image
            .texture_view_descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.dimension)
            .is_some_and(|current| {
                !matches!(
                    current,
                    TextureViewDimension::D1
                        | TextureViewDimension::D2
                        | TextureViewDimension::D2Array
                )
            })
    {
        return false;
    }
    let changed = image.texture_descriptor.dimension != TextureDimension::D2
        || image
            .texture_view_descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.dimension)
            != Some(view);
    if changed {
        image.texture_descriptor.dimension = TextureDimension::D2;
        image
            .texture_view_descriptor
            .get_or_insert_with(TextureViewDescriptor::default)
            .dimension = Some(view);
    }
    changed
}

/// 以固定三线性采样加载二维权重、索引或遮罩；Repeat只决定采样，数组使用具名入口。
pub fn load_terrain_surface_image(
    asset_server: &AssetServer,
    path: String,
    is_srgb: bool,
    repeat: bool,
    label: &'static str,
) -> Handle<Image> {
    load_image(asset_server, path, is_srgb, repeat, label, ImageShape::D2)
}

/// PBR基础色/法线/ARM明确绑定二维数组，单切片同样保留D2Array，不与Repeat语义混用。
pub fn load_terrain_surface_array(
    asset_server: &AssetServer,
    path: String,
    is_srgb: bool,
    label: &'static str,
) -> Handle<Image> {
    load_image(asset_server, path, is_srgb, true, label, ImageShape::Array)
}

fn load_image(
    asset_server: &AssetServer,
    path: String,
    is_srgb: bool,
    repeat: bool,
    label: &'static str,
    shape: ImageShape,
) -> Handle<Image> {
    asset_server
        .load_builder()
        .with_settings(move |settings: &mut ImageLoaderSettings| {
            settings.is_srgb = is_srgb;
            let address_mode = if repeat {
                ImageAddressMode::Repeat
            } else {
                ImageAddressMode::ClampToEdge
            };
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                label: Some(format!("{}{label}", shape.tag())),
                address_mode_u: address_mode,
                address_mode_v: address_mode,
                address_mode_w: ImageAddressMode::ClampToEdge,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                mipmap_filter: ImageFilterMode::Linear,
                ..Default::default()
            });
        })
        .load(path)
}

#[cfg(test)]
mod tests;

/// 根图片或任一依赖明确失败时返回真；仍在加载不算失败。
pub fn terrain_surface_image_failed(asset_server: &AssetServer, image: &Handle<Image>) -> bool {
    asset_server
        .get_load_states(image.id())
        .is_some_and(|(direct, dependencies, recursive)| {
            matches!(direct, LoadState::Failed(_))
                || matches!(dependencies, DependencyLoadState::Failed(_))
                || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
        })
}

/// 固定纹理组根资产及全部依赖是否都已就绪。
pub fn terrain_surface_images_loaded<const N: usize>(
    asset_server: &AssetServer,
    images: &[Handle<Image>; N],
) -> bool {
    images
        .iter()
        .all(|image| asset_server.is_loaded_with_dependencies(image.id()))
}
