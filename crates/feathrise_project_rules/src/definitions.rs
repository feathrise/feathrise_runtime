//! 启动时校验完整作者文件，按变量名和事件目标建立订阅索引。索引顺序就是文件中的规则顺序。

use crate::types::*;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default, Debug)]
pub(crate) struct RuleBook {
    pub variables: BTreeMap<String, Definition>,
    pub rules: Vec<Rule>,
    pub once: HashSet<String>,
    pub changes: HashMap<String, Vec<usize>>,
    pub events: HashMap<String, EventIndex>,
    pub locations: crate::locations::LocationIndex,
}

#[derive(Default, Debug)]
pub(crate) struct EventIndex {
    pub any_target: Vec<usize>,
    pub targets: HashMap<String, Vec<usize>>,
}

impl Definition {
    fn accepts(&self, value: &Value) -> bool {
        match (&self.default, value) {
            (Value::Bool(_), Value::Bool(_)) => true,
            (Value::Number(_), Value::Number(n)) => n.is_finite(),
            (Value::Text(_), Value::Text(s)) => self.choices.as_ref().is_none_or(|v| v.contains(s)),
            _ => false,
        }
    }
}

impl RuleBook {
    pub fn compile(doc: Document) -> Result<Self, String> {
        if doc.version != 1 {
            return Err(format!("不支持的工程变量格式版本 {}", doc.version));
        }
        let mut book = Self {
            variables: doc.variables,
            locations: crate::locations::LocationIndex::build(doc.locations)?,
            ..Self::default()
        };
        for (key, def) in &book.variables {
            if key.is_empty() || !key.chars().all(|c| c == '_' || c.is_alphanumeric()) {
                return Err(format!("工程变量名 {key:?} 应使用字母、数字或下划线"));
            }
            if let Some(choices) = &def.choices {
                if !matches!(def.default, Value::Text(_))
                    || choices.iter().collect::<HashSet<_>>().len() != choices.len()
                {
                    return Err(format!(
                        "工程变量 {key} 的可选项应为不重复的字符串，默认值也应是字符串"
                    ));
                }
            }
            book.validate_value(key, &def.default)?;
        }
        let mut ids = HashSet::new();
        for (index, rule) in doc.rules.iter().enumerate() {
            if rule.id.trim().is_empty() || !ids.insert(&rule.id) {
                return Err(format!("规则 ID {:?} 为空或重复", rule.id));
            }
            let verify = |key: &str, value: &Value| {
                book.validate_value(key, value)
                    .map_err(|e| format!("规则 {}：{e}", rule.id))
            };
            for condition in &rule.conditions {
                verify(&condition.variable, &condition.equals)?;
            }
            if rule.actions.is_empty() {
                return Err(format!("规则 {} 缺少动作", rule.id));
            }
            for action in &rule.actions {
                match action {
                    Action::Set { set, value } => verify(set, value)?,
                    Action::Request { request, target }
                        if request.trim().is_empty() || target.trim().is_empty() =>
                    {
                        return Err(format!("规则 {} 的动作名和目标不能为空", rule.id));
                    }
                    Action::Request { .. } => {}
                }
            }
            match &rule.when {
                Trigger::Changed { changed, from, to } => {
                    if !book.variables.contains_key(changed) {
                        return Err(format!("规则 {} 监听了未声明的工程变量 {changed}", rule.id));
                    }
                    for value in from.iter().chain(to) {
                        verify(changed, value)?;
                    }
                    book.changes.entry(changed.clone()).or_default().push(index);
                }
                Trigger::Event { event, target } => {
                    if event.trim().is_empty()
                        || target.as_ref().is_some_and(|s| s.trim().is_empty())
                    {
                        return Err(format!("规则 {} 的事件名或目标为空", rule.id));
                    }
                    let subscriptions = book.events.entry(event.clone()).or_default();
                    match target {
                        Some(id) => subscriptions
                            .targets
                            .entry(id.clone())
                            .or_default()
                            .push(index),
                        None => subscriptions.any_target.push(index),
                    }
                }
            }
            if rule.once {
                book.once.insert(rule.id.clone());
            }
        }
        book.rules = doc.rules;
        Ok(book)
    }

    pub fn validate_value(&self, key: &str, value: &Value) -> Result<(), String> {
        match self.variables.get(key) {
            None => Err(format!("未声明的工程变量 {key}")),
            Some(def) if !def.accepts(value) => {
                Err(format!("工程变量 {key} 的类型或可选值不接受 {value:?}"))
            }
            Some(_) => Ok(()),
        }
    }
}
