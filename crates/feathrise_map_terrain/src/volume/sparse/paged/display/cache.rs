//! 代理缓存只派生自不可变网格树；摘要对象和固定长度引用都以完整文件发布。
use super::super::{
    residency::MeshBlock,
    store::{Store, hex},
    types::*,
};
use crate::TerrainMeshInput;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub(super) type GridRect = [[i32; 2]; 2];
pub(super) const MAX_BLOB: usize = 60 + 32 * 16 + 8 + 65_536 * 12 + 131_072 * 12;
static TEMP: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy)]
pub(super) struct Key {
    pub source: Hash,
    pub depth: u32,
    pub prefix: u64,
    pub cell_size: f32,
}
impl Key {
    fn path(self, root: &Path) -> PathBuf {
        root.join(format!(
            "lod1-{}-{:08x}-{:016x}-{}.vref",
            self.depth,
            self.cell_size.to_bits(),
            self.prefix,
            hex(self.source)
        ))
    }
    pub fn child(self, source: Hash, slot: usize) -> Self {
        Self {
            source,
            depth: self.depth + 1,
            prefix: self.prefix | ((slot as u64) << ((15 - self.depth) * 4)),
            ..self
        }
    }
}
pub(super) fn read_ref(root: &Path, key: Key) -> Result<Option<Hash>, String> {
    let file = match std::fs::File::open(key.path(root)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取远景代理引用失败：{e}")),
    };
    let mut raw = Vec::with_capacity(33);
    file.take(33)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    Ok(Some(raw.try_into().map_err(|_| "远景代理引用长度无效")?))
}
pub(super) fn publish(root: &Path, key: Key, bytes: &[u8]) -> Result<Hash, String> {
    let hash = Store::new(root).write(bytes)?;
    let path = key.path(root);
    if let Some(old) = read_ref(root, key)? {
        if old != hash {
            return Err("同一源版本的远景代理不一致，禁止覆盖".into());
        }
        return Ok(hash);
    }
    let temp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(&hash)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        std::fs::hard_link(&temp, &path)
            .or_else(|e| if path.exists() { Ok(()) } else { Err(e) })
            .map_err(|e| e.to_string())?;
        if read_ref(root, key)? != Some(hash) {
            return Err("远景代理引用发布不一致".into());
        }
        Ok(hash)
    })();
    let _ = std::fs::remove_file(temp);
    result
}
pub(super) fn encode(key: Key, mesh: Option<TerrainMeshInput>, rects: Vec<GridRect>) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(b"VLP1");
    bytes.extend(key.source);
    bytes.extend(key.cell_size.to_bits().to_le_bytes());
    bytes.extend(key.depth.to_le_bytes());
    bytes.extend(key.prefix.to_le_bytes());
    bytes.extend(u32::from(mesh.is_none()).to_le_bytes());
    bytes.extend((rects.len() as u32).to_le_bytes());
    for r in rects {
        for v in r.into_iter().flatten() {
            bytes.extend(v.to_le_bytes());
        }
    }
    if let Some(mesh) = mesh {
        bytes.extend((mesh.positions.len() as u32).to_le_bytes());
        bytes.extend((mesh.indices.len() as u32).to_le_bytes());
        bytes.extend(
            mesh.positions
                .into_iter()
                .flatten()
                .flat_map(f32::to_le_bytes),
        );
        bytes.extend(mesh.indices.into_iter().flat_map(u32::to_le_bytes));
    }
    bytes
}
pub(super) fn decode(
    key: Key,
    bytes: Vec<u8>,
) -> Result<Option<(MeshBlock, Vec<GridRect>)>, String> {
    if bytes.len() < 60
        || bytes.len() > MAX_BLOB
        || &bytes[..4] != b"VLP1"
        || bytes[4..36] != key.source
        || bytes[36..40] != key.cell_size.to_bits().to_le_bytes()
        || bytes[40..44] != key.depth.to_le_bytes()
        || bytes[44..52] != key.prefix.to_le_bytes()
    {
        return Err("远景代理与源版本或网格层级不一致".into());
    }
    let flag = u32::from_le_bytes(bytes[52..56].try_into().unwrap());
    let n = u32::from_le_bytes(bytes[56..60].try_into().unwrap()) as usize;
    if flag == 1 && bytes.len() == 60 && n == 0 {
        return Ok(None);
    }
    if flag != 0 || n > 32 || bytes.len() < 68 + n * 16 {
        return Err("远景代理尺寸或标志无效".into());
    }
    let offset = 60 + n * 16;
    let vertices = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    let indices = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
    // 删除叶块的持久索引可保留空分支；零覆盖、零几何是已完成的空代理，不是待细分。
    if n == 0 && vertices == 0 && indices == 0 && bytes.len() == offset + 8 {
        return Ok(Some((MeshBlock { bytes, offset }, Vec::new())));
    }
    if n == 0
        || vertices < 3
        || vertices > 65_536
        || indices < 3
        || indices > 131_072 * 3
        || indices % 3 != 0
        || bytes.len() != offset + 8 + vertices * 12 + indices * 4
    {
        return Err("远景代理网格尺寸无效".into());
    }
    let mut rects = Vec::with_capacity(n);
    for raw in bytes[60..60 + n * 16].chunks_exact(16) {
        let r = std::array::from_fn::<_, 2, _>(|i| {
            std::array::from_fn::<_, 2, _>(|j| {
                i32::from_le_bytes(raw[i * 8 + j * 4..i * 8 + j * 4 + 4].try_into().unwrap())
            })
        });
        if (0..2).any(|i| r[0][i] >= r[1][i])
            || r.iter().flatten().any(|v| v.abs_diff(0) > 1_000_000)
        {
            return Err("远景代理覆盖范围无效".into());
        }
        rects.push(r);
    }
    Ok(Some((MeshBlock { bytes, offset }, rects)))
}
pub(super) fn coordinates(mut key: u64) -> [i32; 3] {
    let mut p = [0u32; 3];
    for bit in 0..21 {
        for a in &mut p {
            *a |= ((key & 1) as u32) << bit;
            key >>= 1;
        }
    }
    p.map(|v| v as i32 - 1_048_576)
}

/// 矩形并集按事件扫线合并，包含重复的上下层覆盖；绝不把断开区域的间隙裁掉。
pub(super) fn merge_rectangles(rects: Vec<GridRect>, limit: usize) -> Option<Vec<GridRect>> {
    let mut events: BTreeMap<i32, Vec<([i32; 2], i32)>> = BTreeMap::new();
    for r in rects {
        events
            .entry(r[0][0])
            .or_default()
            .push(([r[0][1], r[1][1]], 1));
        events
            .entry(r[1][0])
            .or_default()
            .push(([r[0][1], r[1][1]], -1));
    }
    let mut active: BTreeMap<[i32; 2], i32> = BTreeMap::new();
    let mut open: BTreeMap<[i32; 2], GridRect> = BTreeMap::new();
    let mut result = Vec::new();
    let mut events = events.into_iter().peekable();
    while let Some((x, changes)) = events.next() {
        for (z, delta) in changes {
            let n = active.entry(z).or_default();
            *n += delta;
            if *n == 0 {
                active.remove(&z);
            }
        }
        let Some(&(end, _)) = events.peek() else {
            result.extend(open.into_values());
            break;
        };
        let mut spans: Vec<[i32; 2]> = Vec::new();
        for &z in active.keys() {
            if let Some(last) = spans.last_mut()
                && z[0] <= last[1]
            {
                last[1] = last[1].max(z[1]);
            } else {
                spans.push(z);
            }
        }
        let mut next = BTreeMap::new();
        for z in spans {
            let mut r = open.remove(&z).unwrap_or([[x, z[0]], [x, z[1]]]);
            r[1][0] = end;
            next.insert(z, r);
        }
        result.extend(open.into_values());
        if result.len() > limit {
            return None;
        }
        open = next;
    }
    (result.len() <= limit).then_some(result)
}
