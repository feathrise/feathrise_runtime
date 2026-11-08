// glTF primitive的祖先规划归属索引：每源最多64级、每帧最多64个定向更新，不扫描或复制整棵场景。
use bevy::prelude::*; // 读取Name和ChildOf的生命周期变化。
use std::collections::{BTreeMap, BTreeSet, VecDeque}; // 有序后代集合支持游标续扫，不克隆整个索引桶。
use std::ops::Bound::{Excluded, Unbounded}; // 从上帧游标之后获取一个后代。

const ANCESTOR_LIMIT: usize = 64; // 与项目其他祖先归属查询采用同样明确的深度边界。
const UPDATE_BUDGET: usize = 64; // 改名/重挂大根时限制单帧祖先检查数量，不阻塞主线程。

#[derive(Resource, Default)] // 索引随真实源登记增长，不保存模型或资产副本。
struct Index {
    // 所有数据仅由主世界观察器及固定预算系统操作。
    sources: BTreeMap<Entity, Vec<Entity>>, // 每个标准材质源登记自身及最多63级祖先。
    descendants: BTreeMap<Entity, BTreeSet<Entity>>, // 父级改名只访问自身关联源，避免全世界搜索。
    pending: BTreeMap<Entity, Option<Entity>>, // 脏键去重并保存游标，处理中再次改名只重置此键而不复制队列。
    queue: VecDeque<Entity>,                   // 每个已登记脏祖先最多一项，避免逐后代排入无界任务。
} // 祖先已删除时，源移除/重挂会定向归还反向登记。

pub(super) fn install(app: &mut App) {
    // 在caster生成前建立索引和层级事件入口。
    app.init_resource::<Index>(); // 空世界没有初始扫描。
    app.add_observer(parent_inserted)
        .add_observer(parent_removed); // 重挂与解除父节点统一失效。
    app.add_systems(
        PostUpdate,
        process.before(super::super::RainOcclusionUpdate),
    ); // 本帧定向变更先于深度捕获需求。
} // 稳定帧只检查空队列。

fn parent_inserted(event: On<Insert<ChildOf>>, mut commands: Commands) {
    // 源和任一祖先挂到新父节点都会影响规划归属。
    super::enqueue_family(event.entity, &mut commands); // 延迟读取已完成关系变更。
} // 不读取完整Children列表。

fn parent_removed(event: On<Remove<ChildOf>>, mut commands: Commands) {
    // 脱离规划父级后需要重新登记世界层。
    super::enqueue_family(event.entity, &mut commands); // Remove完成后再读取当前祖先链。
} // despawn时源清理同样幂等。

pub(super) fn invalidate(world: &mut World, ancestor: Entity) {
    // 单祖先脏标记不会遍历后代。
    let mut index = world.resource_mut::<Index>(); // 独占固定元数据操作。
    if index.descendants.contains_key(&ancestor) {
        // 没有关联源时无后续工作。
        if index.pending.insert(ancestor, None).is_none() {
            index.queue.push_back(ancestor);
        } // 重复变更重置游标但不重复排队。
    } // 队列由注册源规模而非重复事件次数约束。
} // 不分配后代快照。

pub(super) fn forget(world: &mut World, source: Entity) {
    // 源卸载只处理其有限登记链。
    let mut index = world.resource_mut::<Index>(); // 与重新登记串行，避免陈旧关联。
    let Some(ancestors) = index.sources.remove(&source) else {
        return;
    }; // 未登记源无需工作。
    for ancestor in ancestors {
        // 最多64个祖先键。
        if let Some(members) = index.descendants.get_mut(&ancestor) {
            // 允许索引已被清理。
            members.remove(&source); // 归还这个源的一条反向边。
            if members.is_empty() {
                index.descendants.remove(&ancestor);
            } // 空桶不积累卸载地图实体。
        } // 排队的旧桶键将在固定预算处理中自然失效。
    } // 不触碰其他源。
} // 清理结束。

pub(super) fn refresh(world: &mut World, source: Entity) -> bool {
    // 返回当前源是否因规划或异常层级而禁止捕获。
    let mut ancestors = Vec::with_capacity(8); // 常见glTF深度较浅，最坏只扩展到64。
    let mut cursor = Some(source); // 包含自身名称，兼容直接命名的网格。
    let mut excluded = false; // 任意祖先规划名都排除整个primitive。
    for _ in 0..ANCESTOR_LIMIT {
        // 不允许损坏层级触发无界爬树。
        let Some(entity) = cursor else { break }; // 已到根节点。
        if ancestors.contains(&entity) {
            excluded = true;
            break;
        } // 异常循环保守拒绝。
        let Ok(node) = world.get_entity(entity) else {
            excluded = true;
            break;
        }; // 失效父节点不冒充有效世界几何。
        ancestors.push(entity); // 为改名和重挂登记精准反向边。
        excluded |= node
            .get::<Name>()
            .is_some_and(|name| name.as_str().starts_with("规划_")); // 父级规划命名对子primitive同样生效。
        cursor = node.get::<ChildOf>().map(ChildOf::parent); // 单边向上查询不展开兄弟或子树。
    } // 最多固定64次组件查找。
    excluded |= cursor.is_some(); // 深度超界或循环保守不进入遮雨层。
    if world.resource::<Index>().sources.get(&source) != Some(&ancestors) {
        // 相同层级不重建反向集合。
        forget(world, source); // 先撤销旧的有限祖先链。
        let mut index = world.resource_mut::<Index>(); // 只发布该源的新完整关系。
        for ancestor in &ancestors {
            index
                .descendants
                .entry(*ancestor)
                .or_default()
                .insert(source);
        } // 有界反向边登记。
        index.sources.insert(source, ancestors); // 保存完整源链供后续卸载归还。
    } // 名称变化仅重新判定，不重建几何。
    excluded // 上层据此添加或撤销自有层。
} // 初次实体创建在原发布阶段完成有限登记。

fn process(world: &mut World) {
    // 独占系统只为单实体reconcile获取World，不扫描实体表。
    for _ in 0..UPDATE_BUDGET {
        // 大子树事件定向分帧，稳定帧首轮立即退出。
        let source = {
            // 释放索引借用后再更新实体层。
            let mut index = world.resource_mut::<Index>(); // 只查看队首一个祖先桶。
            let Some(ancestor) = index.queue.pop_front() else {
                break;
            }; // 空队列零后代工作。
            let cursor = index.pending.get(&ancestor).copied().flatten(); // 中途再次改名已将此游标重置为None。
            let next = index.descendants.get(&ancestor).and_then(|members| {
                // 桶可能已经因源删除而消失。
                if let Some(cursor) = cursor {
                    members.range((Excluded(cursor), Unbounded)).next().copied()
                } else {
                    members.first().copied()
                } // 有序游标取下一源，不复制桶。
            }); // 找到最多一个源。
            if let Some(entity) = next {
                index.pending.insert(ancestor, Some(entity));
                index.queue.push_front(ancestor);
            } else {
                index.pending.remove(&ancestor);
            } // 保存游标或结束此桶。
            next // 释放索引借用。
        }; // 批次中至多一个源即将重新登记。
        if let Some(entity) = source {
            super::reconcile(world, entity);
        } // 仅变化相关源读取至多64祖先。
    } // 后续脏源留在原队列，没有后台任务或无界输入复制。
    let pending = !world.resource::<Index>().queue.is_empty(); // 只有完整资格批次才允许生产新深度图。
    if let Some(mut capture) = world.get_resource_mut::<super::super::resources::CaptureState>() {
        // 独立caster测试可没有捕获资源。
        if capture.casters_pending != pending {
            capture.casters_pending = pending;
        } // 将部分批次显式托底，避免当作完整结果发布。
    } // 不复制待处理源集合。
} // 与捕获更新顺序固定，处理结果在当前帧可见。
