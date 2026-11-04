// 自动把世界标准材质网格登记到雨向深度层；生命周期事件只访问当前实体，不扫描地图。
// 源层与名称需通过 insert/remove 发布变更；目前项目的原地 RenderLayers 修改仅用于相机和灯光。
use bevy::camera::visibility::RenderLayers; // 与原世界层共存，不改变资产预览的可见性隔离。
use bevy::prelude::*; // 复用标准材质、ECS 生命周期及命令队列。
use feathrise_scene_core::constants::render_layers::RAIN_OCCLUSION_LAYER; // 专用层集中分配，避免与水面和预览通道冲突。
mod hierarchy; // 父级规划名通过有界祖先登记和反向索引传播到glTF primitive。

#[derive(Component, Clone, Copy)] // 仅记录本模块实际添加过的层，外部已有层不归本模块撤销。
pub(super) struct OwnedRainLayer {
    // 生产缓存只检查已登记源变化，避免无关角色移动使静态缓存失效。
    was_implicit: bool, // 无 RenderLayers 等价于世界层 0，不能误变成无可见层。
} // 所有权组件不携带网格或材质副本。

pub(super) fn install(app: &mut App) {
    // 在场景生成前注册，之后每个源实体独立接管。
    hierarchy::install(app); // 重挂事件和父级名称脏队列不扫描全世界。
    app.add_observer(material_inserted) // 标准材质首次生成和替换都重新检查当前实体。
        .add_observer(material_removed) // 改成定制地形、草或水材质时撤销自动层。
        .add_observer(layers_inserted) // 预览系统后写层时及时消除暂存的世界层资格。
        .add_observer(layers_removed) // 移除显式层意味着重新使用隐式世界层。
        .add_observer(name_inserted) // 规划对象改名后立即退出捕获。
        .add_observer(name_removed); // 移除规划名后可重新成为正常世界对象。
} // 不添加每帧遍历所有标准材质实体的系统。

fn material_inserted(event: On<Insert, MeshMaterial3d<StandardMaterial>>, mut commands: Commands) {
    // 材质生命周期事件定位唯一源。
    enqueue(event.entity, &mut commands); // 延迟到 bundle 完整安装后读取最终状态。
} // 不在事件中复制资产。

fn material_removed(event: On<Remove, MeshMaterial3d<StandardMaterial>>, mut commands: Commands) {
    // Remove 触发时旧材质仍存在。
    enqueue(event.entity, &mut commands); // 命令执行时旧材质已移除，避免误判仍有源。
} // despawn 后的命令允许静默退出。

fn layers_inserted(event: On<Insert, RenderLayers>, mut commands: Commands) {
    // 同时接收外部层调整和本模块自己的插入。
    enqueue(event.entity, &mut commands); // reconcile 对相同层无写入，因此不会产生无限递归。
} // 单实体排队成本不随地图规模增加。

fn layers_removed(event: On<Remove, RenderLayers>, mut commands: Commands) {
    // 恢复隐式世界层也需要更新资格。
    enqueue(event.entity, &mut commands); // 当前实体最终状态是唯一权威来源。
} // 不缓存可能已过时的事件快照。

fn name_inserted(event: On<Insert, Name>, mut commands: Commands) {
    // 规划前缀是项目约定的唯一辅助对象标识。
    enqueue_family(event.entity, &mut commands); // glTF的名称通常位于primitive父节点，需要定向更新已登记后代。
} // 不按模型文件名猜测屋顶。

fn name_removed(event: On<Remove, Name>, mut commands: Commands) {
    // 名称删除后解除规划名排除条件。
    enqueue_family(event.entity, &mut commands); // 只标记此祖先的反向索引桶，不复制全部子节点。
} // 事件不自行操作实体层。

fn enqueue(entity: Entity, commands: &mut Commands) {
    // 将单实体检查推迟到结构变更之后。
    commands.queue(move |world: &mut World| reconcile(world, entity)); // 同一 bundle 多个事件可安全重复执行。
} // 命令仅捕获 Entity，不保留资产或世界快照。

fn enqueue_family(entity: Entity, commands: &mut Commands) {
    // 名称或层级变化既影响自身也影响已登记的后代。
    commands.queue(move |world: &mut World| {
        // 必须读取结构变更完成后的真实祖先链。
        hierarchy::invalidate(world, entity); // 只入队一个祖先键，成本不随后代数增长。
        reconcile(world, entity); // 当前primitive仍在同一命令中立即更新。
    }); // 子primitive由有界定向队列处理。
} // 不遍历Children集合。

fn reconcile(world: &mut World, entity: Entity) {
    // 只做固定数量的组件访问与层位修改。
    if world.get_entity(entity).is_err() {
        hierarchy::forget(world, entity);
        return;
    } // 删除源只清理它最多64项反向登记。
    let source = world.entity(entity); // 已验证源存在。
    let owner = source.get::<OwnedRainLayer>().copied(); // 只有确由本插件添加的层才允许清理。
    let material = source.contains::<MeshMaterial3d<StandardMaterial>>(); // 定制草、水、地形材质不自动参与屋顶捕获。
    if !material && owner.is_none() {
        hierarchy::forget(world, entity);
        return;
    } // 大多数名称与层事件无需分配或写入。
    let layers = source.get::<RenderLayers>(); // 缺失层遵循 Bevy 的默认世界层语义。
    let was_implicit = layers.is_none(); // 新接管时记录是否应恢复组件缺失状态。
    let world_layer = layers.is_none_or(|value| value.iter().any(|layer| layer == 0)); // 仅世界层对象能成为遮雨几何。
    let mut desired = layers.cloned().unwrap_or_default(); // 只复制短小层位集合，不复制 Mesh 或材质。
    let owns_layer = owner.is_some(); // 无所有权时保留外部明确登记的专用层。
    let has_layer = desired.iter().any(|layer| layer == RAIN_OCCLUSION_LAYER); // 幂等判断阻断 Insert 事件递归。
    let planning = if material {
        hierarchy::refresh(world, entity)
    } else {
        hierarchy::forget(world, entity);
        false
    }; // 初次/重挂最多读取64级祖先，规划子网格不得误入捕获。
    let eligible = material && world_layer && !planning; // 透明材质由标准深度预通道自身排除或执行alpha mask。
    if eligible && !has_layer {
        // 世界实体首次进入或被外部整体重写层后恢复捕获。
        desired = desired.with(RAIN_OCCLUSION_LAYER); // 原层全部保留，仅增添专用深度层。
        let mut target = world.entity_mut(entity); // 前面只读借用已结束，可以原子发布两个组件。
        target.insert((desired, owner.unwrap_or(OwnedRainLayer { was_implicit }))); // 层与归还信息同次安装，递归事件读取完整状态。
    } else if !eligible && owns_layer {
        // 源被移除、改到预览层或改为规划节点时撤销接管。
        desired = desired.without(RAIN_OCCLUSION_LAYER); // 只撤销自己的层，保留之后新增的其他通道。
        let restore_implicit = owner.unwrap().was_implicit && desired == RenderLayers::default(); // 只有仍为原默认语义才能删除整个组件。
        let mut target = world.entity_mut(entity); // 原层撤销不访问其他实体。
        target.remove::<OwnedRainLayer>(); // 先归还所有权，后续 Remove/Insert 事件不会重复撤销。
        if restore_implicit {
            // 无外部新增层时恢复作者最初的缺省组件状态。
            target.remove::<RenderLayers>(); // 无材质或规划节点保持原世界可见性。
        } else if has_layer {
            // 外部已经重写并移走 19 层时不制造无意义的 Insert。
            target.insert(desired); // 外部预览层与其他层完整保留。
        } // 层已经符合目标时只清理所有权。
    } // 有资格且已接管的稳定实体不产生任何组件变更。
} // 工作量只与发生生命周期事件的单实体有关。

#[cfg(test)] // 无窗口测试真实观察器事件链与层所有权归还。
mod tests; // 边界用例单独存放，保持实现容易审阅。
