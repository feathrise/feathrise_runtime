//! 最小生产KTX2经Bevy真实解码及同帧资产事件回归，覆盖首载、单切片和热更；不访问用户资产。

use super::*;
use bevy::{
    asset::{AssetApp, AssetPlugin, RenderAssetUsages},
    image::{CompressedImageFormats, ImageType},
    prelude::*,
};

// 由本轮失败的正式地表编码器生成的1×1、两切片RGBA8 sRGB文件；只保留208字节最小输入。
const ARRAY_KTX2: &[u8] = &[
    0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a, 0x2b, 0, 0, 0, 1, 0, 0,
    0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0x68, 0,
    0, 0, 0x5c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0xc8, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0x5c, 0, 0, 0, 0, 0,
    0, 0, 2, 0, 0x58, 0, 1, 1, 2, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0xff, 0, 0, 0, 8, 0, 7, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0, 0, 0, 0x10, 0, 7, 2, 0, 0,
    0, 0, 0, 0, 0, 0, 0xff, 0, 0, 0, 0x18, 0, 7, 0x0f, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0, 0, 0, 0, 0,
    0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
];

fn decoded(layers: u32) -> Image {
    let mut payload = ARRAY_KTX2.to_vec();
    if layers == 1 {
        // 仅将真实同格式输入缩为单层；KTX2头保留pixelHeight=1及array layerCount=1。
        payload[32..36].copy_from_slice(&1u32.to_le_bytes());
        payload[88..96].copy_from_slice(&4u64.to_le_bytes());
        payload[96..104].copy_from_slice(&4u64.to_le_bytes());
        payload.truncate(204);
    }
    Image::from_buffer(
        &payload,
        ImageType::Extension("ktx2"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Descriptor(ImageSamplerDescriptor {
            label: Some(format!("{IMAGE_ARRAY_TAG}regression")),
            ..default()
        }),
        RenderAssetUsages::default(),
    )
    .expect("真实最小KTX2必须能由Bevy加载器解码")
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Image>();
    install_shape_normalization(&mut app);
    app
}

#[test]
fn real_one_pixel_ktx2_arrays_are_fixed_in_the_added_frame_without_changing_content() {
    let mut app = app();
    for layers in [1, 2] {
        let image = decoded(layers);
        // 真实重现上游误判，避免只给已经正确的Image跑一个空测试。
        assert_eq!(image.texture_descriptor.dimension, TextureDimension::D1);
        let before = (
            image.data.clone(),
            image.data_order,
            image.texture_descriptor.size,
            image.texture_descriptor.format,
            image.texture_descriptor.mip_level_count,
        );
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        app.update(); // 资产事件同帧发布→规范化；下一步才可能由RenderApp抽取。
        let actual = app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .unwrap();
        assert_eq!(actual.texture_descriptor.dimension, TextureDimension::D2);
        assert_eq!(
            actual.texture_view_descriptor.as_ref().unwrap().dimension,
            Some(TextureViewDimension::D2Array)
        );
        assert_eq!(actual.texture_descriptor.size.depth_or_array_layers, layers);
        assert_eq!(
            (
                actual.data.clone(),
                actual.data_order,
                actual.texture_descriptor.size,
                actual.texture_descriptor.format,
                actual.texture_descriptor.mip_level_count
            ),
            before
        );
    }
}

#[test]
fn hot_reload_is_normalized_before_extraction_and_does_not_loop_modified_events() {
    let mut app = app();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(decoded(1));
    app.update();
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .insert(handle.id(), decoded(2))
        .unwrap();
    app.update();
    let actual = app
        .world()
        .resource::<Assets<Image>>()
        .get(&handle)
        .unwrap();
    assert_eq!(actual.texture_descriptor.dimension, TextureDimension::D2);
    assert_eq!(actual.texture_descriptor.size.depth_or_array_layers, 2);
    let mut cursor = MessageCursor::<AssetEvent<Image>>::default();
    cursor
        .read(app.world().resource::<Messages<AssetEvent<Image>>>())
        .for_each(drop);
    app.update();
    assert_eq!(
        cursor
            .read(app.world().resource::<Messages<AssetEvent<Image>>>())
            .filter(|event| matches!(event, AssetEvent::Modified { .. }))
            .count(),
        0
    );
}

#[test]
fn explicit_two_dimensional_contract_does_not_claim_other_assets_or_three_dimensional_images() {
    let mut image = decoded(1);
    image.sampler = ImageSampler::Default;
    assert!(expected_view(&image).is_none());
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        label: Some(format!("{IMAGE_2D_TAG}mask")),
        ..default()
    });
    assert_eq!(expected_view(&image), Some(TextureViewDimension::D2));
    assert!(normalize_shape(&mut image, TextureViewDimension::D2));
    assert!(!normalize_shape(&mut image, TextureViewDimension::D2));
    image.texture_descriptor.dimension = TextureDimension::D3;
    assert!(!normalize_shape(&mut image, TextureViewDimension::D2Array));
    assert_eq!(image.texture_descriptor.dimension, TextureDimension::D3);
}
