//! 作者文件与玩家存档分开建模。声明中的默认值决定变量类型，枚举在字符串上限定取值。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Number(f32),
    Text(String),
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    pub version: u32,
    #[serde(default)]
    pub variables: BTreeMap<String, Definition>,
    #[serde(default)]
    pub locations: BTreeMap<String, Location>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Location {
    pub scene: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Definition {
    pub default: Value,
    pub choices: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rule {
    pub id: String,
    pub when: Trigger,
    #[serde(default, rename = "if")]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub once: bool,
    pub actions: Vec<Action>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum Trigger {
    Changed {
        changed: String,
        from: Option<Value>,
        to: Option<Value>,
    },
    Event {
        event: String,
        target: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Condition {
    pub variable: String,
    pub equals: Value,
}

#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum Action {
    Set { set: String, value: Value },
    Request { request: String, target: String },
}

/// 快照只含玩家进度。保存请求已生成但领域还未领取时，一起写档以免丢掉这次联动。
#[derive(Clone, Default, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ProjectVariablesSnapshot {
    pub(crate) values: BTreeMap<String, Value>,
    pub(crate) fired: BTreeSet<String>,
    pub(crate) requests: BTreeMap<String, Vec<String>>,
}
