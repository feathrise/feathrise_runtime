//! 树木与灌木共享的固定周期风噪声生成。
//! 【功能设计】该模块只在Startup生成一次固定 `512 × 512` RGBA8 图片；成本不随地图、实体或帧数增长。

use bevy::asset::RenderAssetUsages; // 程序图片需要同时保留主世界资产身份并上传渲染世界。
use bevy::image::{
    // 自有噪声纹理必须使用无缝重复和线性过滤。
    ImageAddressMode,       // U/V越过零至一时回绕到同一周期风场。
    ImageFilterMode,        // 八位值噪声依赖线性过滤恢复连续采样。
    ImageSampler,           // 运行时Image显式覆盖默认采样器。
    ImageSamplerDescriptor, // 完整声明重复、过滤和后续mip口径。
}; // 结束图像采样类型导入。
use bevy::prelude::*; // 引入Commands、Assets和Image等Startup系统类型。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat}; // 程序Image明确二维尺寸与线性RGBA8格式。

use super::resources::TreeWindNoiseTexture; // 完整Image登记后才插入公共就绪资源。

const TREE_WIND_NOISE_SIZE: u32 = 512; // 固定尺寸与既有风场采样周期一致且不会随内容扩张。
const TREE_WIND_NOISE_CHANNELS: [(u32, u32); 4] = [
    (8, 0x5A17_1EED),  // 红通道八格低频宏观风让相邻树自然错相。
    (32, 0xC0DE_4A11), // 绿通道三十二格高频细节只供近中景叶簇使用。
    (16, 0x71A5_E001), // 蓝通道十六格稳定相位驱动横向S曲线。
    (24, 0xB09B_1E55), // Alpha通道二十四格稳定相位驱动上下起伏。
]; // 完成四个独立周期噪声通道规格。

/// 在Startup一次生成并登记共享噪声Image。
/// 资源只在完整像素、采样器和Assets句柄全部成功构造后原子出现。
pub(super) fn setup_tree_wind_noise_texture(
    mut commands: Commands, // 完成后插入唯一公共资源供地图材质系统观察。
    mut images: ResMut<Assets<Image>>, // 程序图片登记必须留在主世界资产所有线程。
) {
    // 固定批次不放入Update，避免任何地图重进或热重载重复生成四通道像素。
    let handle = images.add(build_tree_wind_noise_image()); // 完整构造后一次取得强Image句柄。
    commands.insert_resource(TreeWindNoiseTexture(handle)); // 句柄有效后原子发布材质就绪门。
} // 完成共享风噪声Startup登记。

/// 构造固定尺寸、无缝重复、线性过滤的四通道风噪声图片。
fn build_tree_wind_noise_image() -> Image {
    // 像素生成与Image登记分离，单元测试可直接验证确定性和格式契约。
    let pixels = build_tree_wind_noise_pixels(); // 先完整生成固定一MiB线性RGBA8数据。
    let mut image = Image::new(
        Extent3d {
            width: TREE_WIND_NOISE_SIZE,  // 固定五百一十二像素宽度。
            height: TREE_WIND_NOISE_SIZE, // 固定五百一十二像素高度。
            depth_or_array_layers: 1,     // 风场只使用一张二维纹理层。
        }, // 完成二维图片尺寸描述。
        TextureDimension::D2,         // 顶点Shader按二维世界X/Z坐标采样。
        pixels,                       // 传入完整宏观、微观和两组相位通道。
        TextureFormat::Rgba8Unorm,    // 噪声保持线性零至一，不经过sRGB颜色转换。
        RenderAssetUsages::default(), // 主世界测试可读且允许渲染世界上传。
    ); // 完成程序风噪声Image构造。
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat, // 世界X跨纹理周期时无缝回绕。
        address_mode_v: ImageAddressMode::Repeat, // 世界Z跨纹理周期时无缝回绕。
        address_mode_w: ImageAddressMode::Repeat, // 二维纹理不使用W但保持描述完整。
        mag_filter: ImageFilterMode::Linear,      // 放大时平滑相邻八位格点值。
        min_filter: ImageFilterMode::Linear,      // 缩小时继续使用连续风场而非最近点跳变。
        mipmap_filter: ImageFilterMode::Linear,   // 未来若增加mip仍保持层间平滑。
        ..default()                               // 其余LOD和各向异性采用Bevy稳定默认值。
    }); // 完成无缝重复线性采样器配置。
    image // 返回可直接登记到Assets<Image>的完整图片。
} // 完成风噪声Image构造。

/// 生成四通道固定周期噪声字节。
fn build_tree_wind_noise_pixels() -> Vec<u8> {
    // 容量严格等于512×512×4，禁止循环期间反复扩容。
    let mut pixels = Vec::with_capacity((TREE_WIND_NOISE_SIZE * TREE_WIND_NOISE_SIZE * 4) as usize); // 预留固定一MiB结果。
    for y in 0..TREE_WIND_NOISE_SIZE {
        // 每一行使用纹素中心坐标，重复采样器可在首尾纹素间连续插值。
        let v = (y as f32 + 0.5) / TREE_WIND_NOISE_SIZE as f32; // 垂直坐标映射到零至一周期域。
        for x in 0..TREE_WIND_NOISE_SIZE {
            // 每个纹素独立计算四个固定尺度，不分配中间数组。
            let u = (x as f32 + 0.5) / TREE_WIND_NOISE_SIZE as f32; // 水平坐标映射到零至一周期域。
            for (frequency, seed) in TREE_WIND_NOISE_CHANNELS {
                // 四个通道共用同一周期算法但使用互异频率与种子。
                let value = periodic_value_noise(u, v, frequency, seed); // 计算当前通道连续零至一值。
                pixels.push((value * 255.0).round() as u8); // 单次量化为线性八位纹理通道。
            } // 完成当前纹素四个通道写入。
        } // 完成当前纹理行全部像素。
    } // 完成固定图片全部行。
    pixels // 返回完整且长度严格匹配Image描述的RGBA字节。
} // 完成四通道风噪声字节生成。

/// 计算零至一周期域上的确定性二维值噪声。
fn periodic_value_noise(
    u: f32,         // 水平周期坐标，任意有限值都会回绕到零至一。
    v: f32,         // 垂直周期坐标，任意有限值都会回绕到零至一。
    frequency: u32, // 周期内格点数决定当前噪声空间尺度。
    seed: u32,      // 固定种子保证跨进程和跨地图结果一致。
) -> f32 {
    // 内部调用全部使用非零常量，max仍防止未来测试或重构引入除零。
    let frequency = frequency.max(1); // 保证取模和网格缩放始终合法。
    let wrapped_u = if u.is_finite() {
        u.rem_euclid(1.0)
    } else {
        0.0
    }; // 非有限水平坐标安全回退周期原点。
    let wrapped_v = if v.is_finite() {
        v.rem_euclid(1.0)
    } else {
        0.0
    }; // 非有限垂直坐标安全回退周期原点。
    let x = wrapped_u * frequency as f32; // 把水平周期坐标映射到格点空间。
    let y = wrapped_v * frequency as f32; // 把垂直周期坐标映射到格点空间。
    let x0 = x.floor() as u32 % frequency; // 当前水平格点在周期边界安全回绕。
    let y0 = y.floor() as u32 % frequency; // 当前垂直格点在周期边界安全回绕。
    let x1 = (x0 + 1) % frequency; // 右侧格点越界时回到首列。
    let y1 = (y0 + 1) % frequency; // 下侧格点越界时回到首行。
    let tx = quintic_smooth(x.fract()); // 水平使用二阶导连续的五次平滑权重。
    let ty = quintic_smooth(y.fract()); // 垂直使用同一五次平滑权重。
    let top = linear_interpolate(
        lattice_hash_unit(x0, y0, seed), // 当前单元左上确定性格点值。
        lattice_hash_unit(x1, y0, seed), // 当前单元右上确定性格点值。
        tx,                              // 水平五次平滑权重。
    ); // 完成单元上边插值。
    let bottom = linear_interpolate(
        lattice_hash_unit(x0, y1, seed), // 当前单元左下确定性格点值。
        lattice_hash_unit(x1, y1, seed), // 当前单元右下确定性格点值。
        tx,                              // 与上边共享水平权重保持连续。
    ); // 完成单元下边插值。
    linear_interpolate(top, bottom, ty).clamp(0.0, 1.0) // 沿垂直方向完成周期双线性值噪声。
} // 完成周期值噪声计算。

/// 把二维整数格点与种子稳定映射到零至一伪随机值。
fn lattice_hash_unit(
    x: u32,    // 周期格点水平索引。
    y: u32,    // 周期格点垂直索引。
    seed: u32, // 当前噪声通道固定种子。
) -> f32 {
    // 只使用定义良好的wrapping整数运算，调试与发布构建得到完全相同结果。
    let mut value = seed ^ x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B); // 混合坐标和种子。
    value ^= value >> 16; // 第一次异或移位扩散高低位。
    value = value.wrapping_mul(0x7FEB_352D); // 第一次乘法雪崩降低相邻格点相关性。
    value ^= value >> 15; // 第二次异或移位继续扩散位模式。
    value = value.wrapping_mul(0x846C_A68B); // 第二次乘法雪崩形成稳定均匀散列。
    value ^= value >> 16; // 最后折叠高位完成散列。
    value as f32 / u32::MAX as f32 // 规范化为零至一闭区间附近的浮点值。
} // 完成格点伪随机值计算。

/// 零至一五次平滑曲线，首尾一阶和二阶导数均为零。
fn quintic_smooth(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0); // 先限制定义域避免调用者输入过冲。
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0) // 使用经典五次平滑多项式消除格线导数折点。
} // 完成五次平滑权重计算。

/// 一维线性插值辅助函数。
fn linear_interpolate(from: f32, to: f32, factor: f32) -> f32 {
    from + (to - from) * factor // 四角插值统一使用同一表达式保持周期连续。
} // 完成一维线性插值。

#[cfg(test)] // 固定程序纹理回归不启动窗口、RenderApp或真实GPU。
mod tests {
    // 测试直接读取私有生成器，锁定尺寸、格式、周期和确定性。
    use super::*; // 引入正式噪声生成和Image构造入口。

    #[test] // 相同固定种子和尺寸必须逐字节复现同一纹理。
    fn tree_wind_noise_pixels_are_deterministic() {
        let first = build_tree_wind_noise_pixels(); // 生成第一份完整固定风噪声。
        let second = build_tree_wind_noise_pixels(); // 生成第二份相同输入风噪声。
        assert_eq!(first, second); // 跨地图重进和重启依赖逐字节确定性。
        assert_eq!(
            first.len(),
            (TREE_WIND_NOISE_SIZE * TREE_WIND_NOISE_SIZE * 4) as usize
        ); // RGBA8长度必须严格匹配Image尺寸。
    } // 完成噪声字节确定性回归。

    #[test] // 周期坐标平移整数周期后必须返回同一连续值。
    fn periodic_noise_wraps_exactly_on_both_axes() {
        let base = periodic_value_noise(0.125, 0.625, 16, 0x1234_5678); // 使用二进制可精确表示的周期域采样点。
        let wrapped_u = periodic_value_noise(1.125, 0.625, 16, 0x1234_5678); // 水平平移一个完整周期。
        let wrapped_v = periodic_value_noise(0.125, -0.375, 16, 0x1234_5678); // 垂直反向平移一个完整周期。
        assert!((base - wrapped_u).abs() <= f32::EPSILON); // 水平回绕不得改变结果。
        assert!((base - wrapped_v).abs() <= f32::EPSILON); // 垂直回绕不得改变结果。
    } // 完成双轴周期回绕回归。

    #[test] // 图片格式和采样器必须匹配顶点Shader的线性无缝采样假设。
    fn tree_wind_noise_image_uses_linear_repeat_rgba8() {
        let image = build_tree_wind_noise_image(); // 构造正式程序Image但不登记Assets。
        assert_eq!(image.texture_descriptor.size.width, TREE_WIND_NOISE_SIZE); // 图片宽度必须保持固定五百一十二。
        assert_eq!(image.texture_descriptor.size.height, TREE_WIND_NOISE_SIZE); // 图片高度必须保持固定五百一十二。
        assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba8Unorm); // 噪声通道不得经过sRGB转换。
        let ImageSampler::Descriptor(sampler) = image.sampler else {
            panic!("tree wind noise must use an explicit repeat sampler"); // 默认采样器无法证明无缝世界滚动。
        }; // 取得正式自定义采样器描述。
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat); // 世界X必须周期重复。
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat); // 世界Z必须周期重复。
        assert_eq!(sampler.mag_filter, ImageFilterMode::Linear); // 放大时必须线性过滤八位噪声。
        assert_eq!(sampler.min_filter, ImageFilterMode::Linear); // 缩小时必须线性过滤八位噪声。
    } // 完成程序Image格式和采样器回归。
} // 结束固定风噪声测试模块。
