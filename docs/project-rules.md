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
- `if` 中的条件全部满足才执行动作。同一事实命中的规则按同一组值判断条件，再按作者顺序执行。规则写入其他变量会继续产生变化。同一次外部输入中，每条规则最多执行一次，循环因此能结束。`once: true` 在全部赋值通过校验后提交变量、待办与执行记录，并保留到本局存档。领域动作暂时失败时重试其待办，已成功的赋值和其他动作不会重做。
- 地点是所选场景世界坐标中的三维盒。SDK 跟踪标有 `SceneRuntimeObserver` 的唯一玩家，跨过边界时发布 `location.entered` 或 `location.exited`。停在区域内不会重复发布。读档、换图或玩家重新生成后，以首次得到的位置建立基线。
- `set` 写当前生效的变量层。继承的工程规则写入场景枚举时，值需满足场景候选。规则会先校验整条的全部赋值，任一失败时不执行该规则的任何动作，也不消耗一次性资格。`request` 将动作和目标交给宿主游戏。待办随进度快照保存，具体动作的成功条件由消费它的游戏系统校验。

## 在游戏代码中使用

访问 `Res<feathrise_runtime::project_rules::ProjectVariables>`。例如，在确认交付成功的业务入口调用 `vars.set("sword_recipient", Value::Text("smith".into()))?`。在任务结算完成后可调用 `vars.event("quest.completed", quest_id)`。任务插件准备好处理请求时，通过 `vars.take_requests("quest.start")` 取得目标列表。

领域动作可能暂时失败时，使用 `handle_requests_with_instances("quest.start", |target, instance| { /* 成功后返回 true */ false })`。成功的请求才移出队列，返回 `false`、回调中断或处理中读档都会保留尚未确认的待办。回调不持变量锁，可以发布后续事实。同动作不能重入领取，回调中新生成的请求等下轮处理。`requests_revision()` 提供待办变更版本，宿主应在请求、目录或业务资格变化后调度重试。原有 `take_requests` 接口继续提供直接领取语义，适用于宿主已经接管后续持久队列的情况。

默认 SDK 导演没有另设持久待播队列，因此直到演出收尾成功才确认原规则待办；缺资产、条件未满、呈现或结算失败时保留同一请求，目录、变量或领域事实变化后再试。RPG 导演在成功接入自己的槽位队列时确认，后续失败由该队列保留。`session_revision()` 返回读档和场景范围代次，跨帧消费者须在换代后撤回旧播放，不能确认新一代同名待办。默认 SDK 的已看集合仅属于当前运行会话，读档会清空；完整已看、在播和终态存档由宿主导演提供。

涉及扣物的交付可用 `set_if`，它先校验变量、预期值和新值，再调用游戏的扣物操作。扣物成功才提交变量变化。涉及多个变量的同一选择可通过 `set_batch` 一次提交。普通查询使用 `get`，调试枚举使用 `values`。

`SceneRuntimePlugin` 随正式场景切换更新当前范围。独立使用规则库的宿主可调用 `set_active_scene` 提供活动场景 ID，离开后传入 `None`。脚本可使用同一变量名访问当前生效的值。

游戏保存 `snapshot()` 的返回值，读档时将反序列化的快照交给 `restore`，新游戏传入默认快照。读档直接恢复工程与各场景的变量、一次性规则记录和待处理动作。旧档继续按工程层恢复，未写过的场景从声明值开始。已有脚本句柄随之读到本次载入的进度。调试时开启 `feathrise_project_rules=debug` 日志可查看变化前后的值、规则 ID、未满足的条件及动作目标。
