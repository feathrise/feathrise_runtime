//! GPU草地诊断采样配置。
//! 【功能设计】正式游戏默认每四个渲染帧异步采样一次；自动运动烟雾可临时改为逐帧，仍不等待GPU。

use bevy::prelude::{Entity, Resource}; // 主世界持有配置并由提取插件复制到RenderApp。
use bevy::render::extract_resource::ExtractResource; // 渲染图节点读取同一份固定采样间隔。

/// 筛选和实际回读共用固定上限；诊断身份不会按地图规模增长。
pub const GPU_BLADE_READBACK_WINDOW_CAPACITY: usize = 32;

/// GPU状态异步回读的渲染帧采样间隔。
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Eq, PartialEq)]
#[extract_app(bevy::render::RenderApp)]
pub struct GpuBladeReadbackConfig {
    frame_interval: u64, // 一表示逐帧尝试采样，正式默认四；槽位忙仍只丢诊断而不阻塞绘制。
    selection_ticket: u64, // 零表示原全场聚合；显式票据随GPU copy冻结，防止迟到样本认领新目标。
    selected_count: u32,
    selected_main_entities: [u64; GPU_BLADE_READBACK_WINDOW_CAPACITY],
}

impl GpuBladeReadbackConfig {
    /// 创建合法的非零采样间隔。
    pub fn with_frame_interval(frame_interval: u64) -> Self {
        Self {
            frame_interval: frame_interval.max(1), // 零不能参与整除，统一净化为逐帧采样。
            selection_ticket: 0,
            selected_count: 0,
            selected_main_entities: [0; GPU_BLADE_READBACK_WINDOW_CAPACITY],
        }
    }

    /// 自动运动验收使用逐渲染帧采样，尽可能捕获瞬时空草样本。
    pub fn every_frame() -> Self {
        Self::with_frame_interval(1)
    }

    /// 只筛选诊断拷贝，不修改草窗、可见性或绘制；实体包含完整代次，不能以同索引新实体冒充。
    pub fn for_windows(frame_interval: u64, ticket: u64, windows: &[Entity]) -> Option<Self> {
        if ticket == 0 || windows.is_empty() || windows.len() > GPU_BLADE_READBACK_WINDOW_CAPACITY {
            return None;
        }
        let mut config = Self::with_frame_interval(frame_interval);
        config.selection_ticket = ticket;
        config.selected_count = windows.len() as u32;
        for (index, entity) in windows.iter().enumerate() {
            let bits = entity.to_bits();
            if config.selected_main_entities[..index].contains(&bits) { return None; }
            config.selected_main_entities[index] = bits;
        }
        Some(config)
    }

    /// 用于完整快照匹配；更改目标集合时调用方必须使用新票据。
    pub fn selection_ticket(self) -> u64 { self.selection_ticket }

    pub(crate) fn accepts_main_entity(self, entity: Entity) -> bool {
        self.selection_ticket == 0
            || self.selected_main_entities[..self.selected_count as usize].contains(&entity.to_bits())
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

    #[test]
    fn readback_selection_is_bounded_explicit_and_keeps_entity_generations() {
        let mut world = bevy::prelude::World::new();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        let selection = GpuBladeReadbackConfig::for_windows(1, 7, &[first]).unwrap();
        assert_eq!(selection.selection_ticket(), 7);
        assert!(selection.accepts_main_entity(first));
        assert!(!selection.accepts_main_entity(second));
        assert!(GpuBladeReadbackConfig::default().accepts_main_entity(second));
        assert!(GpuBladeReadbackConfig::for_windows(1, 0, &[first]).is_none());
        assert!(GpuBladeReadbackConfig::for_windows(1, 1, &[]).is_none());
        assert!(GpuBladeReadbackConfig::for_windows(1, 1, &[first, first]).is_none());
        assert!(GpuBladeReadbackConfig::for_windows(1, 1, &[first; GPU_BLADE_READBACK_WINDOW_CAPACITY + 1]).is_none());
        world.despawn(first);
        let replacement = world.spawn_empty().id();
        assert!(!selection.accepts_main_entity(replacement), "同槽位重建不能继承旧目标票据");
    }
}
