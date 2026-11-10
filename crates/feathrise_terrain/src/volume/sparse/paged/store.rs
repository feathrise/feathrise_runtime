//! 内容寻址对象与固定深度写时复制索引；正式引用只在保存事务中发布。
use super::types::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const NODE_BYTES: usize = 4 + 16 * 32;
const MAX_NODES: usize = 14_336; // 包括HashMap容量和待写身份，实际索引缓冲保持在16MiB以内。
static TEMP_ID: AtomicU64 = AtomicU64::new(0);
pub(super) fn digest(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}
pub(super) fn hex(hash: Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) struct Store<'a> {
    root: &'a Path,
    nodes: HashMap<Hash, [Hash; 16]>,
    pending: HashSet<Hash>,
    read_bytes: usize,
    display_read: bool,
}
impl<'a> Store<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            nodes: HashMap::new(),
            pending: HashSet::new(),
            read_bytes: 0,
            display_read: false,
        }
    }
    /// 显示整层按小对象顺序读取，累计IO量不代表同时驻留量；索引缓存可回收后继续读。
    pub(super) fn display(root: &'a Path) -> Self {
        Self {
            display_read: true,
            ..Self::new(root)
        }
    }
    fn path(&self, hash: Hash) -> PathBuf {
        self.root.join(format!("{}.vblk", hex(hash)))
    }
    pub fn read(&mut self, hash: Hash, max: usize) -> Result<Vec<u8>, String> {
        let file =
            std::fs::File::open(self.path(hash)).map_err(|e| format!("读取体积块失败：{e}"))?;
        let len = usize::try_from(file.metadata().map_err(|e| e.to_string())?.len())
            .map_err(|_| "体积块长度溢出")?;
        if len > max {
            return Err("体积对象长度超过格式上限".into());
        }
        if !self.display_read && self.read_bytes.saturating_add(len) > MAX_IO_BYTES {
            return Err("当前驻留窗口对象读取超过32MiB预算".into());
        }
        let mut bytes = Vec::with_capacity(len);
        file.take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() != len || digest(&bytes) != hash {
            return Err("体积块被截断或摘要不一致；保留上一完整地形".into());
        }
        self.read_bytes = self.read_bytes.saturating_add(bytes.len());
        Ok(bytes)
    }
    pub(super) fn node(&mut self, hash: Hash) -> Result<[Hash; 16], String> {
        if hash == EMPTY {
            return Ok([EMPTY; 16]);
        }
        if let Some(node) = self.nodes.get(&hash) {
            return Ok(*node);
        }
        if self.nodes.len() >= MAX_NODES {
            if self.display_read {
                self.nodes.clear(); // 不可变读缓存没有待写节点，清空后按需重读，不截断作者目录。
            } else {
                return Err("体积索引驻留超过16MiB预算".into());
            }
        }
        let bytes = self.read(hash, NODE_BYTES)?;
        if bytes.len() != NODE_BYTES || &bytes[..4] != b"VIX3" {
            return Err("体积索引节点格式无效".into());
        }
        let mut node = [EMPTY; 16];
        for (dst, raw) in node.iter_mut().zip(bytes[4..].chunks_exact(32)) {
            dst.copy_from_slice(raw);
        }
        self.nodes.insert(hash, node);
        Ok(node)
    }
    pub fn lookup(&mut self, mut root: Hash, key: u64) -> Result<Option<Hash>, String> {
        for depth in 0..16 {
            if root == EMPTY {
                return Ok(None);
            }
            root = self.node(root)?[((key >> ((15 - depth) * 4)) & 15) as usize];
        }
        Ok((root != EMPTY).then_some(root))
    }
    pub fn leaf(&mut self, root: Hash, key: u64, max: usize) -> Result<Option<Vec<u8>>, String> {
        self.lookup(root, key)?
            .map(|h| self.read(h, max))
            .transpose()
    }
    /// 只遍历与驻留网格范围相交的稀疏Morton目录；远景简化的三角内部也必须取得作者块。
    /// 调用方使用有界IO槽，目录读取继续共享32MiB/16MiB信用，不枚举范围内全部空格。
    pub(super) fn leaves_in_bounds(
        &mut self,
        root: Hash,
        minimum: [i32; 3],
        maximum: [i32; 3],
    ) -> Result<Vec<([i32; 3], Hash)>, String> {
        let mut stack = vec![(root, 0u64, 16u32, [-1_048_576; 3], [1_048_575; 3])];
        let mut result = Vec::new();
        while let Some((hash, prefix, depth, lo, hi)) = stack.pop() {
            if hash == EMPTY || (0..3).any(|i| hi[i] < minimum[i] || lo[i] > maximum[i]) {
                continue;
            }
            if depth == 0 {
                if !self.display_read && result.len() >= 32768 {
                    return Err("三维作者目录窗口超过32768块预算".into());
                }
                result.push((lo, hash));
                continue;
            }
            let shift = (depth - 1) * 4;
            for (slot, child) in self.node(hash)?.into_iter().enumerate() {
                if child == EMPTY || shift == 60 && slot >= 8 {
                    continue;
                }
                let mut a = lo;
                let mut b = hi;
                for bit in 0..4 {
                    let position = shift + bit;
                    if position >= 63 {
                        continue;
                    }
                    let axis = (position % 3) as usize;
                    let amount = 1i32 << (position / 3);
                    if slot & (1 << bit) != 0 {
                        a[axis] += amount;
                    } else {
                        b[axis] -= amount;
                    }
                }
                stack.push((child, prefix | ((slot as u64) << shift), depth - 1, a, b));
            }
        }
        Ok(result)
    }
    fn update(&mut self, root: Hash, key: u64, leaf: Hash, depth: usize) -> Result<Hash, String> {
        debug_assert!(!self.display_read, "显示读缓存不能承担写时复制事务");
        if depth == 16 {
            return Ok(leaf);
        }
        let mut node = self.node(root)?;
        let slot = ((key >> ((15 - depth) * 4)) & 15) as usize;
        node[slot] = self.update(node[slot], key, leaf, depth + 1)?;
        let mut bytes = Vec::with_capacity(NODE_BYTES);
        bytes.extend_from_slice(b"VIX3");
        for child in node {
            bytes.extend_from_slice(&child);
        }
        let hash = digest(&bytes);
        if self.nodes.len() >= MAX_NODES {
            return Err("体积索引变更超过16MiB预算".into());
        }
        self.nodes.insert(hash, node);
        self.pending.insert(hash);
        Ok(hash)
    }
    pub fn set(&mut self, root: Hash, write: &LeafWrite) -> Result<Hash, String> {
        let leaf = if write.bytes.is_empty() {
            EMPTY
        } else {
            self.write(&write.bytes)?
        };
        self.update(root, write.key, leaf, 0)
    }
    pub fn flush(&self, roots: &[Hash]) -> Result<(), String> {
        fn visit(store: &Store<'_>, hash: Hash, seen: &mut HashSet<Hash>) -> Result<(), String> {
            if !store.pending.contains(&hash) || !seen.insert(hash) {
                return Ok(());
            }
            let node = store.nodes.get(&hash).ok_or("待写体积索引节点缺失")?;
            for &child in node {
                visit(store, child, seen)?;
            }
            let mut bytes = Vec::with_capacity(NODE_BYTES);
            bytes.extend_from_slice(b"VIX3");
            for child in node {
                bytes.extend_from_slice(child);
            }
            store.write(&bytes)?;
            Ok(())
        }
        let mut seen = HashSet::new();
        for &root in roots {
            visit(self, root, &mut seen)?;
        }
        Ok(()) // 只写本段最终根可达的路径，中间刷印产生的临时索引不会充斥磁盘。
    }
    pub(super) fn write(&self, bytes: &[u8]) -> Result<Hash, String> {
        let hash = digest(bytes);
        let path = self.path(hash);
        if path.exists() {
            if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() != bytes.len() as u64
                || std::fs::read(&path).map_err(|e| e.to_string())? != bytes
            {
                return Err("已有体积摘要对象内容不同，禁止覆盖".into());
            }
            return Ok(hash);
        }
        std::fs::create_dir_all(self.root).map_err(|e| format!("创建体积对象目录失败：{e}"))?;
        let temp = self.root.join(format!(
            ".{}-{}-{}.tmp",
            hex(hash),
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("体积块落盘失败：{e}"))?;
            drop(file);
            // 不覆盖已有对象。并行相同摘要写入的赢家只能有一份相同的完整文件。
            std::fs::hard_link(&temp, &path)
                .or_else(|e| if path.exists() { Ok(()) } else { Err(e) })
                .map_err(|e| e.to_string())?;
            if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() != bytes.len() as u64
                || std::fs::read(&path).map_err(|e| e.to_string())? != bytes
            {
                return Err("体积块发布校验失败".into());
            }
            Ok(hash)
        })();
        let _ = std::fs::remove_file(temp);
        result
    }
}

/// 保存IO复验固定四份根；子对象在实际加载时逐项校验摘要，不把全树扫描塞进保存。
pub fn validate_roots(root: &Path, volume: &PagedVolume) -> Result<(), String> {
    volume.validate()?;
    let mut store = Store::new(root);
    for hash in [volume.columns, volume.density, volume.meshes, volume.fields] {
        store.node(hash)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::Temp;
    use super::*;
    #[test]
    fn display_reads_can_continue_past_batch_io_and_recycle_index_cache() {
        let temp = Temp::new();
        let bytes = vec![7u8; 2 * 1024 * 1024];
        let hash = Store::new(&temp.0).write(&bytes).unwrap();
        let mut display = Store::display(&temp.0);
        for _ in 0..17 {
            assert_eq!(display.read(hash, bytes.len()).unwrap(), bytes);
        }
        let mut edit = Store::new(&temp.0);
        edit.read_bytes = MAX_IO_BYTES;
        assert!(edit.read(hash, bytes.len()).is_err()); // 内部写批次仍有背压，不能变成全图同步工作。
        display.nodes.extend((0..MAX_NODES as u32).map(|i| {
            let mut key = [0; 32];
            key[..4].copy_from_slice(&i.to_le_bytes());
            (key, [EMPTY; 16])
        }));
        let mut node = b"VIX3".to_vec();
        node.extend([0; 16 * 32]);
        let hash = Store::new(&temp.0).write(&node).unwrap();
        assert_eq!(display.node(hash).unwrap(), [EMPTY; 16]);
        assert_eq!(display.nodes.len(), 1);
    }
}
