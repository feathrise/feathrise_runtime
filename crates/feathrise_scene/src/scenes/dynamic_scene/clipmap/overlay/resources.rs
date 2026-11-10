//! 独立地表反馈通道与植被输入隔离；两个作者发布者不清空对方的图片。

use super::types::{ClipmapSurfaceCoverage, ClipmapSurfaceOverlayIdentity};
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct DynamicClipmapSurfaceOverlay {
    pub current: Option<ClipmapSurfaceOverlayIdentity>,
    pub trail: Option<ClipmapSurfaceCoverage>,
    pub selection: Option<ClipmapSurfaceCoverage>,
}

impl DynamicClipmapSurfaceOverlay {
    /// 即时笔迹优先；两个通道只允许当前完整作者身份，旧图无需额外世界扫描。
    pub(super) fn coverage(&self) -> Option<&ClipmapSurfaceCoverage> {
        let current = self.current.as_ref()?;
        self.trail
            .as_ref()
            .filter(|coverage| &coverage.identity == current)
            .or_else(|| {
                self.selection
                    .as_ref()
                    .filter(|coverage| &coverage.identity == current)
            })
    }
}
