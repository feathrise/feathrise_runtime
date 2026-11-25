//! 在 GLB 资产线程建立触发盒的空间索引。实例只查询观察者所在的分支。

use bevy::math::Vec3;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MotionRegionAction {
    #[default]
    None,
    Play,
    Reverse,
    Pause,
    Stop,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct MotionRegion {
    pub center: Vec3,
    pub half_extents: Vec3,
    pub enter: MotionRegionAction,
    pub exit: MotionRegionAction,
}

impl MotionRegion {
    pub(super) fn validate(self) -> Result<(), String> {
        if self.half_extents.min_element() <= 0.0
            || !self.half_extents.is_finite()
            || !(self.center - self.half_extents).is_finite()
            || !(self.center + self.half_extents).is_finite()
        {
            return Err("动画触发区域需要有限坐标和正尺寸".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Node {
    low: Vec3,
    high: Vec3,
    branch: Branch,
}

#[derive(Debug)]
enum Branch {
    Region(usize, usize),
    Children(Box<[Node; 2]>),
}

type Entry = (usize, usize, MotionRegion);

#[derive(Debug, Default)]
pub(super) struct Index(Option<Node>);

impl Index {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_none()
    }
    pub(super) fn build(mut entries: Vec<Entry>) -> Self {
        Self((!entries.is_empty()).then(|| Node::build(&mut entries)))
    }

    pub(super) fn visit(&self, point: Vec3, hits: &mut Vec<(usize, usize)>) {
        if let Some(root) = &self.0 {
            root.visit(point, hits);
        }
    }
}

impl Node {
    fn build(entries: &mut [Entry]) -> Self {
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for (_, _, area) in entries.iter() {
            low = low.min(area.center - area.half_extents);
            high = high.max(area.center + area.half_extents);
        }
        let branch = if let [(clip, region, _)] = entries {
            Branch::Region(*clip, *region)
        } else {
            let extent = high - low;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let mid = entries.len() / 2;
            entries
                .select_nth_unstable_by(mid, |a, b| a.2.center[axis].total_cmp(&b.2.center[axis]));
            let (left, right) = entries.split_at_mut(mid);
            Branch::Children(Box::new([Self::build(left), Self::build(right)]))
        };
        Self { low, high, branch }
    }

    fn visit(&self, point: Vec3, hits: &mut Vec<(usize, usize)>) {
        if !point.cmpge(self.low).all() || !point.cmple(self.high).all() {
            return;
        }
        match &self.branch {
            Branch::Region(clip, region) => hits.push((*clip, *region)),
            Branch::Children(children) => {
                for child in children.iter() {
                    child.visit(point, hits);
                }
            }
        }
    }
}
