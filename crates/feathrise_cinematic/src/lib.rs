//! 实时演出、作者拖帧和视频导出共用的时间轴。采样只产生指定时刻的姿态，不执行玩法命令。
mod catalog;
pub mod movie;
mod sample;
mod types;
mod validate;

pub use catalog::{CinematicCatalog, is_cinematic_path};
pub use movie::{GameDirectorProvided, MovieDialogue, MovieYarnProvider};
pub use sample::{CameraPose, ClipSample, Sample, TargetSample};
pub use types::*;

use bevy_ecs::prelude::Resource;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameMode {
    Game,
    Preview,
    Export,
}

#[derive(Clone, Debug)]
pub struct FrameData {
    pub document: Arc<Cinematic>,
    pub time: f32,
    pub ticket: u64,
    pub instance: Option<String>,
    pub mode: FrameMode,
}

impl FrameData {
    pub fn sample(&self) -> Sample<'_> {
        self.document.sample(self.time)
    }
}

/// 一次只有一个导演写入活动场次；捕获端等相同票据和时刻真正呈现后才推进。
#[derive(Resource, Default)]
pub struct CinematicFrame {
    pub active: Option<FrameData>,
}

/// 换图、重载角色、切换槽位后，只重建成功结算的世界姿态。
#[derive(Resource, Default)]
pub struct CinematicWorldState {
    pub completed: Vec<FrameData>,
}

impl CinematicWorldState {
    /// 同一实例重播只保留最新终态；把它放到最后，重建时仍按实际结算顺序覆盖相同目标。
    pub fn record_completion(&mut self, frame: FrameData) {
        self.completed.retain(|earlier| {
            earlier.document.id != frame.document.id || earlier.instance != frame.instance
        });
        self.completed.push(frame);
    }
}

#[cfg(test)]
mod tests;
