//! 每个事实只访问当前生效的订阅，同一事实先判定所有条件再执行动作。

use crate::{
    ProjectVariables,
    definitions::{EffectiveScope, IndexedRule, RuleScope},
    types::{Action, PendingRequest, ProjectVariablesSnapshot, Trigger, Value},
};
use std::collections::{HashSet, VecDeque};

pub(crate) enum Fact {
    Changed { key: String, old: Value, new: Value },
    Event { name: String, target: String },
}

impl RuleScope {
    pub fn subscribes(&self, event: &str, target: &str) -> bool {
        self.events
            .get(event)
            .is_some_and(|index| !index.any_target.is_empty() || index.targets.contains_key(target))
    }

    fn candidates(&self, fact: &Fact) -> impl Iterator<Item = &IndexedRule> {
        let (primary, additional): (&[usize], &[usize]) = match fact {
            Fact::Changed { key, .. } => (
                self.changes.get(key).map(Vec::as_slice).unwrap_or_default(),
                &[],
            ),
            Fact::Event { name, target } => self.events.get(name).map_or((&[], &[]), |index| {
                (
                    &index.any_target,
                    index
                        .targets
                        .get(target)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                )
            }),
        };
        primary.iter().chain(additional).map(|&i| &self.rules[i])
    }
}

impl ProjectVariables {
    pub(crate) fn react(
        &self,
        scope: EffectiveScope<'_>,
        progress: &mut ProjectVariablesSnapshot,
        mut facts: VecDeque<Fact>,
        instance: Option<&str>,
    ) {
        // 同一规则在一次外部输入中最多执行一次，阻止变量链成环。
        let mut fired = HashSet::new();
        let mut matches = Vec::new();
        let scene = scope.scene.map(|(_, scene)| scene);
        while let Some(fact) = facts.pop_front() {
            let project = scope.project.candidates(&fact).filter(|entry| {
                !scene.is_some_and(|local| local.rule_order.contains_key(&entry.rule.id))
            });
            let local = scope.scene.into_iter().flat_map(|(id, scene)| {
                scene.candidates(&fact).map(move |entry| (Some(id), entry))
            });
            for (scene_id, entry) in project.map(|entry| (None, entry)).chain(local) {
                let rule = &entry.rule;
                if fired.contains(rule.id.as_str())
                    || (rule.once
                        && progress
                            .scope(scene_id)
                            .is_some_and(|slot| slot.fired.contains(&rule.id)))
                {
                    continue;
                }
                if let (Trigger::Changed { from, to, .. }, Fact::Changed { old, new, .. }) =
                    (&rule.when, &fact)
                    && (from.as_ref().is_some_and(|v| v != old)
                        || to.as_ref().is_some_and(|v| v != new))
                {
                    continue;
                }
                if let Some(condition) = rule
                    .conditions
                    .iter()
                    .find(|cond| scope.value(progress, &cond.variable) != Some(&cond.equals))
                {
                    tracing::debug!(rule = rule.id, ?condition, "触发条件未满足");
                    continue;
                }
                // 场景枚举可以收窄工程候选。整条动作先校验，避免标记未写入却启动任务，或消耗 once。
                if let Some(error) = rule.actions.iter().find_map(|action| match action {
                    Action::Set { set, value } => scope.validate(set, value).err(),
                    Action::Request { .. } => None,
                }) {
                    tracing::warn!(rule = rule.id, scene = scope.scene.map(|(id, _)| id), %error, "规则动作校验失败，保留触发资格");
                    continue;
                }
                fired.insert(rule.id.as_str());
                if rule.once {
                    progress.scope_mut(scene_id).fired.insert(rule.id.clone());
                }
                matches.push(entry);
            }
            // 合并的是命中结果，工程规则用场景同 ID 规则的执行位替换。
            matches.sort_unstable_by_key(|entry| entry.order);
            for entry in matches.drain(..) {
                let rule = &entry.rule;
                tracing::debug!(rule = rule.id, "执行触发规则");
                for action in &rule.actions {
                    match action {
                        Action::Set { set, value } => {
                            self.write(scope, progress, set, value, &rule.id, &mut facts);
                        }
                        Action::Request { request, target } => {
                            tracing::debug!(
                                rule = rule.id,
                                request,
                                target_id = target,
                                "提交领域动作"
                            );
                            progress.requests.entry(request.clone()).or_default().push(
                                match instance {
                                    Some(path) => PendingRequest::InInstance {
                                        target: target.clone(),
                                        instance: path.into(),
                                    },
                                    None => PendingRequest::Target(target.clone()),
                                },
                            );
                            self.requests_revision.fetch_add(1, std::sync::atomic::Ordering::Release);
                        }
                    }
                }
            }
        }
    }
}
