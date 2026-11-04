//! 分块候选取消后的唯一退役槽，不在主帧析构整片尚未登记的网格。
use bevy::{prelude::*, tasks::Task};
use std::collections::VecDeque;

#[derive(Resource, Default)]
pub(super) struct UploadGarbage {
    pub(super) pending: Vec<VecDeque<Mesh>>,
    pub(super) task: Option<Task<()>>,
}
