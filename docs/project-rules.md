# 工程变量与触发规则

工程主资产根的 `project_rules.json` 定义工程级变量、地点、触发规则以及可选的场景声明。`run` 和 `prepare_project` 在启动工程时加载并校验它。已有工程可以省略文件。配套编辑器在「交互」目录提供「工程变量」「触发规则」和「地点区域」三个入口，分别编辑同一份配置。保存配置后，重新启动工程进程即可采用。

下例记录谁收到了古剑，并在玩家横穿镇门时开放新的对白条件。场景和任务目标均使用工程自己的稳定 ID。

```json
{
  "version": 1,
  "variables": {
    "sword_recipient": { "default": "none", "choices": ["none", "smith", "guard"], "tag": "交付" },
    "visited_gate": { "default": false }
  },
  "locations": {
    "east_gate": { "scene": "MAP-town", "min": [10, 0, 15], "max": [14, 4, 20] }
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
  ],
  "scenes": {
    "MAP-town": {
      "variables": {
        "visited_gate": { "default": false, "tag": "镇门" }
      },
      "rules": [
        {
          "id": "remember_gate",
          "tag": "镇门",
          "when": { "event": "location.entered", "target": "east_gate" },
          "actions": [{ "set": "visited_gate", "value": true }]
        }
      ]
    }
  }
}
```

- `default` 决定初始值和类型，支持布尔、有限的 32 位浮点数和文本。`choices` 为枚举指定固定可选值。变量和规则可以分别填写一个字符串 `tag`，供编辑器分类和查询。
- `scenes` 以场景 ID 为键。活动场景同 ID 变量覆盖工程定义，并使用自己的初始值和存档值。工程未定义的变量也可以在场景层声明。同 ID 覆盖须保持布尔、数值或文本的底层类型，枚举候选和标签可单独设置。离开场景后回到当前场景或工程的有效值，切换本身不触发变量变化。
- 同 ID 场景规则整体覆盖工程规则，替换其原执行位置。场景专用规则随后按声明顺序执行。`once` 记录独立归属工程或声明场景。标签仅供分类，同 ID 的覆盖关系与标签无关。
- `when.changed` 监听值的实际变化，可以独立省略 `from`、`to`。`when.event` 监听玩法成功后发布的事实，省略 `target` 时匹配同名事件的任何目标。
- `if` 中的条件全部满足才执行动作。同一事实命中的规则按同一组值判断条件，再按作者顺序执行。规则写入其他变量会继续产生变化。同一次外部输入中，每条规则最多执行一次，循环因此能结束。`once: true` 将该规则的执行记录保留到本局存档。
- 地点是所选场景世界坐标中的三维盒。SDK 跟踪标有 `SceneRuntimeObserver` 的唯一玩家，跨过边界时发布 `location.entered` 或 `location.exited`。停在区域内不会重复发布。读档、换图或玩家重新生成后，以首次得到的位置建立基线。
- `set` 写当前生效的变量层。继承的工程规则写入场景枚举时，值需满足场景候选，此动作失败时记录日志，同条规则的其他动作继续执行。`request` 将动作和目标交给宿主游戏。动作被领取前保存在进度快照中。具体动作的成功条件由消费它的游戏系统校验。

## 在游戏代码中使用

访问 `Res<feathrise_runtime::project_rules::ProjectVariables>`。例如，在确认交付成功的业务入口调用 `vars.set("sword_recipient", Value::Text("smith".into()))?`。在任务结算完成后可调用 `vars.event("quest.completed", quest_id)`。任务插件准备好处理请求时，通过 `vars.take_requests("quest.start")` 取得目标列表。

涉及扣物的交付可用 `set_if`，它先校验变量、预期值和新值，再调用游戏的扣物操作。扣物成功才提交变量变化。涉及多个变量的同一选择可通过 `set_batch` 一次提交。普通查询使用 `get`，调试枚举使用 `values`。

`SceneRuntimePlugin` 随正式场景切换更新当前范围。独立使用规则库的宿主可调用 `set_active_scene` 提供活动场景 ID，离开后传入 `None`。脚本可使用同一变量名访问当前生效的值。

游戏保存 `snapshot()` 的返回值，读档时将反序列化的快照交给 `restore`，新游戏传入默认快照。读档直接恢复工程与各场景的变量、一次性规则记录和待处理动作。旧档继续按工程层恢复，未写过的场景从声明值开始。已有脚本句柄随之读到本次载入的进度。调试时开启 `feathrise_project_rules=debug` 日志可查看变化前后的值、规则 ID、未满足的条件及动作目标。
