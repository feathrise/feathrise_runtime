//! placement整网格水体根的固定容量去重待处理队列。

use std::collections::HashMap; // 实体到前后邻居的固定容量索引同时提供FIFO、去重和O(1)移除。

use bevy::prelude::*; // Entity与Resource用于保存轻量根身份，不复制网格或作者快照。

 // 队列容量由功能域固定硬上限统一约束。

/// 单次入队的三种确定结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)] // observer和Changed收集系统据此发布Pending或Failed。
pub(super) enum MeshWaterEnqueueResult {
    Queued,    // 新根已进入队尾，后续按每帧一个根处理。
    Coalesced, // 根已经在队列中，只需继续读取组件里的最新作者快照。
    Full,      // 固定容量已满，调用方必须失败关闭而不能丢弃别的根。
} // 结束入队结果。

/// 固定容量、按根实体去重的整网格赋水待处理队列。
#[derive(Resource, Default)] // 资源只保存轻量Entity；实际作者值始终留在根组件上作为唯一最新权威。
pub(in crate::shared::large_scene::water) struct DynamicWaterMeshOverrideQueue {
    head: Option<Entity>,                        // 下一帧应处理的最早有效根。
    tail: Option<Entity>,                        // 新请求O(1)追加到当前队尾。
    links: HashMap<Entity, MeshWaterQueueLinks>, // 每个有效根只占一项并可在despawn时O(1)摘除。
} // 结束整网格赋水待处理队列。

/// 单个有效待处理根在FIFO中的双向邻接关系。
#[derive(Clone, Copy, Debug)] // 两个Entity可直接按值更新，不保存作者快照或场景资产。
struct MeshWaterQueueLinks {
    previous: Option<Entity>, // None表示当前根是队首。
    next: Option<Entity>,     // None表示当前根是队尾。
} // 结束队列邻接记录。

impl DynamicWaterMeshOverrideQueue {
    /// 入队一个根；已经等待的根不复制快照也不新增第二项。
    pub(super) fn enqueue(&mut self, root: Entity) -> MeshWaterEnqueueResult {
        if self.links.contains_key(&root) {
            return MeshWaterEnqueueResult::Coalesced; // 处理系统最终读取根上的最新完整组件值。
        } // 完成同根请求合并。
        if self.links.try_reserve(1).is_err() {
            return MeshWaterEnqueueResult::Full; // 不突破固定内存和最坏等待根数。
        } // 完成队列容量门。
        let previous = self.tail; // 新根的前驱就是当前队尾，空队列则为None。
        let old = self.links.insert(
            root,
            MeshWaterQueueLinks {
                previous,   // 记录原队尾以支持后续O(1)取消。
                next: None, // 新根成为新队尾，暂时没有后继。
            },
        ); // 完成新根唯一邻接记录登记。
        debug_assert!(old.is_none(), "上方contains_key已排除重复根"); // 调试构建锁定去重不变量。
        if let Some(previous) = previous {
            self.links
                .get_mut(&previous)
                .expect("队尾必须存在于有效根索引")
                .next = Some(root); // 原队尾链接到新根，保持FIFO顺序。
        } else {
            self.head = Some(root); // 空队列的第一项同时成为队首。
        } // 完成原队尾或空队列链接。
        self.tail = Some(root); // 无论此前是否为空，新根都成为唯一队尾。
        MeshWaterEnqueueResult::Queued // 返回新入队结果供调用方发布Pending。
    } // 结束根入队。

    /// 弹出本帧唯一允许尝试处理的队首根。
    pub(super) fn pop_front(&mut self) -> Option<Entity> {
        let root = self.head?; // 空队列直接结束本帧，不扫描任何placement。
        let removed = self.cancel(root); // 队首同样走O(1)统一摘链，后续Changed可重新入队。
        debug_assert!(removed, "队首必须存在于有效根索引"); // 调试构建锁定队列内部一致性。
        Some(root) // 即使根随后已删除，本帧也只消费这一项以保持固定峰值。
    } // 结束队首弹出。

    /// 根覆盖移除或实体despawn时O(1)撤销尚未处理的请求。
    pub(super) fn cancel(&mut self, root: Entity) -> bool {
        let Some(links) = self.links.remove(&root) else {
            return false; // 根不在队列时保持幂等，不影响其他请求顺序。
        }; // 先拥有前后邻居身份，再分别修补链路。
        if let Some(previous) = links.previous {
            self.links
                .get_mut(&previous)
                .expect("有效前驱必须仍存在于队列索引")
                .next = links.next; // 从前驱跳过当前根直接连接后继。
        } else {
            self.head = links.next; // 当前根是队首时把后继提升为新队首。
        } // 完成前驱或队首修补。
        if let Some(next) = links.next {
            self.links
                .get_mut(&next)
                .expect("有效后继必须仍存在于队列索引")
                .previous = links.previous; // 从后继反向跳过当前根连接前驱。
        } else {
            self.tail = links.previous; // 当前根是队尾时把前驱降为新队尾。
        } // 完成后继或队尾修补。
        true // 返回确实撤销一项，供测试和调试锁定生命周期。
    } // 结束根请求撤销。

    #[cfg(test)] // 纯队列回归只读内部长度，不向运行时暴露诊断接口。
    pub(super) fn len(&self) -> usize {
        self.links.len() // 邻接索引数量就是当前有效待处理根数量。
    } // 结束测试长度读取。
} // 结束整网格赋水队列操作。
