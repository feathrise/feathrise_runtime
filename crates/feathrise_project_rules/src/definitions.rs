//! 启动时校验声明并按作用域建立订阅索引，场景层只存储本地条目。

use crate::types::*;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default, Debug)]
pub(crate) struct RuleBook {
    pub project: RuleScope,
    pub scenes: BTreeMap<String, RuleScope>,
    pub locations: crate::locations::LocationIndex,
}

#[derive(Default, Debug)]
pub(crate) struct RuleScope {
    pub variables: BTreeMap<String, Definition>,
    pub rules: Vec<IndexedRule>,
    pub rule_order: HashMap<String, usize>,
    pub once: HashSet<String>,
    pub changes: HashMap<String, Vec<usize>>,
    pub events: HashMap<String, EventIndex>,
}

#[derive(Debug)]
pub(crate) struct IndexedRule {
    pub order: usize,
    pub rule: Rule,
}

/// 借用已编译的工程层和当前场景层，不复制全部定义或订阅。
#[derive(Clone, Copy)]
pub(crate) struct EffectiveScope<'a> {
    pub project: &'a RuleScope,
    pub scene: Option<(&'a str, &'a RuleScope)>,
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
        let project = RuleScope::compile(doc.variables, doc.rules, None)?;
        let mut scenes = BTreeMap::new();
        for (id, scene) in doc.scenes {
            if id.trim().is_empty() {
                return Err("场景规则需要稳定场景 ID".into());
            }
            let scope = RuleScope::compile(scene.variables, scene.rules, Some(&project))
                .map_err(|e| format!("场景 {id}：{e}"))?;
            scenes.insert(id, scope);
        }
        Ok(Self {
            project,
            scenes,
            locations: crate::locations::LocationIndex::build(doc.locations)?,
        })
    }

    pub fn scope(&self, scene: Option<&str>) -> EffectiveScope<'_> {
        EffectiveScope {
            project: &self.project,
            scene: scene
                .and_then(|id| self.scenes.get_key_value(id))
                .map(|(id, scope)| (id.as_str(), scope)),
        }
    }
}

impl RuleScope {
    fn compile(
        variables: BTreeMap<String, Definition>,
        rules: Vec<Rule>,
        inherited: Option<&Self>,
    ) -> Result<Self, String> {
        for (key, def) in &variables {
            if key.is_empty() || !key.chars().all(|c| c == '_' || c.is_alphanumeric()) {
                return Err(format!("工程变量名 {key:?} 应使用字母、数字或下划线"));
            }
            if let Some(choices) = &def.choices
                && (!matches!(def.default, Value::Text(_))
                    || choices.iter().collect::<HashSet<_>>().len() != choices.len())
            {
                return Err(format!(
                    "工程变量 {key} 的可选项应为不重复的字符串，默认值也应是字符串"
                ));
            }
            if let Some(global) = inherited.and_then(|scope| scope.variables.get(key))
                && std::mem::discriminant(&global.default) != std::mem::discriminant(&def.default)
            {
                return Err(format!("变量 {key} 的场景覆盖应保留工程变量的类型"));
            }
            validate_value(Some(def), key, &def.default)?;
        }
        let mut scope = Self {
            variables,
            ..Self::default()
        };
        for (index, rule) in rules.into_iter().enumerate() {
            // 场景同 ID 规则替换工程原位置，场景专用规则随后按文件顺序执行。
            let order = inherited.map_or(index, |project| {
                project
                    .rule_order
                    .get(&rule.id)
                    .copied()
                    .unwrap_or(project.rules.len() + index)
            });
            if rule.id.trim().is_empty()
                || scope.rule_order.insert(rule.id.clone(), order).is_some()
            {
                return Err(format!("规则 ID {:?} 为空或重复", rule.id));
            }
            let definition = |key: &str| {
                scope
                    .variables
                    .get(key)
                    .or_else(|| inherited.and_then(|project| project.variables.get(key)))
            };
            let verify = |key: &str, value: &Value| {
                validate_value(definition(key), key, value)
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
                    if definition(changed).is_none() {
                        return Err(format!("规则 {} 监听了未声明的工程变量 {changed}", rule.id));
                    }
                    for value in from.iter().chain(to) {
                        verify(changed, value)?;
                    }
                    scope
                        .changes
                        .entry(changed.clone())
                        .or_default()
                        .push(index);
                }
                Trigger::Event { event, target } => {
                    if event.trim().is_empty()
                        || target.as_ref().is_some_and(|s| s.trim().is_empty())
                    {
                        return Err(format!("规则 {} 的事件名或目标为空", rule.id));
                    }
                    let subscriptions = scope.events.entry(event.clone()).or_default();
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
                scope.once.insert(rule.id.clone());
            }
            scope.rules.push(IndexedRule { order, rule });
        }
        Ok(scope)
    }
}

pub(crate) fn validate_value(
    def: Option<&Definition>,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    match def {
        None => Err(format!("未声明的工程变量 {key}")),
        Some(def) if !def.accepts(value) => {
            Err(format!("工程变量 {key} 的类型或可选值不接受 {value:?}"))
        }
        Some(_) => Ok(()),
    }
}
