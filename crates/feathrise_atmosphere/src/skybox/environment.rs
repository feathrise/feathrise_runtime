//! 作者天空补光与镜面反射的独立强度适配。
//!
//! 【功能设计】Bevy 0.19 的 `EnvironmentMapLight` 只有一个公共强度。运行时以镜面作者值
//! 作为公共载体，并只复制、缩放最大 64²×6 的离线漫反射 RGBA16F Cube；512² 完整
//! GGX Mip 链始终复用原资产，因此水面反射不会因降低人物补光而一起消失。

use std::collections::VecDeque; // 少量最近比例采用固定上限FIFO，连续拖动不会永久累积强句柄。

use bevy::asset::{AssetId, RenderAssetUsages}; // 缓存按源Image身份失效，程序图同时保留主世界和渲染世界用途。
use bevy::image::ImageSampler; // 共享黑Cube使用线性采样，符合环境贴图绑定契约。
use bevy::prelude::*; // App、Resource、Assets、Handle与Image。
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
}; // 构造最小有效Cube并保留派生图原元数据。
use half::f16; // 对RGBA16F源逐通道缩放，避免转换整张图为更大的f32缓冲。

/// 作者漫反射资产允许的最大单面边长；把同步派生工作固定在约二十万字节内。
pub(super) const MAX_AUTHORED_DIFFUSE_EDGE: u32 = 64; // 正式离线辐照图为32²，预留一倍扩展空间。
/// 最近使用的源图与补光/反射比例缓存上限；玩法和F12相机可共享同一派生句柄。
const SCALED_DIFFUSE_CACHE_CAPACITY: usize = 4; // 连续拖动最多强持有四张小图，旧活动句柄由组件自行保活到换代。

/// 一张按作者比例缩放过的漫反射Cube缓存项。
struct ScaledDiffuseCube {
    source: AssetId<Image>, // 热更新只清除由对应离线辐照图派生的缓存。
    scale_bits: u32,        // 使用规范化f32位模式精确区分作者比例，避免模糊命中旧值。
    image: Handle<Image>,   // 强句柄让两台相机复用同一派生资产。
}

/// 天空环境运行时共享的黑Cube与有界漫反射派生缓存。
#[derive(Resource, Default)]
pub(crate) struct AuthoredEnvironmentMapAssets {
    black_cube: Option<Handle<Image>>, // 零补光或零反射都复用同一1×1×6线性黑图。
    scaled_diffuse_cubes: VecDeque<ScaledDiffuseCube>, // 只在请求变化或源资产热更新后写入。
}

/// 一次采用最终交给Bevy原生环境贴图组件的两个句柄与公共强度。
pub(super) struct ResolvedEnvironmentMaps {
    pub(super) diffuse_map: Handle<Image>, // 已把补光/反射比写入像素的小型辐照Cube。
    pub(super) specular_map: Handle<Image>, // 始终是原GGX链或共享黑Cube。
    pub(super) carrier_intensity: f32,     // Bevy公共Uniform承载镜面值；镜面为零时退回承载补光值。
}

/// 安装独立环境强度所需共享资源；正式应用和纯ECS测试使用同一入口。
pub(crate) fn install_authored_environment_map_support(app: &mut App) {
    app.init_resource::<AuthoredEnvironmentMapAssets>() // 缓存资源在任何天空请求前可用。
        .add_systems(PreStartup, initialize_black_cube); // 启动时创建唯一极小黑Cube，稳定帧不占用Assets写访问。
}

/// 启动阶段幂等建立共享黑Cube。
fn initialize_black_cube(
    mut shared: ResMut<AuthoredEnvironmentMapAssets>, // 保存跨相机共享句柄。
    mut images: ResMut<Assets<Image>>,                // 只在PreStartup插入一次程序Image。
) {
    black_cube(&mut shared, &mut images); // 测试或插件重组重复调用也只返回现有句柄。
}

/// 返回共享黑色环境Cube，供任一被手动设为零的通道使用。
fn black_cube(
    shared: &mut AuthoredEnvironmentMapAssets, // 全局唯一句柄所有者。
    images: &mut Assets<Image>,                // 首次调用时登记最小Cube。
) -> Handle<Image> {
    if let Some(handle) = shared.black_cube.as_ref() {
        return handle.clone(); // 后续相机和请求只克隆同一强句柄。
    }
    let mut pixels = Vec::with_capacity(6 * 8); // RGBA16F每像素八字节，共六个面。
    for _ in 0..6 {
        pixels.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0x3c]); // RGB为零，Alpha为f16的1.0。
    }
    let mut image = Image::new(
        Extent3d {
            width: 1,                 // 最小合法Cube面不浪费显存。
            height: 1,                // 六面均为单像素。
            depth_or_array_layers: 6, // 正负XYZ六面组成texture_cube。
        },
        TextureDimension::D2,       // Cube底层是二维数组纹理。
        pixels,                     // 黑色线性HDR像素不贡献任一环境通道。
        TextureFormat::Rgba16Float, // 与正式离线辐照图保持相同格式。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 主世界可验证，渲染世界可采样。
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube), // EnvironmentMapLight要求Cube视图。
        ..default()                                  // 单Mip与六层范围覆盖完整最小资源。
    });
    image.sampler = ImageSampler::linear(); // 环境采样禁止最近邻放大。
    let handle = images.add(image); // 交给Bevy资产生命周期管理。
    shared.black_cube = Some(handle.clone()); // 资源保活到应用退出。
    handle
}

/// 在源漫反射图热更新或移除时丢弃对应比例缓存；活动组件会保留旧句柄直到原子换代。
pub(super) fn invalidate_scaled_diffuse_cubes(
    shared: &mut AuthoredEnvironmentMapAssets, // 只修改最多四项的小型缓存。
    changed_ids: &[AssetId<Image>],            // 来自有界Image事件读取结果。
    clear_all: bool,                           // 消息淘汰时无法证明具体身份，保守清空全部缓存。
) {
    if clear_all {
        shared.scaled_diffuse_cubes.clear(); // 最多丢弃四个强句柄，下一次Loading按当前源重建。
        return;
    }
    shared
        .scaled_diffuse_cubes
        .retain(|entry| !changed_ids.contains(&entry.source)); // 只淘汰确实由变更源图派生的比例。
}

/// 把两个作者强度解析为Bevy单Uniform可表达的句柄组合。
pub(super) fn resolve_environment_maps(
    shared: &mut AuthoredEnvironmentMapAssets, // 复用黑Cube与最近比例缓存。
    images: &mut Assets<Image>,                // 只在Loading采用边界创建有界小图。
    diffuse_source: &Handle<Image>,            // 已通过严格格式门的离线辐照Cube。
    specular_source: &Handle<Image>,           // 已通过完整Mip门的GGX反射链。
    diffuse_intensity: f32,                    // 作者独立天空补光基准。
    specular_intensity: f32,                   // 作者独立天空反射基准。
) -> Result<ResolvedEnvironmentMaps, String> {
    if specular_intensity == 0.0 {
        let black = black_cube(shared, images); // 镜面为零时不能继续绑定有内容的GGX链。
        let diffuse_map = if diffuse_intensity == 0.0 {
            black.clone() // 两个通道都关闭时完全复用最小黑Cube。
        } else {
            diffuse_source.clone() // 公共Uniform改由补光值承载，源辐照图无需缩放。
        };
        return Ok(ResolvedEnvironmentMaps {
            diffuse_map,                          // 补光保持作者值或为黑色。
            specular_map: black,                  // 斜视镜面也明确关闭。
            carrier_intensity: diffuse_intensity, // 镜面为零时以补光值驱动公共昼夜倍率。
        });
    }
    let scale = diffuse_intensity / specular_intensity; // 像素比例乘公共镜面强度后恰好得到作者补光值。
    if !scale.is_finite() {
        return Err("天空补光与反射比例超出可表示范围".to_owned()); // 极端作者文件不得生成NaN像素。
    }
    let diffuse_map = if scale == 0.0 {
        black_cube(shared, images) // 只关闭补光时不复制整张零图。
    } else if scale == 1.0 {
        diffuse_source.clone() // 两值相同直接沿用离线辐照图，兼容旧单字段地图。
    } else {
        scaled_diffuse_cube(shared, images, diffuse_source, scale)? // 只派生低分辨率漫反射图。
    };
    Ok(ResolvedEnvironmentMaps {
        diffuse_map,                           // 已编码独立补光/反射比例。
        specular_map: specular_source.clone(), // 高分辨率GGX链始终保持原图和完整Mip。
        carrier_intensity: specular_intensity, // Bevy公共Uniform准确表达镜面作者值。
    })
}

/// 返回或创建一张按比例缩放RGB、保留Alpha和全部Cube元数据的漫反射图。
fn scaled_diffuse_cube(
    shared: &mut AuthoredEnvironmentMapAssets, // 查询并写入最多四项的共享缓存。
    images: &mut Assets<Image>,                // 读取源CPU像素并登记派生Image。
    source_handle: &Handle<Image>,             // 缓存键与源资产身份保持一致。
    scale: f32,                                // 已验证有限且不等于零或一的补光/反射比。
) -> Result<Handle<Image>, String> {
    let source_id = source_handle.id(); // 热更新失效和跨相机复用都按AssetId处理。
    let scale_bits = scale.to_bits(); // 同一规范化作者比率稳定命中缓存。
    if let Some(entry) = shared
        .scaled_diffuse_cubes
        .iter()
        .find(|entry| entry.source == source_id && entry.scale_bits == scale_bits)
    {
        return Ok(entry.image.clone()); // 玩法与F12不重复缩放相同小图。
    }
    let source = images
        .get(source_id)
        .ok_or_else(|| "漫反射源图在派生时已离开主世界".to_owned())?; // 热更窗口等待下一次完整采用。
    let mut derived = source.clone(); // 复制固定小图并完整保留Cube视图、采样器与用途。
    let pixels = derived
        .data
        .as_mut()
        .ok_or_else(|| "漫反射源图没有可供独立缩放的主世界像素".to_owned())?; // 格式门理论上已保证，仍保持失败关闭。
    for pixel in pixels.chunks_exact_mut(8) {
        for channel in 0..3 {
            let offset = channel * 2; // RGBA16F每个通道占两个小端字节。
            let value = f16::from_bits(u16::from_le_bytes([pixel[offset], pixel[offset + 1]])); // 解码当前线性HDR通道。
            let scaled = value.to_f32() * scale; // 只改变RGB能量，Alpha保持离线资产原值。
            if !scaled.is_finite() || scaled.abs() > f16::MAX.to_f32() {
                return Err("天空补光比例会使RGBA16F漫反射像素溢出".to_owned()); // 不静默钳制作者结果。
            }
            let bytes = f16::from_f32(scaled).to_bits().to_le_bytes(); // 重新编码为原格式并保持跨平台字节序。
            pixel[offset..offset + 2].copy_from_slice(&bytes); // 只覆盖当前RGB通道。
        }
    }
    let handle = images.add(derived); // 派生图进入与普通Image一致的渲染资产上传路径。
    if shared.scaled_diffuse_cubes.len() == SCALED_DIFFUSE_CACHE_CAPACITY {
        shared.scaled_diffuse_cubes.pop_front(); // 丢弃最旧缓存强句柄，活动组件仍自行保活。
    }
    shared.scaled_diffuse_cubes.push_back(ScaledDiffuseCube {
        source: source_id,     // 记录热更新失效身份。
        scale_bits,            // 记录精确作者比例。
        image: handle.clone(), // 后续相机与相同输入直接复用。
    });
    Ok(handle) // 当前采用立即绑定新派生图。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接验证缩放数学、黑通道与缓存共享。

    /// 构造一个每面单像素、RGB为二且Alpha为一的合法辐照Cube。
    fn diffuse_test_cube() -> Image {
        let mut pixels = Vec::with_capacity(6 * 8); // 六个RGBA16F像素。
        for _ in 0..6 {
            for value in [2.0_f32, 2.0, 2.0, 1.0] {
                pixels.extend_from_slice(&f16::from_f32(value).to_bits().to_le_bytes()); // 写入稳定小端半浮点。
            }
        }
        let mut image = Image::new(
            Extent3d {
                width: 1,                 // 测试无需更大像素预算。
                height: 1,                // Cube面保持正方形。
                depth_or_array_layers: 6, // 六面满足环境贴图契约。
            },
            TextureDimension::D2,       // Cube底层二维数组。
            pixels,                     // 每面相同便于精确检查比例。
            TextureFormat::Rgba16Float, // 正式派生格式。
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 同时允许CPU读取与GPU上传。
        );
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube), // 正式Cube视图。
            ..default()                                  // 单Mip覆盖全部六层。
        });
        image
    }

    #[test]
    fn diffuse_scale_is_cached_and_does_not_change_alpha_or_specular_handle() {
        let mut shared = AuthoredEnvironmentMapAssets::default(); // 从空共享资源开始。
        let mut images = Assets::<Image>::default(); // 纯CPU资产表足以验证派生结果。
        let diffuse = images.add(diffuse_test_cube()); // 正式低分辨率源图。
        let specular = images.add(Image::default()); // 镜面像素不被本适配器读取或复制。
        let first = resolve_environment_maps(
            &mut shared,
            &mut images,
            &diffuse,
            &specular,
            300.0,
            1_000.0,
        )
        .expect("三成补光比例应可派生"); // 复现用户需要的300/1000组合。
        let second = resolve_environment_maps(
            &mut shared,
            &mut images,
            &diffuse,
            &specular,
            300.0,
            1_000.0,
        )
        .expect("相同组合应命中缓存"); // 第二台相机不得重复创建图像。
        assert_eq!(first.diffuse_map.id(), second.diffuse_map.id()); // 玩法与F12共享同一派生Cube。
        assert_eq!(first.specular_map.id(), specular.id()); // 高分辨率镜面句柄保持原身份。
        assert_eq!(first.carrier_intensity, 1_000.0); // 公共Uniform承载完整镜面反射。
        let data = images
            .get(first.diffuse_map.id())
            .and_then(|image| image.data.as_ref())
            .expect("派生Cube必须保留CPU像素"); // 读取首像素验证能量比例。
        let red = f16::from_bits(u16::from_le_bytes([data[0], data[1]])).to_f32(); // 原值二乘零点三。
        let alpha = f16::from_bits(u16::from_le_bytes([data[6], data[7]])).to_f32(); // Alpha不参与强度适配。
        assert!((red - 0.6).abs() < 0.001); // 最终乘1000等效于源图乘300。
        assert_eq!(alpha, 1.0); // 派生只修改RGB。
    }

    #[test]
    fn zero_specular_uses_black_cube_without_disabling_diffuse() {
        let mut shared = AuthoredEnvironmentMapAssets::default(); // 独立测试零反射分支。
        let mut images = Assets::<Image>::default(); // 程序黑Cube登记于同一资产表。
        let diffuse = images.add(diffuse_test_cube()); // 补光仍有内容。
        let specular = images.add(Image::default()); // 原镜面必须被黑Cube替换。
        let resolved =
            resolve_environment_maps(&mut shared, &mut images, &diffuse, &specular, 300.0, 0.0)
                .expect("零反射与非零补光应可表达"); // 公共Uniform改由补光值承载。
        assert_eq!(resolved.diffuse_map.id(), diffuse.id()); // 漫反射源无需缩放。
        assert_ne!(resolved.specular_map.id(), specular.id()); // 原GGX链不再贡献镜面。
        assert_eq!(resolved.carrier_intensity, 300.0); // 补光仍保留完整作者强度。
    }
}
