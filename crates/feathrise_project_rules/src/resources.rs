//! 每个游戏会话只有一份变量。Yarn、游戏系统和存档持有同一资源句柄，脚本回读能立即看到结果。

use crate::{RULES_FILE, definitions::RuleBook, types::*};
use bevy_ecs::resource::Resource;
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs, io,
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default, Clone, Debug, Resource)]
pub struct ProjectVariables {
    pub(crate) book: Arc<RuleBook>,
    state: Arc<Mutex<ProjectVariablesSnapshot>>,
    pub(crate) generation: Arc<AtomicU64>,
    values_revision: Arc<AtomicU64>,
}

enum Fact {
    Changed { key: String, old: Value, new: Value },
    Event { name: String, target: String },
}

impl ProjectVariables {
    pub fn from_json(text: &str) -> Result<Self, String> {
        Self::from_slice(text.as_bytes())
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, String> {
        // Windows 文本工具可保留 UTF-8 BOM，散文件、PAK 和编辑器使用同一正文语义。
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let doc = serde_json::from_slice(bytes).map_err(|e| format!("{RULES_FILE}：{e}"))?;
        Ok(Self {
            book: Arc::new(RuleBook::compile(doc)?),
            ..Self::default()
        })
    }

    /// 工程启动准备阶段读取。旧工程可省略配置，有文件但内容错误时向启动端返回具体原因。
    pub fn load(asset_root: &Path) -> Result<Self, String> {
        let path = asset_root.join(RULES_FILE);
        match fs::read(&path) {
            Ok(bytes) => Self::from_slice(&bytes).map_err(|e| format!("{}：{e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("无法读取 {}：{e}", path.display())),
        }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.value(&self.state(), key).cloned()
    }

    /// 玩法投影可在稳定帧无锁跳过变量查询。成功变值或替换槽位会更新版本，纯事件和取待办保持原值。
    pub fn values_revision(&self) -> u64 {
        self.values_revision.load(Ordering::Acquire)
    }

    /// 返回声明的默认值及本局覆盖，供脚本调试枚举；常规分支直接按 key 查询。
    pub fn values(&self) -> BTreeMap<String, Value> {
        let state = self.state();
        self.book
            .variables
            .iter()
            .map(|(key, def)| {
                (
                    key.clone(),
                    state.values.get(key).unwrap_or(&def.default).clone(),
                )
            })
            .collect()
    }

    pub fn validate_value(&self, key: &str, value: &Value) -> Result<(), String> {
        self.book.validate_value(key, value)
    }

    pub fn set(&self, key: &str, value: Value) -> Result<bool, String> {
        self.validate_value(key, &value)?;
        let mut state = self.state();
        let mut changes = VecDeque::new();
        let changed = self.write(&mut state, key, &value, "set", &mut changes);
        self.react(&mut state, changes, None);
        Ok(changed)
    }

    /// 多个变量属于同一次选择时先全部校验和写入，再让规则读取整组新值。
    pub fn set_batch(&self, values: BTreeMap<String, Value>) -> Result<(), String> {
        self.write_batch(values, None)
    }

    /// 子场景演出在收尾时引出的下一段剧情仍作用于这一次的场景实例。
    pub fn set_batch_in_instance(&self, values: BTreeMap<String, Value>, instance: &str) -> Result<(), String> {
        self.write_batch(values, Some(instance))
    }

    fn write_batch(&self, values: BTreeMap<String, Value>, instance: Option<&str>) -> Result<(), String> {
        for (key, value) in &values {
            self.validate_value(key, value)?;
        }
        let mut state = self.state();
        let mut changes = VecDeque::new();
        for (key, value) in values {
            self.write(&mut state, &key, &value, "set_batch", &mut changes);
        }
        self.react(&mut state, changes, instance);
        Ok(())
    }

    /// 在去向仍是预期值时执行一次短小的领域提交（例如扣下已核对的任务物品）。
    /// 回调只操作调用方已借用的数据，不可再调用同一会话；失败或相同值均不会扣物或触发规则。
    pub fn set_if(
        &self,
        key: &str,
        expected: &Value,
        value: Value,
        commit: impl FnOnce() -> bool,
    ) -> Result<bool, String> {
        self.validate_value(key, expected)?;
        self.validate_value(key, &value)?;
        let mut state = self.state();
        if expected == &value || self.value(&state, key) != Some(expected) || !commit() {
            return Ok(false);
        }
        let mut changes = VecDeque::new();
        self.write(&mut state, key, &value, "set_if", &mut changes);
        self.react(&mut state, changes, None);
        Ok(true)
    }

    /// 调用方仅在领域事务成功后发布事实。未订阅该类事件的工程直接返回。
    pub fn event(&self, event: &str, target: &str) {
        self.publish(event, target, None);
    }

    /// 同一子图可在多个位置实例化。沿本次事实的整个规则链保留产生它的稳定实例路径。
    pub fn event_in_instance(&self, event: &str, target: &str, instance: &str) {
        self.publish(event, target, Some(instance));
    }

    fn publish(&self, event: &str, target: &str, instance: Option<&str>) {
        let Some(index) = self.book.events.get(event) else {
            return;
        };
        if index.any_target.is_empty() && !index.targets.contains_key(target) {
            return;
        }
        tracing::debug!(event, target_id = target, "收到游戏事件");
        let fact = Fact::Event {
            name: event.into(),
            target: target.into(),
        };
        self.react(&mut self.state(), VecDeque::from([fact]), instance);
    }

    /// 领域按动作名领取自己的有序请求，并在自己的成功边界发布后续事实。
    pub fn take_requests(&self, action: &str) -> Vec<String> {
        self.take_requests_with_instances(action).into_iter().map(|(target, _)| target).collect()
    }

    pub fn take_requests_with_instances(&self, action: &str) -> Vec<(String,Option<String>)> {
        self.state().requests.remove(action).unwrap_or_default().into_iter().map(|r| match r {
            PendingRequest::Target(target)=>(target,None),
            PendingRequest::InInstance{target,instance}=>(target,Some(instance)),
        }).collect()
    }

    pub fn snapshot(&self) -> ProjectVariablesSnapshot {
        self.state().clone()
    }

    /// 用同一把锁替换整局数据。旧档沿用当前声明的默认值，删除或改型的变量不污染其他变量。
    /// 这里不走 set，恢复已发生的选择不会重发事件；脚本已经持有的句柄也立即指向新进度。
    pub fn restore(&self, mut snapshot: ProjectVariablesSnapshot) {
        snapshot
            .values
            .retain(|key, value| match self.validate_value(key, value) {
                Ok(()) => self.book.variables[key].default != *value,
                Err(e) => {
                    tracing::warn!("{e}，已忽略该存档变量");
                    false
                }
            });
        snapshot.fired.retain(|id| self.book.once.contains(id));
        let mut state = self.state();
        *state = snapshot;
        self.generation.fetch_add(1, Ordering::Release);
        self.values_revision.fetch_add(1, Ordering::Release);
    }

    fn state(&self) -> MutexGuard<'_, ProjectVariablesSnapshot> {
        self.state.lock().expect("工程变量会话未中断")
    }

    fn value<'a>(&'a self, state: &'a ProjectVariablesSnapshot, key: &str) -> Option<&'a Value> {
        state
            .values
            .get(key)
            .or_else(|| self.book.variables.get(key).map(|def| &def.default))
    }

    fn write(
        &self,
        state: &mut ProjectVariablesSnapshot,
        key: &str,
        value: &Value,
        source: &str,
        changes: &mut VecDeque<Fact>,
    ) -> bool {
        let default = &self.book.variables[key].default;
        let old = state.values.get(key).unwrap_or(default);
        if old == value {
            return false;
        }
        tracing::debug!(source, variable = key, ?old, new = ?value, "工程变量变化");
        if self.book.changes.contains_key(key) {
            changes.push_back(Fact::Changed {
                key: key.into(),
                old: old.clone(),
                new: value.clone(),
            });
        }
        if value == default {
            state.values.remove(key);
        } else {
            state.values.insert(key.into(), value.clone());
        }
        self.values_revision.fetch_add(1, Ordering::Release);
        true
    }

    fn react(&self, state: &mut ProjectVariablesSnapshot, mut facts: VecDeque<Fact>, instance: Option<&str>) {
        // 一次外部输入中，同一条规则最多执行一次，消除 A→B→A 循环。下一次独立输入可再次触发。
        // 同一事实的所有条件先按同一状态判定，再按文件顺序执行动作，不递归调用玩法系统。
        let mut fired = HashSet::new();
        while let Some(fact) = facts.pop_front() {
            let indices = match &fact {
                Fact::Changed { key, .. } => &self.book.changes[key],
                Fact::Event { name, target } => {
                    let index = &self.book.events[name];
                    let mut matches = index.any_target.clone();
                    if let Some(exact) = index.targets.get(target) {
                        matches.extend(exact);
                        matches.sort_unstable();
                    }
                    self.run_matches(state, &fact, &matches, &mut fired, &mut facts, instance);
                    continue;
                }
            };
            self.run_matches(state, &fact, indices, &mut fired, &mut facts, instance);
        }
    }

    fn run_matches(
        &self,
        state: &mut ProjectVariablesSnapshot,
        fact: &Fact,
        indices: &[usize],
        fired: &mut HashSet<usize>,
        facts: &mut VecDeque<Fact>,
        instance: Option<&str>,
    ) {
        let mut matches = Vec::new();
        for &index in indices {
            let rule = &self.book.rules[index];
            if fired.contains(&index) || (rule.once && state.fired.contains(&rule.id)) {
                continue;
            }
            if let (Trigger::Changed { from, to, .. }, Fact::Changed { old, new, .. }) =
                (&rule.when, fact)
                && (from.as_ref().is_some_and(|v| v != old)
                    || to.as_ref().is_some_and(|v| v != new))
            {
                continue;
            }
            if let Some(condition) = rule
                .conditions
                .iter()
                .find(|cond| self.value(state, &cond.variable) != Some(&cond.equals))
            {
                tracing::debug!(rule = rule.id, ?condition, "触发条件未满足");
                continue;
            }
            fired.insert(index);
            if rule.once {
                state.fired.insert(rule.id.clone());
            }
            matches.push(rule);
        }
        for rule in matches {
            tracing::debug!(rule = rule.id, "执行触发规则");
            for action in &rule.actions {
                match action {
                    Action::Set { set, value } => {
                        self.write(state, set, value, &rule.id, facts);
                    }
                    Action::Request { request, target } => {
                        tracing::debug!(
                            rule = rule.id,
                            request,
                            target_id = target,
                            "提交领域动作"
                        );
                        state
                            .requests
                            .entry(request.clone())
                            .or_default()
                            .push(match instance {
                                Some(path) => PendingRequest::InInstance { target:target.clone(), instance:path.into() },
                                None => PendingRequest::Target(target.clone()),
                            });
                    }
                }
            }
        }
    }
}
