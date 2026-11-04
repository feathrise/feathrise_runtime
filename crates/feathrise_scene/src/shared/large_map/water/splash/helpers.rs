//! 雨滴随机候选与保守积水采样纯函数。
use super::super::types::DynamicWaterGpuFrame;
use bevy::{math::Vec4Swizzles, prelude::*}; // 世界范围与R8图。 // 与顶点着色器共享当前实际材质波参数。

pub(super) fn splash_wind_velocity(wind: Vec4) -> Vec3 {
    // 发射瞬间采样风，不改变已发射弹道。
    let direction = Vec2::new(wind.x, wind.y); // 天气契约中的世界XZ风向。
    if !direction.is_finite() || !wind.z.is_finite() {
        return Vec3::ZERO;
    } // 坏调试值不得产生非有限实体位置。
    let scale = direction.abs().max_element(); // 先缩放再归一化，避免极大有限输入长度溢出。
    if scale <= f32::EPSILON {
        return Vec3::ZERO;
    } // 零风向不人为指定偏转。
    let velocity = (direction / scale).normalize_or_zero() * wind.z.clamp(0.0, 1.0) * 0.3; // 风偏移初速上限0.3米/秒，寿命内最多7.2厘米。
    Vec3::new(velocity.x, 0.0, velocity.y) // 风不改竖直初速或水花峰高。
} // 完成有界发射风速。

pub(super) fn elapsed_delta(previous: Option<f32>, now: f32) -> f32 {
    // 存活寿命消费完整时间差，不因低帧延长。
    previous.map_or(0.0, |old| (now - old).rem_euclid(4096.0)) // 共享时钟自然回绕保持连续。
} // 完成视觉帧时差。

fn smooth(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
} // 同源三次衰减。

pub(in super::super) fn player_height(frame: &DynamicWaterGpuFrame, point: Vec2) -> f32 {
    // 固定八槽事件与GPU一致的高度。
    let mut height = 0.0; // 累计当前有效事件。
    for ripple in frame
        .ripples
        .iter()
        .take((frame.ripple_limits.z as usize).min(8))
    {
        // 不访问无效槽。
        if ripple.z < 0.0 || ripple.z >= frame.ripple_shape.x {
            continue;
        } // 过期事件不贡献。
        let offset = point.distance(ripple.xy()) - ripple.z * frame.ripple_shape.y; // 波前米制偏移。
        if offset.abs() > frame.ripple_shape.z {
            continue;
        } // 环带外快速拒绝。
        height += (offset * std::f32::consts::TAU / frame.ripple_shape.w).cos()
            * (1.0 - smooth(offset.abs() / frame.ripple_shape.z))
            * smooth(ripple.z / frame.ripple_limits.x)
            * (1.0 - ripple.z / frame.ripple_shape.x).clamp(0.0, 1.0)
            * ripple.w; // 同源攻击、空间包络及寿命。
    } // 完成八槽求和。
    height.clamp(-frame.ripple_limits.y, frame.ripple_limits.y) // 与GPU最终波高限幅一致。
} // 完成事件波高度。

pub(super) fn still_displacement(
    frame: &DynamicWaterGpuFrame,
    elapsed: f32,
    point: Vec2,
    alpha: f32,
) -> Vec3 {
    // 四组静水宏观波与水平Gerstner偏移。
    let edge = alpha.clamp(0.0, 1.0).powf(frame.time_and_visual.z.max(0.1)); // 实际透明岸带波幅。
    let mut displacement = Vec3::ZERO; // 局部位移从零开始。
    for (index, wave) in frame.wave_directions_amplitudes.iter().enumerate() {
        // 固定四组波。
        let cycles = (frame.wave_speeds[index] * wave.w * 3600.0 / std::f32::consts::TAU).round(); // GPU小时回绕相位量化。
        let phase = point.dot(wave.xy()) * wave.w
            - (elapsed / 3600.0 * cycles).rem_euclid(1.0) * std::f32::consts::TAU; // 同源周期相位。
        displacement.y += phase.sin() * wave.z * edge; // 真实竖向波高。
        let horizontal = wave.xy() * (phase.cos() * wave.z * frame.wave_steepness[index] * edge); // 与GPU相同水平位移。
        displacement.x += horizontal.x;
        displacement.z += horizontal.y; // 保留真实落点XZ。
    } // 完成固定四组宏观波。
    displacement.y += player_height(frame, point) * edge; // 玩家事件不改变槽池，仅采样其高度。
    displacement // 完成同源点位移。
} // 结束静水采样。
#[cfg(test)]
use super::constants::*; // 回归固定寿命。
pub(super) fn random(seed: u32) -> f32 {
    // 整数哈希不依赖线程或帧数随机源。
    let mut value = seed.wrapping_mul(747796405).wrapping_add(2891336453); // PCG风格混合输入。
    value = ((value >> ((value >> 28) + 4)) ^ value).wrapping_mul(277803737); // 扩散低位。
    ((value >> 22) ^ value) as f32 / u32::MAX as f32 // 映射到零至一。
} // 结束候选哈希。

/// 采用噪声最坏值和完全覆盖门，CPU不尝试复制GPU浮点sin哈希。
pub(super) fn puddle_core(potential: f32, amount: f32, normal_y: f32) -> bool {
    // 保守子集保证水滴不会落在程序噪声掏空区域。
    amount >= 0.08
        && normal_y >= 0.997
        && potential - 0.14 >= 1.02 - amount.clamp(0.0, 1.0) * 0.96 + 0.10 // 要求完整水膜。
} // 结束积水核心判定。

pub(super) fn mask_potential(image: &Image, bounds: Vec4, point: Vec2) -> Option<f32> {
    // 固定四像素读取，无整图复制。
    let size = Vec2::new(bounds.z - bounds.x, bounds.w - bounds.y); // 当前完整图的世界范围。
    let uv = (point - bounds.xy()) / size; // 与地形Shader一致的世界XZ投影。
    if !uv.is_finite() || uv.min_element() <= 0.0 || uv.max_element() >= 1.0 {
        return None;
    } // 边界保守拒绝。
    if image.texture_descriptor.format != bevy::render::render_resource::TextureFormat::R8Unorm {
        return None;
    } // 不猜测其他纹理布局。
    let width = image.texture_descriptor.size.width as usize; // 基级R8行宽。
    let height = image.texture_descriptor.size.height as usize; // 基级R8行数。
    if width < 2 || height < 2 {
        return None;
    } // 仅完整作者图可采样。
    let pixel = uv * Vec2::new(width as f32, height as f32) - Vec2::splat(0.5); // 纹素中心约定。
    let x = pixel.x.floor().clamp(0.0, (width - 2) as f32) as usize; // 保守相邻四点。
    let y = pixel.y.floor().clamp(0.0, (height - 2) as f32) as usize; // 保证所有读取在基级内。
    let bytes = image.data.as_ref()?; // GPU专有图缺CPU数据时跳过。
    let top_left = *bytes.get(y * width + x)?; // 读取左上角。
    let top_right = *bytes.get(y * width + x + 1)?; // 读取右上角。
    let bottom_left = *bytes.get((y + 1) * width + x)?; // 读取左下角。
    let bottom_right = *bytes.get((y + 1) * width + x + 1)?; // 读取右下角。
    Some(top_left.min(top_right).min(bottom_left).min(bottom_right) as f32 / 255.0) // 固定四值无分配最小值比双线性更保守。
} // 结束固定像素采样。

#[cfg(test)] // 纯数学边界无需GPU。
mod tests {
    // 真实离水参数回归。
    use super::*; // 测试生产helper与常量。
    #[test]
    fn puddle_splash_rejects_edges_and_slopes() {
        // 最坏噪声下不误生。
        assert!(!puddle_core(1.0, 0.0, 1.0)); // 干燥绝不发射。
        assert!(!puddle_core(1.0, 1.0, 0.99)); // 坡面必须排除。
        assert!(!puddle_core(0.2, 0.5, 1.0)); // 扩张边缘必须排除。
        assert!(puddle_core(1.0, 1.0, 1.0)); // 完整强潜力水膜可发射。
    } // 完成核心测试。
    #[test]
    fn splash_lifetime_ends_above_surface() {
        // 保证水滴不会穿入地面。
        assert!(1.25 * LIFE - 4.9 * LIFE * LIFE > 0.0); // 最慢初速在隐藏前仍离水。
        assert!(random(1) != random(2)); // 连续事件不固定重复同点。
    } // 完成弹道边界。
    #[test]
    fn splash_mask_reads_conservative_four_pixels_and_rejects_bad_images() {
        // 实际R8布局与边界安全测试。
        use bevy::{
            asset::RenderAssetUsages,
            render::render_resource::{Extent3d, TextureDimension, TextureFormat},
        }; // 构建与生产一致的R8图片。
        let mut image = Image::new(
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![255, 128, 64, 192],
            TextureFormat::R8Unorm,
            RenderAssetUsages::all(),
        ); // 四点使用不同潜力以发现错误单点或均值读取。
        let bounds = Vec4::new(0.0, 0.0, 10.0, 10.0); // 清晰的世界XZ边界。
        assert_eq!(
            mask_potential(&image, bounds, Vec2::splat(5.0)),
            Some(64.0 / 255.0)
        ); // 应取四点最小值而不是双线性均值。
        assert_eq!(mask_potential(&image, bounds, Vec2::ZERO), None); // 世界范围边缘保守拒绝。
        assert_eq!(mask_potential(&image, Vec4::ZERO, Vec2::ONE), None); // 零世界跨度不除零后继续索引。
        image.data = Some(vec![255]); // 损坏或尚未完整上传的CPU数据。
        assert_eq!(mask_potential(&image, bounds, Vec2::splat(5.0)), None); // 截断像素安全失败而非越界。
        image.texture_descriptor.size.width = 1; // 作者分辨率不合法。
        assert_eq!(mask_potential(&image, bounds, Vec2::splat(5.0)), None); // 无法建立四邻域时明确拒绝。
    } // 完成R8采样回归。
} // 结束测试模块。
