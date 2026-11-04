//! 数字输入变化、提交、取消和灵敏度消息的唯一共享契约。

use bevy::prelude::*;
use crate::types::NumericValue;

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputChanged {
    pub entity: Entity,
    pub value: NumericValue,
}

/// 一次数字编辑结束的原因；业务层可区分有效提交、显式取消和非法缓冲。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericInputCommitOutcome {
    /// Enter、失焦或步进按钮提交了可解析数值。
    Accepted,
    /// Esc显式恢复编辑前数值。
    Canceled,
    /// 空字符串、单独负号等缓冲无法解析，控件已恢复编辑前数值。
    Rejected,
}

/// 一次提交来自字符编辑、步进按钮还是专业字段拖动；业务层可据此选择事务语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericInputCommitSource {
    Text,  // 中键单击专业字段（或左键单击普通字段）后的Enter、失焦、Esc或非法缓冲恢复。
    Step,  // 公共加减按钮形成的一次离散变化。
    Scrub, // 左键相对拖动松开形成的一次事务收口。
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputCommitted {
    pub entity: Entity,
    pub value: NumericValue,
    /// 业务系统不得把取消或非法输入误当成一次正式作者修改。
    pub outcome: NumericInputCommitOutcome,
    /// 提交来源决定业务层是否需要把拖动期间的Changed合并为一次撤销。
    pub source: NumericInputCommitSource,
    /// 最终值是否不同于本次聚焦前的控件值；未编辑失焦不会误触业务写入。
    pub changed: bool,
    /// 用户是否实际编辑过缓冲；显式重输相同显示值仍可由业务层视为作者意图。
    pub edited: bool,
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputSetValue {
    pub entity: Entity,
    pub value: NumericValue,
    pub cancel_edit: bool,
}

/// 右键灵敏度拖动途中发布受检倍率，只供即时状态回显，不应直接触发持久化。
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputScrubSensitivityChanged {
    pub entity: Entity, // 当前正在右拖的数字输入业务根。
    pub value: f32,     // 本帧受检倍率。
}

/// 右键灵敏度拖动结束后只发布一次，避免自动保存系统在拖动期间逐帧写设置文件。
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct NumericInputScrubSensitivityCommitted {
    pub entity: Entity, // 被调整的数字输入业务根。
    pub before: f32,    // 本次右拖开始前的灵敏度倍率。
    pub after: f32,     // 松开右键后采用并应持久化的灵敏度倍率。
}
