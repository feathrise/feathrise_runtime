# 工程变量与触发规则

工程主资产根的 `project_rules.json` 定义跨场景使用的变量、地点和规则。`run` 和 `prepare_project` 在启动工程时加载并校验它。已有工程可以省略文件。配套编辑器可从「交互 → 工程变量与触发规则」创建和编辑这份配置。保存配置后，重新启动工程进程即可采用。

下例记录谁收到了古剑，并在玩家横穿镇门时开放新的对白条件。场景和任务目标均使用工程自己的稳定 ID。

```json
{
  "version": 1,
  "variables": {
    "sword_recipient": { "default": "none", "choices": ["none", "smith", "guard"] },
    "visited_gate": { "default": false }
  },
  "locations": {
    "east_gate": { "scene": "town", "min": [10, 0, 15], "max": [14, 4, 20] }
  },
  "rules": [
    {
      "id": "remember_gate",
      "when": { "event": "location.entered", "target": "east_gate" },
      "actions": [{ "set": "visited_gate", "value": true }]
    },
    {
      "id": "smith_branch",
      "when": { "changed": "sword_recipient", "from": "none", "to": "smith" },
      "if": [{ "variable": "visited_gate", "equals": true }],
      "once": true,
      "actions": [{ "request": "quest.start", "target": "reforge" }]
    }
  ]
}
```

- `default` 决定类型，支持布尔、有限的 32 位浮点数和文本。`choices` 为文本指定固定可选值。
- `when.changed` 监听值的实际变化，可以独立省略 `from`、`to`。`when.event` 监听玩法成功后发布的事实，省略 `target` 时匹配同名事件的任何目标。
- `if` 中的条件全部满足才执行动作。同一事实命中的规则按同一组值判断条件，再按作者顺序执行。规则写入其他变量会继续产生变化。同一次外部输入中，每条规则最多执行一次，循环因此能结束。`once: true` 将该规则的执行记录保留到本局存档。
- 地点是所选场景世界坐标中的三维盒。SDK 跟踪标有 `SceneRuntimeObserver` 的唯一玩家，跨过边界时发布 `location.entered` 或 `location.exited`。停在区域内不会重复发布。读档、换图或玩家重新生成后，以首次得到的位置建立基线。
- `set` 写工程变量。`request` 将动作和目标交给宿主游戏。动作被领取前保存在进度快照中。具体动作的成功条件由消费它的游戏系统校验。

## 在游戏代码中使用

访问 `Res<feathrise_runtime::project_rules::ProjectVariables>`。例如，在确认交付成功的业务入口调用 `vars.set("sword_recipient", Value::Text("smith".into()))?`。在任务结算完成后可调用 `vars.event("quest.completed", quest_id)`。任务插件准备好处理请求时，通过 `vars.take_requests("quest.start")` 取得目标列表。

涉及扣物的交付可用 `set_if`，它先校验变量、预期值和新值，再调用游戏的扣物操作。扣物成功才提交变量变化。涉及多个变量的同一选择可通过 `set_batch` 一次提交。普通查询使用 `get`，调试枚举使用 `values`。

游戏保存 `snapshot()` 的返回值，读档时将反序列化的快照交给 `restore`，新游戏传入默认快照。读档直接恢复变量、一次性规则记录和待处理动作。已有脚本句柄随之读到本次载入的进度。调试时开启 `feathrise_project_rules=debug` 日志可查看变化前后的值、规则 ID、未满足的条件及动作目标。
