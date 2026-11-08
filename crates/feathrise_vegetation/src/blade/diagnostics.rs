//! GPU草地诊断采样配置。
//! 【功能设计】正式游戏默认每四个渲染帧异步采样一次；自动运动烟雾可临时改为逐帧，仍不等待GPU。

use bevy::prelude::Resource; // 主世界持有配置并由提取插件复制到RenderApp。
use bevy::render::extract_resource::ExtractResource; // 渲染图节点读取同一份固定采样间隔。

/// GPU状态异步回读的渲染帧采样间隔。
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Eq, PartialEq)]
#[extract_app(bevy::render::RenderApp)]
pub struct GpuBladeReadbackConfig {
    frame_interval: u64, // 一表示逐帧尝试采样，正式默认四；槽位忙仍只丢诊断而不阻塞绘制。
}

impl GpuBladeReadbackConfig {
    /// 创建合法的非零采样间隔。
    pub fn with_frame_interval(frame_interval: u64) -> Self {
        Self {
            frame_interval: frame_interval.max(1), // 零不能参与整除，统一净化为逐帧采样。
        }
    }

    /// 自动运动验收使用逐渲染帧采样，尽可能捕获瞬时空草样本。
    pub fn every_frame() -> Self {
        Self::with_frame_interval(1)
    }

    /// 返回渲染图使用的非零间隔。
    pub(crate) fn frame_interval(self) -> u64 {
        self.frame_interval
    }
}

impl Default for GpuBladeReadbackConfig {
    fn default() -> Self {
        Self::with_frame_interval(4) // F3默认十五赫兹上限足够观察且降低驱动映射开销。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readback_interval_is_always_nonzero() {
        assert_eq!(GpuBladeReadbackConfig::default().frame_interval(), 4);
        assert_eq!(GpuBladeReadbackConfig::every_frame().frame_interval(), 1);
        assert_eq!(
            GpuBladeReadbackConfig::with_frame_interval(0).frame_interval(),
            1
        );
    }
}
