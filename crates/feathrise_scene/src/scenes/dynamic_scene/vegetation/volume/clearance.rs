//! 三维作者格的建筑净空：后台BVH只查询同一真实承托面，不跨洞顶清除植被。
use crate::{BuildingVegetationClearanceSnapshot, ClipmapGroundHit, ClipmapGroundSnapshot};
use bevy::prelude::*;

struct Node {
    bounds: Rect,
    start: usize,
    end: usize,
    children: Option<(usize, usize)>,
}
pub(super) struct Index {
    snapshot: BuildingVegetationClearanceSnapshot,
    order: Vec<usize>,
    nodes: Vec<Node>,
}
impl Index {
    /// 作者格数量和建筑数量都可增长，禁止逐格扫描整图；树只在源身份换代的计算任务里建立。
    pub(super) fn new(snapshot: BuildingVegetationClearanceSnapshot) -> Result<Self, String> {
        if snapshot.regions.len() > 8192
            || (!snapshot.floor_heights.is_empty()
                && snapshot.floor_heights.len() != snapshot.regions.len())
            || snapshot.floor_heights.iter().any(|h| !h.is_finite())
            || snapshot
                .regions
                .iter()
                .any(|r| r.corners.iter().any(|p| !p.is_finite()))
        {
            return Err("三维建筑净空数量或地基层数据无效".into());
        }
        let mut index = Self {
            order: (0..snapshot.regions.len()).collect(),
            snapshot,
            nodes: Vec::new(),
        };
        if !index.order.is_empty() {
            index.build_node(0, index.order.len());
        }
        Ok(index)
    }
    fn build_node(&mut self, start: usize, end: usize) -> usize {
        let mut bounds = self.snapshot.regions[self.order[start]].aabb();
        for &i in &self.order[start + 1..end] {
            let b = self.snapshot.regions[i].aabb();
            bounds.min = bounds.min.min(b.min);
            bounds.max = bounds.max.max(b.max);
        }
        let node = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            start,
            end,
            children: None,
        });
        if end - start > 8 {
            let axis = usize::from((bounds.max - bounds.min).y > (bounds.max - bounds.min).x);
            let regions = &self.snapshot.regions;
            self.order[start..end].sort_unstable_by(|&a, &b| {
                let x = regions[a].aabb();
                let y = regions[b].aabb();
                (x.min + x.max)[axis]
                    .total_cmp(&(y.min + y.max)[axis])
                    .then_with(|| a.cmp(&b))
            });
            let middle = (start + end) / 2;
            let left = self.build_node(start, middle);
            let right = self.build_node(middle, end);
            self.nodes[node].children = Some((left, right));
        }
        node
    }
    pub(super) fn excludes(
        &self,
        point: Vec3,
        cell_size: f32,
        ground: &ClipmapGroundSnapshot,
    ) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let mut stack = [0usize; 64];
        let mut count = 1;
        while count > 0 {
            count -= 1;
            let node = &self.nodes[stack[count]];
            if !node.bounds.contains(point.xz()) {
                continue;
            }
            if let Some((left, right)) = node.children {
                stack[count] = left;
                stack[count + 1] = right;
                count += 2;
                continue;
            }
            for &i in &self.order[node.start..node.end] {
                if !self.snapshot.regions[i].contains(point.xz()) {
                    continue;
                }
                let floor_y = self.snapshot.floor_heights.get(i).copied().or_else(|| {
                    // 旧二维接口明确只属于外表面；不得把未知层解释为整根垂直柱。
                    match ground.sample_authoritative_surface(point.xz()) {
                        ClipmapGroundHit::Hit(p) => Some(p.y),
                        _ => None,
                    }
                });
                let Some(floor_y) = floor_y else {
                    continue;
                };
                let from = Vec3::new(point.x, floor_y + 0.5, point.z);
                if ground
                    .three_dimensional_hit(from, Vec3::NEG_Y, 100000.)
                    .is_some_and(|(hit, n)| {
                        n.y > 0.05 && hit.distance(point) <= cell_size * 2.0 + 0.05
                    })
                {
                    return true; // 比较真实面而非根Y差，坡地仍生效，洞顶隔开的另一层仍保留。
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use feathrise_scene_content::BuildingWorldQuad;
    use std::sync::Arc;
    #[test]
    fn underground_clearance_does_not_cross_a_solid_roof() {
        let ground = ClipmapGroundSnapshot::stacked_for_test();
        let region = BuildingWorldQuad {
            corners: [
                Vec2::ZERO,
                Vec2::new(4., 0.),
                Vec2::splat(4.),
                Vec2::new(0., 4.),
            ],
        };
        let make = |floor_y| {
            Index::new(BuildingVegetationClearanceSnapshot {
                scene_id: ground.scene_id.clone(),
                revision: 1,
                regions: Arc::from(vec![region]),
                floor_heights: Arc::from(vec![floor_y]),
            })
            .unwrap()
        };
        assert!(make(2.).excludes(Vec3::new(1., 2., 1.), 0.1, &ground));
        assert!(!make(8.).excludes(Vec3::new(1., 2., 1.), 0.1, &ground));
        assert!(!make(2.).excludes(Vec3::new(6., 2., 1.), 0.1, &ground));
    }
}
