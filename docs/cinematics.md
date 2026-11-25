# 演出资产与工程接入

将版本 1 的 `*.cinematic.json` 放在主资产根里的任意普通目录。SDK 在启动时后台建立 ID 目录，重名 ID 会给出诊断。可以从示例工程的[工作室镜头](../examples/minimal_project/assets/cinematics/studio.cinematic.json)开始。键的时间单位为秒，姿态由平移、四元数旋转和缩放组成，摄影视角使用垂直角度制。

工程规则可以发出 `{"request":"cinematic.play","target":"minimal.studio"}`。自定义代码可以通过 `ProjectVariables::event_in_instance` 把事实和发生位置交给同一规则链；多个子图实例可复用同一演出。已有世界由工程提供主相机标记 `PrimarySceneView`。相机演出也支持工程启动时自行创建的场景。

动作轨可以选择当前模型的具名动作。角色还可提供自己的动画适配器，复用已登记且能重定向到当前骨骼的外部动作。实例内摆放异步创建时，呈现器等待该实例正式发布，随后按稳定摆放 ID 绑定。演出收尾修改工程变量时，`set_batch_in_instance` 会把来源延续给变量改变引出的下一段剧情。 成功收尾还会发布 `cinematic.finished`，规则可按演出 ID 衔接下一段。

SDK 默认导演负责读取请求、显示字幕和按时结算工程变量。自有任务、背包、存档与 Yarn 的工程可提供 `GameDirectorProvided`，自行负责等待和结算，把 `FrameData` 交给 `CinematicFrame`。角色和物件均按稳定绑定执行，具名动作和相机呈现结束后可以从 `CinematicPresentation` 读取与票据、时间一致的回执。

编辑器导出时启动工程本身并设置 `FEATHRISE_CINEMATIC_EXPORT_JOB`。SDK 校验工程 ID、资产相对位置和已保存文件摘要，按影片帧号等候实际呈现并拍摄。提供 Yarn 的工程还需登记 `MovieYarnProvider`，使用私有变量副本解析 `MovieDialogue` 中的选项，把本次分支的对白交回摄影进程。影片进程只写任务目录。自定义工程插件应在此模式关闭自动存档和面向玩家的持久化。
