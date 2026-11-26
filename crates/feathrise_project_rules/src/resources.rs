//! 游戏及脚本共享同一局变量，活动场景选择与变量事务使用同一把锁。

use crate::{
    RULES_FILE,
    definitions::{EffectiveScope, RuleBook, RuleScope, validate_value},
    dispatch::Fact,
    types::*,
};
use bevy_ecs::resource::Resource;
use std::{
    collections::{BTreeMap, VecDeque},
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
    state: Arc<Mutex<Session>>,
    pub(crate) generation: Arc<AtomicU64>,
    values_revision: Arc<AtomicU64>,
}

#[derive(Default, Debug)]
struct Session {
    active_scene: Option<String>,
    progress: ProjectVariablesSnapshot,
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
        let state = self.state();
        self.book
            .scope(state.active_scene.as_deref())
            .value(&state.progress, key)
            .cloned()
    }

    /// 玩法投影可在稳定帧无锁跳过查询。变值、切换有定义的场景和读档会更新版本。
    pub fn values_revision(&self) -> u64 {
        self.values_revision.load(Ordering::Acquire)
    }

    /// 切换活动场景的定义和存档层。出入场景只改变当前读写范围，不会生成变量变化事实。
    pub fn set_active_scene(&self, scene: Option<&str>) {
        let mut state = self.state();
        if state.active_scene.as_deref() == scene {
            return;
        }
        let changes_definitions = [state.active_scene.as_deref(), scene]
            .into_iter()
            .flatten()
            .any(|id| self.book.scenes.contains_key(id));
        state.active_scene = scene.map(str::to_owned);
        if changes_definitions {
            self.values_revision.fetch_add(1, Ordering::Release);
        }
    }

    pub fn active_scene(&self) -> Option<String> {
        self.state().active_scene.clone()
    }

    /// Yarn 预编译检查的是声明类型，调用时可能还没有进入任何场景。
    pub fn matches_declaration(&self, key: &str, value: &Value) -> bool {
        let matches = |def: &Definition| {
            std::mem::discriminant(&def.default) == std::mem::discriminant(value)
        };
        if let Some(def) = self.book.project.variables.get(key) {
            return matches(def);
        }
        let mut declarations = self
            .book
            .scenes
            .values()
            .filter_map(|scope| scope.variables.get(key));
        declarations.next().is_some_and(matches) && declarations.all(matches)
    }

    /// 返回声明的默认值及本局覆盖，供脚本调试枚举；常规分支直接按 key 查询。
    pub fn values(&self) -> BTreeMap<String, Value> {
        let state = self.state();
        let scope = self.book.scope(state.active_scene.as_deref());
        let mut values = scope
            .project
            .variables
            .iter()
            .map(|(key, def)| {
                (
                    key.clone(),
                    state
                        .progress
                        .project
                        .values
                        .get(key)
                        .unwrap_or(&def.default)
                        .clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        if let Some((id, scene)) = scope.scene {
            let progress = state.progress.scenes.get(id);
            values.extend(scene.variables.iter().map(|(key, def)| {
                let saved = progress.and_then(|slot| slot.values.get(key));
                (key.clone(), saved.unwrap_or(&def.default).clone())
            }));
        }
        values
    }

    pub fn validate_value(&self, key: &str, value: &Value) -> Result<(), String> {
        self.book
            .scope(self.state().active_scene.as_deref())
            .validate(key, value)
    }

    pub fn set(&self, key: &str, value: Value) -> Result<bool, String> {
        let mut state = self.state();
        let scope = self.book.scope(state.active_scene.as_deref());
        scope.validate(key, &value)?;
        let mut changes = VecDeque::new();
        let changed = self.write(scope, &mut state.progress, key, &value, "set", &mut changes);
        self.react(scope, &mut state.progress, changes, None);
        Ok(changed)
    }

    /// 多个变量属于同一次选择时先全部校验和写入，再让规则读取整组新值。
    pub fn set_batch(&self, values: BTreeMap<String, Value>) -> Result<(), String> {
        self.write_batch(values, None)
    }

    /// 子场景演出在收尾时引出的下一段剧情仍作用于这一次的场景实例。
    pub fn set_batch_in_instance(
        &self,
        values: BTreeMap<String, Value>,
        instance: &str,
    ) -> Result<(), String> {
        self.write_batch(values, Some(instance))
    }

    fn write_batch(
        &self,
        values: BTreeMap<String, Value>,
        instance: Option<&str>,
    ) -> Result<(), String> {
        let mut state = self.state();
        let scope = self.book.scope(state.active_scene.as_deref());
        for (key, value) in &values {
            scope.validate(key, value)?;
        }
        let mut changes = VecDeque::new();
        for (key, value) in values {
            self.write(
                scope,
                &mut state.progress,
                &key,
                &value,
                "set_batch",
                &mut changes,
            );
        }
        self.react(scope, &mut state.progress, changes, instance);
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
        let mut state = self.state();
        let scope = self.book.scope(state.active_scene.as_deref());
        scope.validate(key, expected)?;
        scope.validate(key, &value)?;
        if expected == &value || scope.value(&state.progress, key) != Some(expected) || !commit() {
            return Ok(false);
        }
        let mut changes = VecDeque::new();
        self.write(
            scope,
            &mut state.progress,
            key,
            &value,
            "set_if",
            &mut changes,
        );
        self.react(scope, &mut state.progress, changes, None);
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
        if self.book.scenes.is_empty() && !self.book.project.subscribes(event, target) {
            return;
        }
        let mut state = self.state();
        let scope = self.book.scope(state.active_scene.as_deref());
        if !scope.project.subscribes(event, target)
            && !scope
                .scene
                .is_some_and(|(_, scope)| scope.subscribes(event, target))
        {
            return;
        }
        tracing::debug!(event, target_id = target, "收到游戏事件");
        let fact = Fact::Event {
            name: event.into(),
            target: target.into(),
        };
        self.react(scope, &mut state.progress, VecDeque::from([fact]), instance);
    }

    /// 领域按动作名领取自己的有序请求，并在自己的成功边界发布后续事实。
    pub fn take_requests(&self, action: &str) -> Vec<String> {
        self.take_requests_with_instances(action)
            .into_iter()
            .map(|(target, _)| target)
            .collect()
    }

    pub fn take_requests_with_instances(&self, action: &str) -> Vec<(String, Option<String>)> {
        self.state()
            .progress
            .requests
            .remove(action)
            .unwrap_or_default()
            .into_iter()
            .map(|r| match r {
                PendingRequest::Target(target) => (target, None),
                PendingRequest::InInstance { target, instance } => (target, Some(instance)),
            })
            .collect()
    }

    pub fn snapshot(&self) -> ProjectVariablesSnapshot {
        let mut snapshot = self.state().progress.clone();
        snapshot.scenes.retain(|_, progress| !progress.is_empty());
        snapshot
    }

    /// 用同一把锁替换整局数据。旧档沿用当前声明的默认值，删除或改型的变量不污染其他变量。
    /// 这里不走 set，恢复已发生的选择不会重发事件；脚本已经持有的句柄也立即指向新进度。
    pub fn restore(&self, mut snapshot: ProjectVariablesSnapshot) {
        snapshot.project.sanitize(&self.book.project);
        snapshot.scenes.retain(|id, progress| {
            let Some(scope) = self.book.scenes.get(id) else {
                return false;
            };
            progress.sanitize(scope);
            !progress.is_empty()
        });
        let mut state = self.state();
        state.progress = snapshot;
        self.generation.fetch_add(1, Ordering::Release);
        self.values_revision.fetch_add(1, Ordering::Release);
    }

    fn state(&self) -> MutexGuard<'_, Session> {
        self.state.lock().expect("工程变量会话未中断")
    }
}

impl ScopeSnapshot {
    fn sanitize(&mut self, scope: &RuleScope) {
        self.values.retain(|key, value| {
            let def = scope.variables.get(key);
            match validate_value(def, key, value) {
                Ok(()) => def.is_some_and(|def| def.default != *value),
                Err(e) => {
                    tracing::warn!("{e}，已忽略该存档变量");
                    false
                }
            }
        });
        self.fired.retain(|id| scope.once.contains(id));
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty() && self.fired.is_empty()
    }
}

impl ProjectVariablesSnapshot {
    pub(crate) fn scope(&self, scene: Option<&str>) -> Option<&ScopeSnapshot> {
        match scene {
            Some(id) => self.scenes.get(id),
            None => Some(&self.project),
        }
    }

    pub(crate) fn scope_mut(&mut self, scene: Option<&str>) -> &mut ScopeSnapshot {
        match scene {
            Some(id) => self.scenes.entry(id.into()).or_default(),
            None => &mut self.project,
        }
    }
}

impl<'a> EffectiveScope<'a> {
    fn definition(self, key: &str) -> Option<(Option<&'a str>, &'a Definition)> {
        self.scene
            .and_then(|(id, scope)| scope.variables.get(key).map(|def| (Some(id), def)))
            .or_else(|| self.project.variables.get(key).map(|def| (None, def)))
    }

    pub fn validate(self, key: &str, value: &Value) -> Result<(), String> {
        validate_value(self.definition(key).map(|(_, def)| def), key, value)
    }

    pub fn value(self, progress: &'a ProjectVariablesSnapshot, key: &str) -> Option<&'a Value> {
        let (scene, def) = self.definition(key)?;
        // 场景定义命中后只看该场景的覆盖值或定义值，不能读到同名工程进度。
        Some(
            progress
                .scope(scene)
                .and_then(|slot| slot.values.get(key))
                .unwrap_or(&def.default),
        )
    }
}

impl ProjectVariables {
    pub(crate) fn write(
        &self,
        scope: EffectiveScope<'_>,
        progress: &mut ProjectVariablesSnapshot,
        key: &str,
        value: &Value,
        source: &str,
        changes: &mut VecDeque<Fact>,
    ) -> bool {
        let (scene, def) = scope.definition(key).expect("写入前必须校验变量");
        let old = progress
            .scope(scene)
            .and_then(|slot| slot.values.get(key))
            .unwrap_or(&def.default);
        if old == value {
            return false;
        }
        tracing::debug!(source, variable = key, ?old, new = ?value, "工程变量变化");
        if scope.project.changes.contains_key(key)
            || scope
                .scene
                .is_some_and(|(_, scope)| scope.changes.contains_key(key))
        {
            changes.push_back(Fact::Changed {
                key: key.into(),
                old: old.clone(),
                new: value.clone(),
            });
        }
        let slot = progress.scope_mut(scene);
        if value == &def.default {
            slot.values.remove(key);
        } else {
            slot.values.insert(key.into(), value.clone());
        }
        self.values_revision.fetch_add(1, Ordering::Release);
        true
    }
}
