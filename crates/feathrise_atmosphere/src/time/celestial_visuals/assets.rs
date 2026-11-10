//! 可见天体资产：构造曝光校准的白热太阳核心、径向柔光程序纹理和跨场景强句柄缓存。

use bevy::asset::RenderAssetUsages; // 程序太阳柔光纹理需要同时保留主世界测试数据并上传渲染世界。
use bevy::image::{
    ImageAddressMode,       // ClampToEdge禁止透明柔光边缘在圆盘UV外重复。
    ImageFilterMode,        // 线性过滤平滑一百二十八像素径向渐变。
    ImageSampler,           // 运行时Image显式携带柔光采样方式。
    ImageSamplerDescriptor, // 描述U/V/W寻址与放大缩小过滤。
}; // 结束程序太阳柔光图像采样类型导入。
use bevy::prelude::*; // Assets句柄、Image、StandardMaterial、Color与AlphaMode来自Bevy预导入。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat}; // 程序柔光纹理需要明确二维RGBA8尺寸与格式。

/// 太阳核心最亮线性色通道使用的曝光校准HDR峰值。
const SUN_CORE_HDR_PEAK: f32 = 8_192.0; // Bevy默认EV100 9.7会把输入缩小约一千倍，本峰值让正午核心经过曝光后仍保持白热且保留暮色色相。
/// 月亮圆盘最亮线性色通道使用的曝光校准HDR峰值。
const MOON_CORE_HDR_PEAK: f32 = 1_024.0; // 默认曝光后维持约一倍显示白点，确保月盘清晰但仍明显弱于太阳核心。
/// 月亮圆盘固定冷白基础色。
pub(super) const MOON_VISUAL_COLOR: Color = Color::srgb(0.78, 0.84, 0.96); // 固定圆月外观与夜间冷蓝照明保持协调，不把月盘误做成第二个太阳。
/// 太阳视觉圆盘相对物理光源角直径的艺术放大倍率。
pub(super) const SUN_VISUAL_RADIUS_MULTIPLIER: f32 = 2.0; // 只放大画面圆盘，不写回SunDisk或改变光栅与Solari软阴影范围。
/// 太阳柔光纹理单边像素数。
const SUN_HALO_TEXTURE_SIZE: u32 = 128; // 固定小纹理只在首张动态场景出现时生成一次。
/// 太阳柔光相对放大后视觉圆盘半径的倍率。
pub(super) const SUN_HALO_RADIUS_MULTIPLIER: f32 = 8.0; // 柔光跟随两倍视觉核心形成更接近天空盒太阳的宽阔散射感。
/// 太阳柔光中心相对不透明核心的相机距离倍率。
pub(super) const SUN_HALO_DISTANCE_MULTIPLIER: f32 = 1.01; // 把透明柔光稳定放到核心后方并按距离同比缩放，消除共面三角片在时域抖动下的闪烁。

/// 从同帧最终太阳方向光颜色提取不含能量大小的线性色相。
pub(super) fn sun_visual_tint(final_light_color: Color) -> Color {
    let linear = final_light_color.to_linear(); // Bevy PBR也在线性空间消费方向光颜色，视觉圆盘必须使用同一色彩空间。
    if !linear.red.is_finite() || !linear.green.is_finite() || !linear.blue.is_finite() {
        return Color::WHITE; // 损坏的外部光色回退中性白，禁止无效值污染StandardMaterial资产。
    }
    let red = linear.red.max(0.0); // 负红通道没有物理意义，统一钳制到零。
    let green = linear.green.max(0.0); // 负绿通道同样不能进入可见太阳材质。
    let blue = linear.blue.max(0.0); // 负蓝通道同样不能进入可见太阳材质。
    let peak = red.max(green).max(blue); // 取最亮通道剥离方向光颜色自身的整体能量大小。
    if peak <= f32::EPSILON {
        return Color::BLACK; // 合法全黑作者光也必须得到全黑视觉，禁止圆盘重新产生与水面不一致的白色。
    }
    Color::linear_rgb(red / peak, green / peak, blue / peak) // 保持方向光色相比例并把最亮通道规范到一。
}

/// 把合法天体色相提升到指定的曝光校准HDR峰值。
fn celestial_core_hdr_color(source_color: Color, hdr_peak: f32) -> Color {
    let tint = sun_visual_tint(source_color).to_linear(); // 复用同一有限值校验与线性色相归一规则，太阳和月亮不会因输入能量重复缩放。
    Color::linear_rgb(
        tint.red * hdr_peak,   // 红色通道保留来源天体色相比例。
        tint.green * hdr_peak, // 绿色通道使用同一个曝光峰值缩放。
        tint.blue * hdr_peak,  // 蓝色通道同样保留来源色相比例。
    ) // 返回交给StandardMaterial和相机物理曝光处理的HDR颜色。
}

/// 把最终太阳方向光色相提升到白热核心使用的HDR峰值。
pub(super) fn sun_core_hdr_color(final_light_color: Color) -> Color {
    celestial_core_hdr_color(final_light_color, SUN_CORE_HDR_PEAK) // 太阳保留同帧方向光色相并使用较高白热峰值。
}

/// 把固定冷白月色提升到清晰但弱于太阳的HDR峰值。
pub(super) fn moon_core_hdr_color() -> Color {
    celestial_core_hdr_color(MOON_VISUAL_COLOR, MOON_CORE_HDR_PEAK) // 月盘不依赖月光照度曲线，显隐窗口内始终维持稳定可辨识外观。
}

/// 可见天体资产强句柄缓存；首个动态场景出现时才创建。
#[derive(Default)] // 晴天、固定场景和菜单切换期间仍保留网格、纹理和三份材质供后续复用。
pub(in crate::time) struct CelestialVisualAssetCache {
    pub(super) mesh: Option<Handle<Mesh>>, // 太阳核心、太阳柔光和月亮共享一个单位圆盘网格。
    pub(super) sun_material: Option<Handle<StandardMaterial>>, // 太阳物理核心使用独立HDR白热前向无光照材质。
    pub(super) sun_halo_texture: Option<Handle<Image>>, // 太阳柔光使用运行时生成的径向透明渐变。
    pub(super) sun_halo_material: Option<Handle<StandardMaterial>>, // 柔光层使用独立透明无光照材质。
    pub(super) moon_material: Option<Handle<StandardMaterial>>, // 月亮使用独立曝光校准的冷白前向无光照材质。
}

/// 创建视觉不透明但固定走透明前向阶段的太阳或月亮核心材质。
pub(super) fn celestial_core_material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color, // 太阳和月亮都允许HDR线性颜色超过一，以抵消相机物理曝光造成的黑盘。
        alpha_mode: AlphaMode::Blend, // 输入Alpha保持一而画面完全不透明，但可绕开Solari延迟桥对远景装饰圆盘的接管。
        unlit: true,                  // 直接输出作者颜色，不接受环境光、方向光或Solari结果。
        fog_enabled: false, // 相机远景不被欧氏距离雾直接吃掉；云幕仍可通过深度与透明度遮挡。
        cull_mode: None,    // Circle正反面均可见，避免镜头旋转导致绕序方向翻转。
        ..default()         // 其余PBR通道保持关闭和默认值。
    } // 返回在Solari解析后绘制且仍接受场景深度遮挡的天体核心材质。
}

/// 创建带径向透明纹理并消费最终太阳色相的柔光材质。
pub(super) fn sun_halo_material(texture: Handle<Image>, tint: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: tint, // 中性程序纹理乘入同帧太阳方向光色相，傍晚柔光与水面直射高光保持一致。
        base_color_texture: Some(texture), // 径向渐隐纹理只提供白色强度与Alpha，不再固化正午色温。
        alpha_mode: AlphaMode::Blend, // 连续Alpha避免柔光外缘出现硬圆边。
        unlit: true,      // 柔光只是视觉叠加，不接受场景灯光或反向照亮世界。
        fog_enabled: false, // 远景柔光不能被相机距离雾整体抹除。
        cull_mode: None,  // 从Circle任意绕序方向都能稳定看见柔光。
        depth_bias: 0.0, // 核心与柔光改用明确世界空间距离分层，不再依赖远裁面附近精度相关的常量深度偏移。
        ..default()      // 其余PBR通道保持关闭和默认值。
    } // 返回可直接登记到Assets的透明柔光材质。
}

/// 生成确定性的中性径向太阳柔光纹理。
pub(super) fn build_sun_halo_image() -> Image {
    let mut pixels = Vec::with_capacity(
        (SUN_HALO_TEXTURE_SIZE * SUN_HALO_TEXTURE_SIZE * 4) as usize, // 一次预分配完整RGBA8像素缓冲。
    ); // 完成固定尺寸像素缓冲预分配。
    for y in 0..SUN_HALO_TEXTURE_SIZE {
        let normalized_y = ((y as f32 + 0.5) / SUN_HALO_TEXTURE_SIZE as f32) * 2.0 - 1.0; // 像素中心映射到负一至一垂直坐标。
        for x in 0..SUN_HALO_TEXTURE_SIZE {
            let normalized_x = ((x as f32 + 0.5) / SUN_HALO_TEXTURE_SIZE as f32) * 2.0 - 1.0; // 像素中心映射到负一至一水平坐标。
            let radius = (normalized_x * normalized_x + normalized_y * normalized_y).sqrt(); // 圆对称半径让任意相机滚转都得到相同柔光。
            let alpha = if radius >= 1.0 {
                0.0 // 单位圆外完全透明，Circle边界与纹理边界都不会留下方框或硬边。
            } else {
                let inner_glow = (-18.0 * radius * radius).exp() * 0.62; // 靠近视觉圆盘形成足够接近天空盒高光的白热乳光。
                let outer_glow = (1.0 - radius).powi(3) * 0.20; // 宽阔低透明尾部模拟晴空中的大范围散射。
                (inner_glow + outer_glow).clamp(0.0, 1.0) // 合并两级柔光并约束为合法Alpha。
            };
            pixels.extend_from_slice(&[
                255,                           // 中性红通道只承载径向强度，色相由材质动态乘入。
                255,                           // 中性绿通道避免程序纹理二次改变最终太阳光色相。
                255,                           // 中性蓝通道确保水面与太阳视觉共享同一颜色来源。
                (alpha * 255.0).round() as u8, // 连续透明度由标准Alpha混合叠加到现有天空上。
            ]); // 完成当前径向柔光像素写入。
        } // 结束当前纹理行像素生成。
    } // 结束全部径向柔光像素生成。
    let mut image = Image::new(
        Extent3d {
            width: SUN_HALO_TEXTURE_SIZE, // 固定一百二十八像素宽度足够平滑低频渐变。
            height: SUN_HALO_TEXTURE_SIZE, // 正方形尺寸保证径向圆对称。
            depth_or_array_layers: 1,     // 太阳柔光只使用单张二维纹理层。
        }, // 结束程序纹理尺寸描述。
        TextureDimension::D2, // StandardMaterial采样普通二维基础色纹理。
        pixels,               // 传入完整中性RGBA8像素数据。
        TextureFormat::Rgba8UnormSrgb, // RGB按sRGB解释，Alpha继续作为线性透明遮罩。
        RenderAssetUsages::default(), // 主世界测试可读并允许渲染世界上传。
    ); // 完成太阳柔光Image构造。
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge, // 水平方向固定透明边缘，禁止重复出第二圈光晕。
        address_mode_v: ImageAddressMode::ClampToEdge, // 垂直方向同样钳制透明边缘。
        address_mode_w: ImageAddressMode::ClampToEdge, // 二维纹理不使用W，但保持采样描述完整一致。
        mag_filter: ImageFilterMode::Linear,           // 放大时平滑相邻Alpha，消除像素环带。
        min_filter: ImageFilterMode::Linear,           // 缩小时平滑融合径向渐变。
        mipmap_filter: ImageFilterMode::Linear,        // 未来生成mipmap后仍保持层级连续。
        ..default()                                    // 其余LOD与各向异性采用Bevy稳定默认值。
    }); // 完成不重复线性采样器配置。
    image // 返回可直接登记到Assets<Image>的太阳柔光纹理。
}

#[cfg(test)] // 资产纯函数测试不需要窗口、GPU或RenderApp。
mod tests {
    use super::*; // 直接验证本模块正式材质、像素和采样器构造。

    #[test]
    fn sun_visual_uses_hdr_core_and_soft_radial_halo() {
        let mesh = Mesh::from(Circle::new(1.0)); // 正式三层视觉共享Bevy单位Circle网格。
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some()); // 柔光纹理依赖UV0，升级Bevy后不能静默退化为无纹理材质。
        let core = celestial_core_material(sun_core_hdr_color(Color::WHITE)); // 中性正午光应构造三通道一致的白热前向核心材质。
        let core_linear = core.base_color.to_linear(); // HDR阈值必须在线性色彩空间验证。
        assert!(core_linear.red > 1.0 && core_linear.green > 1.0 && core_linear.blue > 1.0); // 三通道都越过显示白点，不能再得到棕黄色硬币。
        let default_exposed_peak = core_linear.red * bevy::camera::Exposure::default().exposure(); // 按项目相机实际继承的Bevy默认物理曝光验证屏幕前能量。
        assert!(default_exposed_peak >= 4.0); // 正午核心曝光后仍需明显高于显示白点，禁止再次成为光晕前的黑色遮挡盘。
        assert!((core_linear.green / core_linear.red - 1.0).abs() < f32::EPSILON); // 中性方向光的绿色必须精确保持中性。
        assert!((core_linear.blue / core_linear.red - 1.0).abs() < f32::EPSILON); // 中性方向光的蓝色必须精确保持中性。
        assert_eq!(core.alpha_mode, AlphaMode::Blend); // 视觉不透明核心必须留在Solari解析后的前向透明阶段。
        assert_eq!(core.base_color.alpha(), 1.0); // 使用透明阶段不能把核心本身变成半透明或重新露出静态天空太阳。
        assert!(core.unlit); // 核心亮度不能随方向光或夜间环境反向变化。
        let halo_material = sun_halo_material(Handle::default(), Color::WHITE); // 使用弱句柄即可验证材质状态，不需要RenderApp。
        assert_eq!(halo_material.alpha_mode, AlphaMode::Blend); // 柔光外缘必须使用连续透明混合。
        assert!(halo_material.base_color_texture.is_some()); // 柔光必须实际引用径向纹理而不是更大的纯色圆盘。
        assert!(halo_material.unlit); // 柔光不能成为额外场景光照来源。
        assert_eq!(halo_material.depth_bias, 0.0); // 远景太阳不得再依赖平台相关深度偏移掩盖共面闪烁。
        let image = build_sun_halo_image(); // 生成正式运行使用的确定性径向纹理。
        let data = image.data.as_ref().expect("太阳柔光必须保留CPU像素供测试"); // 主世界用途是程序纹理可验证性的前提。
        assert_eq!(
            data.len(),
            (SUN_HALO_TEXTURE_SIZE * SUN_HALO_TEXTURE_SIZE * 4) as usize,
        ); // 一百二十八方形RGBA8纹理必须包含完整像素数据。
        let center_alpha =
            ((SUN_HALO_TEXTURE_SIZE / 2 * SUN_HALO_TEXTURE_SIZE + SUN_HALO_TEXTURE_SIZE / 2) * 4
                + 3) as usize; // 取纹理中心附近像素的Alpha字节。
        let corner = 3_usize; // 左上角第一个像素的Alpha字节位于索引三。
        assert!(data[center_alpha] > 120); // 核心周围必须有足够明显的乳白柔光。
        assert_eq!(&data[center_alpha - 3..center_alpha], &[255, 255, 255]); // 纹理RGB必须完全中性，不能覆盖运行时太阳色相。
        assert_eq!(data[corner], 0); // 圆外边缘必须完全透明，禁止出现方框或硬圆边。
        let ImageSampler::Descriptor(sampler) = image.sampler else {
            panic!("太阳柔光必须使用自定义线性采样器"); // 默认采样不可依赖项目外部设置。
        }; // 结束自定义采样器存在性检查。
        assert_eq!(sampler.mag_filter, ImageFilterMode::Linear); // 放大时必须平滑径向渐变。
        assert_eq!(sampler.address_mode_u, ImageAddressMode::ClampToEdge); // 透明边缘不得重复生成第二圈光晕。
    }

    #[test]
    fn moon_visual_uses_weaker_exposure_calibrated_forward_core() {
        let moon = celestial_core_material(moon_core_hdr_color()); // 正式月盘同时应用冷白HDR颜色与Solari后的前向材质路径。
        let moon_linear = moon.base_color.to_linear(); // HDR强度与冷暖排序必须在线性色彩空间验证。
        let moon_peak = moon_linear.red.max(moon_linear.green).max(moon_linear.blue); // 最亮蓝通道代表固定圆月HDR峰值。
        let default_exposed_peak = moon_peak * bevy::camera::Exposure::default().exposure(); // 使用玩法和F12相机实际继承的默认物理曝光。
        assert!((0.75..2.0).contains(&default_exposed_peak)); // 月盘曝光后应清晰越过暗灰区，但不能达到太阳白热强度。
        assert!(moon_linear.blue > moon_linear.green && moon_linear.green > moon_linear.red); // 固定圆月继续保持低饱和冷白色相。
        assert!(moon_peak < SUN_CORE_HDR_PEAK); // 月亮HDR峰值必须显著低于太阳，避免夜空出现第二个太阳。
        assert_eq!(moon.alpha_mode, AlphaMode::Blend); // 月盘与太阳核心一样绕开Solari延迟桥造成的黑盘风险。
        assert_eq!(moon.base_color.alpha(), 1.0); // 前向透明阶段只用于选择渲染路径，月盘本身仍完全不透明。
        assert!(moon.unlit); // 月盘不接受自身月光或其他场景光的二次照明。
    }

    #[test]
    fn sun_visual_colors_preserve_final_directional_light_chromaticity() {
        let sunset_light = Color::srgb(1.0, 0.36, 0.10); // 使用正式低角度暖橙太阳色验证截图中的傍晚场景。
        let source = sunset_light.to_linear(); // PBR水面接收的方向光色彩比例在线性空间表达。
        let source_peak = source.red.max(source.green).max(source.blue); // 规范化后只比较色相而不比较灯光能量。
        let expected_red = source.red / source_peak; // 计算水面PBR使用的归一红色比例。
        let expected_green = source.green / source_peak; // 计算水面PBR使用的归一绿色比例。
        let expected_blue = source.blue / source_peak; // 计算水面PBR使用的归一蓝色比例。
        let halo = sun_visual_tint(sunset_light).to_linear(); // 柔光直接使用同一归一色相。
        let core = sun_core_hdr_color(sunset_light).to_linear(); // 核心只在同一色相上乘固定HDR峰值。
        assert!((halo.red - expected_red).abs() < 0.000_01); // 柔光红色比例必须等于最终方向光。
        assert!((halo.green - expected_green).abs() < 0.000_01); // 柔光绿色比例必须等于最终方向光。
        assert!((halo.blue - expected_blue).abs() < 0.000_01); // 柔光蓝色比例必须等于最终方向光。
        assert!((core.red / SUN_CORE_HDR_PEAK - expected_red).abs() < 0.000_01); // HDR核心红色比例保持一致。
        assert!((core.green / SUN_CORE_HDR_PEAK - expected_green).abs() < 0.000_01); // HDR核心绿色比例保持一致。
        assert!((core.blue / SUN_CORE_HDR_PEAK - expected_blue).abs() < 0.000_01); // HDR核心蓝色比例保持一致。
        assert!(core.red > core.green && core.green > core.blue); // 傍晚核心必须保留橙暖排序，不能重新退化为固定白色。
        assert_eq!(sun_visual_tint(Color::BLACK), Color::BLACK); // 合法黑色方向光不得回退成白色可见太阳。
    }
}
