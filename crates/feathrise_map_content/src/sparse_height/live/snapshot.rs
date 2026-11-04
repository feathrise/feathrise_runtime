//! 未保存高度的持久四叉索引；32层固定地址路径更新，后台只遍历请求Mip页覆盖的实际块。

use super::{MapId, SparseHeightTile, SparseHeightTileKey};
use std::sync::Arc;

#[cfg(test)]
std::thread_local! {
    static INDEX_WRITES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static QUERY_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
enum Node {
    Branch { children: [Option<Arc<Node>>; 4], bounds: Option<[f32; 2]> },
    Tile(SparseHeightTileKey, Arc<SparseHeightTile>),
}

impl Node {
    fn height_bounds(&self) -> Option<[f32; 2]> {
        match self { Self::Branch { bounds, .. } => *bounds, Self::Tile(_, tile) => tile.meter_bounds() }
    }
}

fn union_bounds(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0].min(b[0]), a[1].max(b[1])]
}

/// 唯一完整块表；资源继承、后台快照和Mip占用都共享这份根，避免另存BTree镜像后Clone或下笔整图COW。
#[derive(Clone, Default)]
pub(super) struct LiveTileIndex {
    root: Option<Arc<Node>>,
    count: usize,
}

impl LiveTileIndex {
    pub(super) fn insert(&mut self, key: SparseHeightTileKey, tile: Arc<SparseHeightTile>) {
        if insert(&mut self.root, key, tile, u32::BITS) {
            self.count += 1;
        }
    }

    pub(super) fn len(&self) -> usize {
        self.count
    }

    pub(super) fn height_bounds(&self) -> Option<[f32; 2]> {
        self.root.as_deref().and_then(Node::height_bounds)
    }

    pub(super) fn tile(&self, key: SparseHeightTileKey) -> Option<&Arc<SparseHeightTile>> {
        match mip_node(self.root.as_deref(), 0, key)? {
            Node::Tile(_, tile) => Some(tile),
            Node::Branch { .. } => None,
        }
    }

    pub(super) fn has_mip_tiles(&self, level: u8, key: SparseHeightTileKey) -> bool {
        mip_node(self.root.as_deref(), level, key).is_some()
    }

    pub(super) fn tile_after(
        &self,
        after: Option<SparseHeightTileKey>,
    ) -> Option<(SparseHeightTileKey, &Arc<SparseHeightTile>)> {
        let root = self.root.as_deref()?;
        match after {
            Some(key) => successor(root, key, u32::BITS),
            None => first_tile(root),
        }
    }

    pub(super) fn tiles(
        &self,
    ) -> impl ExactSizeIterator<Item = (SparseHeightTileKey, &Arc<SparseHeightTile>)> {
        let mut stack = [None; 3 * u32::BITS as usize + 1];
        stack[0] = self.root.as_deref();
        TileIter {
            stack,
            len: usize::from(self.root.is_some()),
            remaining: self.count,
        }
    }

    pub(super) fn snapshot(
        &self,
        map_id: Option<MapId>,
        revision: u64,
    ) -> SparseHeightLiveSnapshot {
        SparseHeightLiveSnapshot {
            map_id,
            revision,
            count: self.count,
            root: self.root.clone(),
        }
    }
}

fn insert(
    link: &mut Option<Arc<Node>>,
    key: SparseHeightTileKey,
    tile: Arc<SparseHeightTile>,
    remaining: u32,
) -> bool {
    #[cfg(test)]
    INDEX_WRITES.with(|count| count.set(count.get() + 1));
    if remaining == 0 {
        let new = link.is_none();
        *link = Some(Arc::new(Node::Tile(key, tile)));
        return new;
    }
    let node = link.get_or_insert_with(|| Arc::new(Node::Branch { children: [None, None, None, None], bounds: None }));
    let Node::Branch { children, bounds } = Arc::make_mut(node) else {
        unreachable!("固定32层地址的叶节点只位于完整键末端");
    };
    let bit = remaining - 1;
    let slot = ((key.x >> bit) & 1) | (((key.z >> bit) & 1) << 1);
    let new = insert(&mut children[slot as usize], key, tile, bit);
    *bounds = children.iter().flatten().filter_map(|child| child.height_bounds()).reduce(union_bounds);
    new // 每次只沿原32层地址更新四个子摘要，不扫描整棵作者树。
}

/// 前缀即Mip占用，不另维护随全图增长的祖先集合。每次最多32条边，不依赖相交块数。
fn mip_node(root: Option<&Node>, level: u8, key: SparseHeightTileKey) -> Option<&Node> {
    let prefix = u32::BITS.checked_sub(u32::from(level))?;
    if key.x.checked_shr(prefix).is_some_and(|value| value != 0)
        || key.z.checked_shr(prefix).is_some_and(|value| value != 0)
    {
        return None;
    }
    let mut node = root?;
    for bit in (0..prefix).rev() {
        #[cfg(test)]
        QUERY_VISITS.with(|count| count.set(count.get() + 1));
        let Node::Branch { children, .. } = node else {
            return None;
        };
        let slot = ((key.x >> bit) & 1) | (((key.z >> bit) & 1) << 1);
        node = children[slot as usize].as_deref()?;
    }
    Some(node)
}

fn first_tile(mut node: &Node) -> Option<(SparseHeightTileKey, &Arc<SparseHeightTile>)> {
    loop {
        #[cfg(test)]
        QUERY_VISITS.with(|count| count.set(count.get() + 1));
        match node {
            Node::Tile(key, tile) => return Some((*key, tile)),
            Node::Branch { children, .. } => node = children.iter().find_map(Option::as_deref)?,
        }
    }
}

/// 先沿游标路径查找，再沿第一个后继子树降到底；最多两条32层路径，不为维持顺序扫描整树。
fn successor(
    node: &Node,
    after: SparseHeightTileKey,
    remaining: u32,
) -> Option<(SparseHeightTileKey, &Arc<SparseHeightTile>)> {
    #[cfg(test)]
    QUERY_VISITS.with(|count| count.set(count.get() + 1));
    let Node::Branch { children, .. } = node else {
        return None;
    };
    let bit = remaining.checked_sub(1)?;
    let slot = (((after.x >> bit) & 1) | (((after.z >> bit) & 1) << 1)) as usize;
    if let Some(child) = children[slot].as_deref()
        && let Some(next) = successor(child, after, bit)
    {
        return Some(next);
    }
    children[slot + 1..]
        .iter()
        .find_map(Option::as_deref)
        .and_then(first_tile)
}

/// 深度32的四叉DFS最多同时保留1+3×32个节点引用；迭代器不申请与地图规模相关的临时数组。
struct TileIter<'a> {
    stack: [Option<&'a Node>; 3 * u32::BITS as usize + 1],
    len: usize,
    remaining: usize,
}
impl<'a> Iterator for TileIter<'a> {
    type Item = (SparseHeightTileKey, &'a Arc<SparseHeightTile>);
    fn next(&mut self) -> Option<Self::Item> {
        while self.len > 0 {
            self.len -= 1;
            match self.stack[self.len].take()? {
                Node::Tile(key, tile) => {
                    self.remaining -= 1;
                    return Some((*key, tile));
                }
                Node::Branch { children, .. } => {
                    for child in children.iter().rev().flatten() {
                        self.stack[self.len] = Some(child);
                        self.len += 1;
                    }
                }
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for TileIter<'_> {}

/// 地图、修订和一份持久根就是完整后台输入；克隆只增加一次根Arc计数。
/// 快照可能保有旧作者块，调用方须在原后台worker或既有回收槽中释放，不能在帧内枚举或全树析构。
#[derive(Clone)]
pub struct SparseHeightLiveSnapshot {
    map_id: Option<MapId>,
    revision: u64,
    count: usize,
    root: Option<Arc<Node>>,
}

impl SparseHeightLiveSnapshot {
    /// 后台增量消费者跳过指针相同的持久子树，只访问本轮真正改变的页；不扫描全部已触及地形。
    /// 此索引只插入/替换页；整图撤回由调用方的地图/空快照身份显式重置。
    pub fn visit_changed_tiles(&self, previous: Option<&Self>, mut visit: impl FnMut(SparseHeightTileKey, &Arc<SparseHeightTile>)) {
        let previous = previous.filter(|old| old.map_id == self.map_id);
        visit_changed(self.root.as_ref(), previous.and_then(|old| old.root.as_ref()), &mut visit);
    }
    pub fn map_id(&self) -> Option<&MapId> {
        self.map_id.as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn tile_count(&self) -> usize {
        self.count
    }

    pub fn height_bounds(&self) -> Option<[f32; 2]> {
        self.root.as_deref().and_then(Node::height_bounds)
    }

    /// 仅供已有后台任务：按祖先页前缀直达子树，遍历成本只随真实相交块增长。
    /// level0访问一个基础块，level32覆盖全部u32地址；越界层级或不能表达的祖先键返回空集。
    pub fn visit_mip_tiles(
        &self,
        level: u8,
        key: SparseHeightTileKey,
        mut visit: impl FnMut(SparseHeightTileKey, &Arc<SparseHeightTile>),
    ) {
        let Some(node) = mip_node(self.root.as_deref(), level, key) else {
            return;
        };
        visit_subtree(node, &mut visit);
    }
}

fn visit_changed(current: Option<&Arc<Node>>, previous: Option<&Arc<Node>>,
    visit: &mut impl FnMut(SparseHeightTileKey, &Arc<SparseHeightTile>)) {
    let Some(current) = current else { return; };
    if previous.is_some_and(|old| Arc::ptr_eq(current, old)) { return; }
    match current.as_ref() {
        Node::Tile(key, tile) => {
            if !previous.is_some_and(|old| matches!(old.as_ref(), Node::Tile(old_key, old_tile)
                if old_key == key && Arc::ptr_eq(tile, old_tile))) { visit(*key, tile); }
        }
        Node::Branch { children, .. } => {
            let old = previous.and_then(|old| match old.as_ref() {
                Node::Branch { children, .. } => Some(children), _ => None,
            });
            for (index, child) in children.iter().enumerate() {
                visit_changed(child.as_ref(), old.and_then(|children| children[index].as_ref()), visit);
            }
        }
    }
}

fn visit_subtree(node: &Node, visit: &mut impl FnMut(SparseHeightTileKey, &Arc<SparseHeightTile>)) {
    #[cfg(test)]
    QUERY_VISITS.with(|count| count.set(count.get() + 1));
    match node {
        Node::Tile(key, tile) => visit(*key, tile),
        Node::Branch { children, .. } => {
            for child in children.iter().flatten() {
                visit_subtree(child, visit);
            }
        }
    }
}

#[cfg(test)]
mod tests;
