//! 动态云周期噪声：一次生成四个独立尺度通道，Shader 每次采样即可组合自然云团。

use bevy::asset::RenderAssetUsages; // 程序图需要同时保留主世界测试数据与渲染世界上传资格。
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor}; // 周期重复与线性过滤共同消除滚动接缝。
use bevy::prelude::Image; // 返回可直接登记到 Assets<Image> 的程序资产。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat}; // 明确固定二维 RGBA8 线性格式。

/// 程序云噪声纹理单边像素数。
pub(super) const CLOUD_NOISE_SIZE: u32 = 128; // 四通道 64 KiB，足以避免低频方块又保持很小常驻成本。
/// 程序噪声使用的固定种子。
pub(super) const CLOUD_NOISE_SEED: u32 = 0x6C8E_9CF5; // 不同运行与测试得到完全相同的云形基线。

/// 生成确定性、无缝可平铺且没有固定 Alpha 底膜的四通道云噪声。
pub(super) fn build_cloud_noise_image() -> Image {
    let mut pixels = Vec::with_capacity((CLOUD_NOISE_SIZE * CLOUD_NOISE_SIZE * 4) as usize); // 一次分配完整 64 KiB。
    for y in 0..CLOUD_NOISE_SIZE {
        let v = y as f32 / CLOUD_NOISE_SIZE as f32; // 半开周期坐标不重复保存首行。
        for x in 0..CLOUD_NOISE_SIZE {
            let u = x as f32 / CLOUD_NOISE_SIZE as f32; // 半开周期坐标不重复保存首列。
            let channels = [
                periodic_value_noise(u, v, 3, CLOUD_NOISE_SEED), // 大块主云团。
                periodic_value_noise(u, v, 7, CLOUD_NOISE_SEED ^ 0xA511_E9B3), // 中尺度分叉。
                periodic_value_noise(u, v, 17, CLOUD_NOISE_SEED ^ 0x63D8_35A7), // 细节侵蚀。
                periodic_value_noise(u, v, 11, CLOUD_NOISE_SEED ^ 0xB529_7A4D), // 第二套边缘扰动。
            ];
            pixels.extend(channels.map(|value| (value * 255.0).round() as u8)); // RGBA 都是原始线性噪声，不承载最低透明度。
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: CLOUD_NOISE_SIZE,  // 固定一百二十八像素宽。
            height: CLOUD_NOISE_SIZE, // 固定一百二十八像素高。
            depth_or_array_layers: 1, // 只使用单张二维纹理。
        },
        TextureDimension::D2,      // 自定义材质绑定普通 texture_2d。
        pixels,                    // 四通道各自保存一个周期尺度。
        TextureFormat::Rgba8Unorm, // 噪声必须保持线性数值，不能经过 sRGB 解码。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 测试可读且 GPU 可采样。
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat, // 风偏移跨 U 边界无缝循环。
        address_mode_v: ImageAddressMode::Repeat, // 风偏移跨 V 边界无缝循环。
        address_mode_w: ImageAddressMode::Repeat, // 二维纹理不使用 W，但保持完整周期语义。
        mag_filter: ImageFilterMode::Linear,      // 放大低频噪声时保持柔和云边。
        min_filter: ImageFilterMode::Linear,      // 缩小时混合邻近噪声，降低闪烁。
        mipmap_filter: ImageFilterMode::Linear,   // 未来添加 Mip 时保持层级平滑。
        ..Default::default()                      // 其余 LOD 与各向异性使用 Bevy 安全默认值。
    });
    image // 返回由系统缓存强句柄的一次性程序纹理。
}

/// 计算零至一周期域上的确定性二维值噪声。
pub(super) fn periodic_value_noise(
    u: f32,         // 任意有限水平坐标会循环到零至一。
    v: f32,         // 任意有限垂直坐标会循环到零至一。
    frequency: u32, // 周期内网格数决定当前尺度。
    seed: u32,      // 固定种子区分各通道图案。
) -> f32 {
    let frequency = frequency.max(1); // 防御零频率造成取模错误。
    let x = finite_unit(u.rem_euclid(1.0), 0.0) * frequency as f32; // 映射到周期网格空间。
    let y = finite_unit(v.rem_euclid(1.0), 0.0) * frequency as f32; // 映射到周期网格空间。
    let x0 = x.floor() as u32 % frequency; // 当前网格水平索引。
    let y0 = y.floor() as u32 % frequency; // 当前网格垂直索引。
    let x1 = (x0 + 1) % frequency; // 边界相邻点回绕到首列。
    let y1 = (y0 + 1) % frequency; // 边界相邻点回绕到首行。
    let tx = smoothstep(x.fract()); // 水平使用端点零导数插值。
    let ty = smoothstep(y.fract()); // 垂直使用端点零导数插值。
    let top = lerp(hash_unit(x0, y0, seed), hash_unit(x1, y0, seed), tx); // 当前单元上边。
    let bottom = lerp(hash_unit(x0, y1, seed), hash_unit(x1, y1, seed), tx); // 当前单元下边。
    lerp(top, bottom, ty) // 双线性完成周期值噪声。
}

/// 把二维整数网格稳定映射到零至一伪随机值。
fn hash_unit(x: u32, y: u32, seed: u32) -> f32 {
    let mut value = seed ^ x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B); // 混合坐标与通道种子。
    value ^= value >> 16; // 第一次扩散高低位。
    value = value.wrapping_mul(0x7FEB_352D); // 乘法雪崩降低相邻相关性。
    value ^= value >> 15; // 第二次扩散位模式。
    value = value.wrapping_mul(0x846C_A68B); // 最终乘法雪崩。
    value ^= value >> 16; // 折叠高位完成散列。
    value as f32 / u32::MAX as f32 // 归一化为零至一。
}

/// 零至一三次平滑曲线。
pub(super) fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0); // 先钳制定义域。
    value * value * (3.0 - 2.0 * value) // 首尾导数为零。
}

/// 一维线性插值。
fn lerp(from: f32, to: f32, factor: f32) -> f32 {
    from + (to - from) * factor // 统一表达式用于周期网格四角。
}

/// 返回有限零至一值，否则使用安全回退。
fn finite_unit(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) // 合法值限制在归一化域。
    } else {
        fallback // NaN 与无穷不得传播到纹理像素。
    }
}
