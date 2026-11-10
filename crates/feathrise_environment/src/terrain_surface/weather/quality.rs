//! P4固定四档表面效果预算；切档只改可见成本，不改天气和积水历史。

use super::types::SurfaceEffectsQualityProfile; // 统一CPU/GPU预算值类型。

pub fn surface_effects_quality_profile(quality: f32) -> SurfaceEffectsQualityProfile {
    // 输入沿用Low0/Medium1/High2/Ultra3共享编码。
    let tier = if quality.is_finite() {
        quality.clamp(0.0, 3.0) as u32
    } else {
        0
    }; // 损坏质量编码回落最低成本。
    let (
        rain_range,
        rain_density,
        splash_capacity,
        splash_candidates,
        splash_interval,
        splash_distance,
    ) = match tier {
        // 表面效果唯一档位表。
        0 => (14.0, 0.35, 0, 0, 0.1, 0.0), // 低档保留短距稀疏雨纹与全部湿润积水主体。
        1 => (24.0, 0.55, 0, 0, 0.1, 0.0), // 中档扩大细纹范围，仍不发离水几何。
        2 => (34.0, 0.75, 24, 2, 0.1, 12.0), // 高档每秒最多20候选，24滴上限。
        _ => (44.0, 0.95, 48, 4, 0.05, 20.0), // 极高每秒最多80候选，保持既定48槽硬上限。
    }; // 所有参数均为常量，不按场景像素或实体数计算。
    SurfaceEffectsQualityProfile {
        // 一次返回完整档位，避免CPU水花和GPU雨纹独立猜测。
        rain_range,        // 雨纹最大距离。
        rain_density,      // 候选接纳倍率。
        splash_capacity,   // 存活槽数。
        splash_candidates, // 单批查询上界。
        splash_interval,   // 最小调度间隔。
        splash_distance,   // 主相机近景范围。
    } // 完成无分配预算组装。
} // 完成表面质量映射。

#[cfg(test)] // 不启动渲染的预算契约回归。
mod tests {
    // 锁定低档核心效果与最坏预算。
    use super::*; // 测试真实映射函数。
    #[test] // 质量提高可增加可见成本，但不能突破批准的固定池。
    fn surface_quality_retains_core_and_caps_peak_cost() {
        // 对完整四档同时检查跨档约束。
        let profiles = [0.0, 1.0, 2.0, 3.0].map(surface_effects_quality_profile); // 运行生产映射。
        assert!(
            profiles
                .iter()
                .all(|p| p.rain_range > 0.0 && p.rain_density > 0.0)
        ); // 最低档仍有核心雨纹。
        assert!(profiles.iter().all(|p| p.splash_capacity <= 48
            && p.splash_candidates <= 4
            && p.splash_interval >= 0.05)); // 固定最坏预算不能被任何档位突破。
        assert!(
            profiles
                .windows(2)
                .all(|pair| pair[0].rain_range <= pair[1].rain_range
                    && pair[0].rain_density <= pair[1].rain_density)
        ); // 降档不增加GPU覆盖和密度。
        assert_eq!(profiles[0].splash_capacity + profiles[1].splash_capacity, 0); // 中低档不产生几何水花。
        assert!(
            profiles[2].splash_candidates as f32 / profiles[2].splash_interval
                < profiles[3].splash_candidates as f32 / profiles[3].splash_interval
        ); // 高档确实比极高降低发射查询成本。
        assert_eq!(surface_effects_quality_profile(f32::NAN), profiles[0]); // 坏输入不能进入最高成本。
        assert_eq!(surface_effects_quality_profile(-1.0), profiles[0]); // 负值保守降级。
        assert_eq!(surface_effects_quality_profile(99.0), profiles[3]); // 超范围也不扩大现有预算。
    } // 完成质量策略验收。
} // 结束无窗口预算测试。
