//! 水下距离雾与白平衡的纯参数计算；深度采用静止水面，避免每个小波峰反复切换整屏外观。
use bevy::prelude::*;

#[derive(Clone, Copy)]
pub(super) struct WaterViewSample {
    pub depth: f32,
    pub color: Color,
}

pub(super) fn profile(sample: WaterViewSample, sky: Color) -> (DistanceFog, Vec2) {
    // 前三厘米保留空气视角，四十厘米内连续入水；不把人物入水等同于相机入水。
    let amount = ((sample.depth - 0.03) / 0.37).clamp(0.0, 1.0);
    let amount = amount * amount * (3.0 - 2.0 * amount);
    let color = sample.color.to_linear();
    let sky = sky.to_linear();
    let illumination = (sky.red * 0.2126 + sky.green * 0.7152 + sky.blue * 0.0722).clamp(0.0, 1.0);
    let peak = color.red.max(color.green).max(color.blue).max(0.001);
    let fog = DistanceFog {
        // 水色只决定相对吸收色调，亮度跟随当前昼夜，夜间不会凭空产生明亮青色雾墙。
        color: Color::linear_rgba(
            color.red / peak * illumination * 0.18,
            color.green / peak * illumination * 0.18,
            color.blue / peak * illumination * 0.18,
            amount * 0.97,
        ),
        directional_light_color: Color::NONE,
        directional_light_exponent: 8.0,
        falloff: FogFalloff::Exponential { density: 0.065 + sample.depth.min(12.0) * 0.004 },
    };
    (fog, Vec2::new(-0.035, -0.020) * amount)
}

pub(super) fn same_fog(a: &DistanceFog, b: &DistanceFog) -> bool {
    a.color == b.color && a.directional_light_color == b.directional_light_color
        && a.directional_light_exponent == b.directional_light_exponent
        && matches!((&a.falloff, &b.falloff),
            (FogFalloff::Exponential { density: a }, FogFalloff::Exponential { density: b }) if a == b)
}
