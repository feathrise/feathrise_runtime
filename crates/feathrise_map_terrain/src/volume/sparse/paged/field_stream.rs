//! 原生表面字段的稀疏附近窗口：按距离优先读取不可变叶块，整对象作者数据从不截断。
use super::{
    fields::{MAX_FIELD_BYTES, VolumeFieldBytes},
    store::Store,
    types::*,
};
use bevy::math::Vec3;
use std::{cmp::Ordering, collections::BinaryHeap, path::Path};

const MAX_LEAVES: usize = 64;
const MAX_VISITS: usize = 4096;
const MAX_FIELD_WINDOW_BYTES: usize = 8 * 1024 * 1024;

struct Node {
    hash: Hash,
    depth: u32,
    lo: [i32; 3],
    hi: [i32; 3],
    distance: f32,
}
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance && self.lo == other.lo && self.depth == other.depth
    }
}
impl Eq for Node {}
impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| other.lo.cmp(&self.lo))
            .then_with(|| self.depth.cmp(&other.depth))
    }
}

/// 调用方在IO池执行；焦点应取实际地表/洞面附近的源坐标，不能只用高空相机的作者块。
/// 工作集受8MiB字节、64叶和4096目录节点共同约束；溢出的远块仍保留在作者库。
/// 返回limited供运行时明确显示本次字段窗口降级，不能把大地图总量作为资产加载失败。
pub fn read_native_surface_fields(
    root: &Path,
    volume: &PagedVolume,
    foci: &[[f32; 3]],
    scale: [f32; 3],
    radius: f32,
) -> Result<(VolumeFieldBytes, bool), String> {
    read_native_surface_fields_with_budget(
        root,
        volume,
        foci,
        scale,
        radius,
        MAX_FIELD_WINDOW_BYTES,
    )
}
/// GPU字段单次登记信用不足时，worker重取更小的近叶集合；不重写作者根。
pub fn read_native_surface_fields_with_budget(
    root: &Path,
    volume: &PagedVolume,
    foci: &[[f32; 3]],
    scale: [f32; 3],
    radius: f32,
    maximum_bytes: usize,
) -> Result<(VolumeFieldBytes, bool), String> {
    let maximum_bytes = maximum_bytes.clamp(MAX_FIELD_BYTES, MAX_FIELD_WINDOW_BYTES);
    volume.validate()?;
    let scale = Vec3::from_array(scale);
    if !radius.is_finite()
        || radius <= 0.0
        || !scale.is_finite()
        || scale.cmple(Vec3::ZERO).any()
        || foci.is_empty()
        || foci
            .iter()
            .any(|point| !Vec3::from_array(*point).is_finite())
    {
        return Err("原生三维字段窗口参数无效".into());
    }
    let foci: Vec<_> = foci.iter().take(2).copied().map(Vec3::from_array).collect();
    let extent = volume.extent();
    let distance = |lo: [i32; 3], hi: [i32; 3]| {
        let min = Vec3::from_array(lo.map(|value| value as f32)) * extent;
        let max = (Vec3::from_array(hi.map(|value| value as f32)) + Vec3::ONE) * extent;
        foci.iter()
            .map(|focus| ((focus.clamp(min, max) - *focus) * scale).length())
            .fold(f32::INFINITY, f32::min)
    };
    let mut queue = BinaryHeap::new();
    if volume.fields != EMPTY {
        queue.push(Node {
            hash: volume.fields,
            depth: 16,
            lo: [-1_048_576; 3],
            hi: [1_048_575; 3],
            distance: 0.0,
        });
    }
    let mut store = Store::display(root);
    let mut fields = Vec::new();
    let mut bytes = 0usize;
    let mut visits = 0usize;
    let mut limited = false;
    while let Some(node) = queue.pop() {
        if node.distance > radius {
            continue;
        }
        if visits >= MAX_VISITS || fields.len() >= MAX_LEAVES {
            limited = true;
            break;
        }
        visits += 1;
        if node.depth == 0 {
            let raw = store.read(node.hash, MAX_FIELD_BYTES)?;
            if bytes.saturating_add(raw.len()) > maximum_bytes {
                limited = true;
                break;
            }
            bytes += raw.len();
            fields.push((node.lo, raw));
            continue;
        }
        let shift = (node.depth - 1) * 4;
        for (slot, hash) in store.node(node.hash)?.into_iter().enumerate() {
            if hash == EMPTY || shift == 60 && slot >= 8 {
                continue;
            }
            let mut lo = node.lo;
            let mut hi = node.hi;
            for bit in 0..4 {
                let position = shift + bit;
                if position >= 63 {
                    continue;
                }
                let axis = (position % 3) as usize;
                let amount = 1i32 << (position / 3);
                if slot & (1 << bit) != 0 {
                    lo[axis] += amount;
                } else {
                    hi[axis] -= amount;
                }
            }
            let distance = distance(lo, hi);
            if distance <= radius {
                queue.push(Node {
                    hash,
                    depth: node.depth - 1,
                    lo,
                    hi,
                    distance,
                });
            }
        }
    }
    // 原生盒体没有冻结高度列；exposed_depth由其显式顶面合同取得，不扫描无关Clipmap列。
    Ok((
        VolumeFieldBytes {
            fields,
            heights: Vec::new(),
        },
        limited,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::tests::Temp;
    use super::*;
    #[test]
    fn fine_author_grid_reads_real_surface_leaves_and_has_a_fixed_window_budget() {
        let temp = Temp::new();
        let mut volume = PagedVolume::new(0.01).unwrap();
        let mut store = Store::new(&temp.0);
        for x in -80..80 {
            volume.fields = store
                .set(
                    volume.fields,
                    &LeafWrite {
                        key: key([x, 0, 0]).unwrap(),
                        bytes: b"VFS1{}".to_vec(),
                    },
                )
                .unwrap();
        }
        store.flush(&[volume.fields]).unwrap();
        // 相机可以在百米高空，调用方给到真正的盒体顶面焦点，细作者格依然命中。
        let (window, limited) =
            read_native_surface_fields(&temp.0, &volume, &[[0., 0., 0.]], [1.; 3], 64.).unwrap();
        assert!(limited);
        assert_eq!(window.fields.len(), MAX_LEAVES);
        assert!(window.fields.iter().any(|(key, _)| *key == [0, 0, 0]));
        assert!(window.fields.iter().all(|(key, _)| key[0].abs() < 40));
        assert_eq!(volume.cell_size, 0.01); // 派生窗口不修改作者根和精度。
    }
}
