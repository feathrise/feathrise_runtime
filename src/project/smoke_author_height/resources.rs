//! 只允许一个实际分页源读取任务；过期任务自然结束前不会归还后台槽。

use super::types::Identity;
use bevy::{prelude::Resource, tasks::Task};

#[derive(Resource, Default)]
pub(super) struct SourceState {
    pub task: Option<(Identity, Task<Result<String, String>>)>,
    pub completed: Option<(Identity, Result<String, String>)>,
}
