//! 作者文件与玩家存档分开建模。声明中的默认值决定变量类型，枚举在字符串上限定取值。

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{Error, MapAccess, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fmt,
    marker::PhantomData,
};

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
    #[serde(default, deserialize_with = "unique_ids")]
    pub variables: BTreeMap<String, Definition>,
    #[serde(default, deserialize_with = "unique_ids")]
    pub locations: BTreeMap<String, Location>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default, deserialize_with = "unique_ids")]
    pub scenes: BTreeMap<String, SceneRules>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SceneRules {
    #[serde(default, deserialize_with = "unique_ids")]
    pub variables: BTreeMap<String, Definition>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

// 只限制作者声明。读档仍按原快照合同恢复，不能用重复声明让运行时默默取最后一个值。
fn unique_ids<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, T>, D::Error> {
    struct Unique<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Unique<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("标识互不重复的 JSON 对象")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut entries: M) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = entries.next_entry::<String, T>()? {
                match result.entry(key) {
                    Entry::Vacant(entry) => {
                        entry.insert(value);
                    }
                    Entry::Occupied(entry) => {
                        return Err(M::Error::custom(format!("重复的标识「{}」", entry.key())));
                    }
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Unique(PhantomData))
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
    // 作者标签只用于编辑器分类，不参与运行时索引。
    #[serde(rename = "tag")]
    pub _tag: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rule {
    pub id: String,
    #[serde(rename = "tag")]
    pub _tag: Option<String>,
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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum PendingRequest {
    Target(String),
    InInstance { target: String, instance: String },
}

#[derive(Clone, Default, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ProjectVariablesSnapshot {
    #[serde(flatten)]
    pub(crate) project: ScopeSnapshot,
    pub(crate) requests: BTreeMap<String, Vec<PendingRequest>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) scenes: BTreeMap<String, ScopeSnapshot>,
}

#[derive(Clone, Default, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct ScopeSnapshot {
    pub(crate) values: BTreeMap<String, Value>,
    pub(crate) fired: BTreeSet<String>,
}
