// 通过真实 ECS 生命周期验证遮雨层登记、预览隔离、幂等与源移除，不需要 GPU 或窗口。
use super::*; // 与正式插件使用相同安装入口。

fn app() -> App {
    // 每个测试独立注册观察器。
    let mut app = App::new(); // 此测试不需要渲染插件或资源加载。
    install(&mut app); // 必须在测试实体生成前注册，与正式启动顺序相同。
    app // 调用方直接驱动 World 结构变更。
} // 不共享跨测试状态。

fn flush(app: &mut App) {
    // 让观察器命令及其幂等后续事件全部完成。
    app.world_mut().flush(); // 若存在递归插入错误，本操作无法正常返回。
} // 不依赖游戏帧或计时器。

#[test] // 普通无显式层的 GLB primitive 自动进入雨向捕获，材质移除后还原缺省层。
fn implicit_world_layer_is_registered_and_restored() {
    // 覆盖最常见建筑加载与材质替换路径。
    let mut app = app(); // 注册正式观察器。
    let entity = app
        .world_mut()
        .spawn(MeshMaterial3d::<StandardMaterial>::default())
        .id(); // 模拟未显式指定层的世界建筑 primitive。
    flush(&mut app); // 观察器应补齐世界层和专用层。
    let layers = app.world().get::<RenderLayers>(entity).unwrap(); // 建筑现在有自动安装的层组件。
    assert!(layers.iter().any(|layer| layer == 0)); // 原世界相机仍能看见建筑。
    assert!(layers.iter().any(|layer| layer == RAIN_OCCLUSION_LAYER)); // 雨向相机同时能捕获建筑。
    app.world_mut()
        .entity_mut(entity)
        .remove::<MeshMaterial3d<StandardMaterial>>(); // 模拟切换到不参与捕获的自定义材质。
    flush(&mut app); // 归还原层语义。
    assert!(app.world().get::<RenderLayers>(entity).is_none()); // 精确恢复最初不存在的 RenderLayers。
    assert!(app.world().get::<OwnedRainLayer>(entity).is_none()); // 不残留失效所有权。
} // 不需要销毁网格或材质资产。

#[test] // 预览层先后插入都不能被误当成世界建筑。
fn preview_layer_replacement_preserves_only_author_layers() {
    // 覆盖 glTF 先实例化后被预览系统重新分层。
    let mut app = app(); // 安装观察器。
    let entity = app
        .world_mut()
        .spawn(MeshMaterial3d::<StandardMaterial>::default())
        .id(); // 初始临时默认世界层。
    flush(&mut app); // 初始接管后再模拟异步预览层发布。
    app.world_mut()
        .entity_mut(entity)
        .insert(RenderLayers::from_layers(&[14, 18])); // 预览系统整体替换为专属层。
    flush(&mut app); // 立即撤销自动捕获资格。
    assert_eq!(
        app.world().get::<RenderLayers>(entity).unwrap(),
        &RenderLayers::from_layers(&[14, 18])
    ); // 不污染两个原预览层。
    assert!(app.world().get::<OwnedRainLayer>(entity).is_none()); // 不保留无效接管标志。
    let preview = app
        .world_mut()
        .spawn((
            // 同 bundle 创建时应直接读取完整预览状态。
            MeshMaterial3d::<StandardMaterial>::default(), // 预览也使用标准材质。
            RenderLayers::layer(14),                       // 仅资产浏览器可以看见。
        ))
        .id(); // 保留实体用于层断言。
    flush(&mut app); // 重复 bundle 事件必须保持预览隔离。
    assert_eq!(
        app.world().get::<RenderLayers>(preview).unwrap(),
        &RenderLayers::layer(14)
    ); // 不添加世界层或遮雨层。
} // 预览模型不能在世界坐标原点形成幽灵屋顶。

#[test] // 层重复发布、规划改名和改回普通名均须保持幂等。
fn replanning_and_repeated_insert_preserve_other_layers() {
    // 验证所有权撤销不会损失小场景等外部层。
    let mut app = app(); // 安装观察器。
    let entity = app
        .world_mut()
        .spawn((
            // 源实体已有多个明确作者层。
            MeshMaterial3d::<StandardMaterial>::default(), // 标准建筑材质。
            RenderLayers::from_layers(&[0, 5]),            // 正式世界与小场景都可见。
        ))
        .id(); // 记录待测试实体。
    flush(&mut app); // 首次自动接管。
    for _ in 0..4 {
        // 相同组件反复发布不应持续产生新命令链。
        let layers = app.world().get::<RenderLayers>(entity).unwrap().clone(); // 保留当前完整层集合。
        app.world_mut().entity_mut(entity).insert(layers); // 模拟重复同步同一层信息。
        flush(&mut app); // 幂等后续命令必须终止。
    } // 不创建附加实体或重复 Mesh。
    app.world_mut()
        .entity_mut(entity)
        .insert(Name::new("规划_类别_测试")); // 改成项目保留的规划辅助对象。
    flush(&mut app); // 名称事件撤销捕获资格。
    assert_eq!(
        app.world().get::<RenderLayers>(entity).unwrap(),
        &RenderLayers::from_layers(&[0, 5])
    ); // 归还世界及小场景层。
    app.world_mut().entity_mut(entity).remove::<Name>(); // 删除规划名恢复普通世界对象。
    flush(&mut app); // Name Remove 需读取移除后的最终状态。
    assert!(
        app.world()
            .get::<RenderLayers>(entity)
            .unwrap()
            .iter()
            .any(|layer| layer == RAIN_OCCLUSION_LAYER)
    ); // 合法源重新参与捕获。
    app.world_mut().entity_mut(entity).despawn(); // 删除源时观察器会产生迟到命令。
    flush(&mut app); // 删除后的命令应静默结束，不 panic。
    assert!(app.world().get_entity(entity).is_err()); // 不因清理事件复活实体。
} // 模拟加载、重命名与卸载的完整生命周期。

#[test] // 外部明确指定的专用层不属于自动接管所有权。
fn preexisting_capture_layer_is_not_removed() {
    // 防止自动系统破坏其他显式作者控制。
    let mut app = app(); // 安装观察器。
    let layers = RenderLayers::from_layers(&[0, RAIN_OCCLUSION_LAYER]); // 外部从创建起就明确拥有捕获层。
    let entity = app
        .world_mut()
        .spawn((
            // 原生手动参与捕获的世界实体。
            MeshMaterial3d::<StandardMaterial>::default(), // 标准材质源。
            layers.clone(),                                // 全部层来自调用方。
        ))
        .id(); // 后续验证不会抢占其层所有权。
    flush(&mut app); // 首次事件不应添加所有权。
    app.world_mut()
        .entity_mut(entity)
        .remove::<MeshMaterial3d<StandardMaterial>>(); // 删除源时仍应保留外部层。
    flush(&mut app); // 完成材质移除事件。
    assert_eq!(app.world().get::<RenderLayers>(entity).unwrap(), &layers); // 只负责清理自己安装的层。
} // 保留可扩展的显式登记入口。

#[test] // glTF规划名位于父节点时，普通primitive名称也必须被排除。
fn planning_ancestor_rename_updates_primitive_descendants() {
    // 覆盖用户F1显示规划节点后的实际GLTF结构。
    let mut app = app(); // 安装层和祖先观察器。
    let parent = app.world_mut().spawn(Name::new("规划_区域_院落")).id(); // 规划名由glTF节点持有。
    let child = app
        .world_mut()
        .spawn((
            // primitive子节点使用完全不同的普通名称。
            Name::new("Cube.Material"), // 不含规划前缀，不能只检查此实体。
            MeshMaterial3d::<StandardMaterial>::default(), // 标准材质本来具备自动捕获资格。
            ChildOf(parent),            // 正式glTF父子关系。
        ))
        .id(); // 记录primitive便于检查层。
    flush(&mut app); // 初次祖先登记在生成命令完成时立即判定。
    assert!(app.world().get::<OwnedRainLayer>(child).is_none()); // 不因规划节点可见而产生幽灵屋顶。
    app.world_mut()
        .entity_mut(parent)
        .insert(Name::new("正式屋顶")); // 父节点改为正式建筑。
    app.update(); // 定向祖先脏队列更新已有primitive。
    assert!(app.world().get::<OwnedRainLayer>(child).is_some()); // 名称变更恢复真实世界遮雨资格。
    app.world_mut()
        .entity_mut(parent)
        .insert(Name::new("规划_区域_院落")); // 再次变为规划节点。
    app.update(); // 不需要重新创建网格或扫描世界。
    assert!(app.world().get::<OwnedRainLayer>(child).is_none()); // 自动撤销之前的深度层。
} // 名称变更作用于整条祖先链。

#[test] // 重挂中间父节点也必须刷新其已经登记的primitive后代。
fn reparenting_intermediate_node_refreshes_indexed_primitives() {
    // 覆盖primitive自身ChildOf未变的重挂路径。
    let mut app = app(); // 安装正式观察器。
    let planning = app.world_mut().spawn(Name::new("规划_类别_辅助")).id(); // 禁止捕获的目标父级。
    let ordinary = app.world_mut().spawn(Name::new("建筑")).id(); // 合法建筑根。
    let middle = app.world_mut().spawn(ChildOf(ordinary)).id(); // glTF中间变换节点没有自身材质。
    let child = app
        .world_mut()
        .spawn((
            // 真实标准材质primitive位于两级之下。
            MeshMaterial3d::<StandardMaterial>::default(), // 初次登记有效。
            ChildOf(middle),                               // primitive父关系始终不变。
        ))
        .id(); // 保留叶节点身份。
    app.update(); // 完成初始登记和排队关系通知。
    assert!(app.world().get::<OwnedRainLayer>(child).is_some()); // 普通建筑可以遮雨。
    app.world_mut().entity_mut(middle).insert(ChildOf(planning)); // 仅重挂中间节点。
    app.update(); // 反向索引定向更新其叶节点。
    assert!(app.world().get::<OwnedRainLayer>(child).is_none()); // 新规划祖先排除叶网格。
    app.world_mut().entity_mut(middle).remove::<ChildOf>(); // 解除规划父级恢复普通独立节点。
    app.update(); // Removed关系事件同样使用最新祖先链。
    assert!(app.world().get::<OwnedRainLayer>(child).is_some()); // 移出规划层级后重新登记。
} // 不依赖Children递归扫描或整个场景重新生成。

#[test] // 一个大型父级改名不在单帧遍历所有子网格，也不能永久遗漏尾部。
fn ancestor_fanout_is_bounded_and_completes_without_truncation() {
    // 固定小夹具验证分帧边界，不代表目标硬件性能测量。
    let mut app = app(); // 安装正式队列处理系统。
    app.init_resource::<super::super::resources::CaptureState>(); // 验证批次完整性门与生产者共享。
    let parent = app.world_mut().spawn(Name::new("普通建筑")).id(); // 多primitive共同依赖一个祖先。
    let children: Vec<_> = (0..130)
        .map(|_| {
            app.world_mut()
                .spawn((
                    // 略多于两帧64预算以验证尾部不会被丢弃。
                    MeshMaterial3d::<StandardMaterial>::default(), // 每个源只有固定轻量组件。
                    ChildOf(parent),                               // 同一根的大扇出。
                ))
                .id()
        })
        .collect(); // 测试夹具允许同步构建有限实体集合。
    for _ in 0..5 {
        app.update();
    } // 清空创建阶段的定向生命周期通知。
    app.world_mut()
        .entity_mut(parent)
        .insert(Name::new("规划_类别_大根")); // 一次父名称变化只排一个祖先键。
    app.update(); // 只允许有限后代更新。
    let excluded = children
        .iter()
        .filter(|&&entity| app.world().get::<OwnedRainLayer>(entity).is_none())
        .count(); // 测试侧统计实际更新数量。
    assert!(excluded > 0 && excluded <= 64); // 不能在改名触发帧偷偷完成整树同步批处理。
    assert!(
        app.world()
            .resource::<super::super::resources::CaptureState>()
            .casters_pending
    ); // 部分集合不能生成被误认作完整的深度图。
    for _ in 0..4 {
        app.update();
    } // 后续帧消费剩余游标范围。
    assert!(
        children
            .iter()
            .all(|&entity| app.world().get::<OwnedRainLayer>(entity).is_none())
    ); // 尾部source也必须最终撤销，不能截断。
    assert!(
        !app.world()
            .resource::<super::super::resources::CaptureState>()
            .casters_pending
    ); // 完整批次才恢复捕获资格。
} // 稳定之后队列为空，不再遍历后代。
