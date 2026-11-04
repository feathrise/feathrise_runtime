//! P4表面天气质量预算值；CPU水花和GPU雨纹共用同一个配置来源。

#[derive(Clone, Copy, Debug, PartialEq)] // 固定小值可直接比较，不创建运行时资源或分配。
pub struct SurfaceEffectsQualityProfile {
    // 不依赖游戏设置枚举，环境能力仍可独立复用。
    pub rain_range: f32,          // GPU解析雨纹最远米数。
    pub rain_density: f32,        // 相对满雨的候选接纳倍率。
    pub splash_capacity: usize,   // 当前档位允许的存活水滴数，不扩大48槽物理池。
    pub splash_candidates: usize, // 每个发射时刻最多检查的局部候选数。
    pub splash_interval: f32,     // 发射调度间隔秒，关闭档同样提供正数。
    pub splash_distance: f32,     // 几何水滴距离主相机的最远米数。
} // 完成共享质量预算。
