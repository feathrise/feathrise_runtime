//! 用作者声明的三维区域发布进出地点事实。空间索引在工程启动时建立，移动只查询当前场景。

use crate::{ProjectVariables, types::Location};
use rstar::{
    AABB, RTree,
    primitives::{GeomWithData, Rectangle},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Weak, atomic::AtomicU64},
};

type Area = GeomWithData<Rectangle<[f32; 3]>, usize>;

#[derive(Debug, Default)]
pub(crate) struct LocationIndex {
    ids: Vec<String>,
    scenes: HashMap<String, RTree<Area>>,
}

impl LocationIndex {
    pub fn build(locations: BTreeMap<String, Location>) -> Result<Self, String> {
        let mut ids = Vec::with_capacity(locations.len());
        let mut scenes: HashMap<String, Vec<Area>> = HashMap::new();
        for (id, location) in locations {
            if id.trim().is_empty() || location.scene.trim().is_empty() {
                return Err(format!("地点 {id:?} 需要稳定 ID 和所属场景"));
            }
            if (0..3).any(|i| {
                !location.min[i].is_finite()
                    || !location.max[i].is_finite()
                    || location.min[i] >= location.max[i]
            }) {
                return Err(format!("地点 {id} 的 XYZ 边界必须有限且每轴 min 小于 max"));
            }
            let area = Rectangle::from_corners(location.min, location.max);
            scenes
                .entry(location.scene)
                .or_default()
                .push(GeomWithData::new(area, ids.len()));
            ids.push(id);
        }
        Ok(Self {
            ids,
            scenes: scenes
                .into_iter()
                .map(|(id, areas)| (id, RTree::bulk_load(areas)))
                .collect(),
        })
    }
}

/// 每个本地观察者的边沿检测。首次生成、读档、换图和退出编辑态时先建立占用基线。
#[derive(Default)]
pub struct LocationTracker {
    scene: String,
    session: Weak<AtomicU64>,
    identity: Option<(u64, u64, u64)>,
    position: [f32; 3],
    inside: Vec<usize>,
    hits: Vec<usize>,
    crossings: Vec<(f64, bool, usize)>,
}

impl LocationTracker {
    pub fn clear(&mut self) {
        self.identity = None;
        self.inside.clear();
    }

    pub fn observe(
        &mut self,
        vars: &ProjectVariables,
        scene: &str,
        observer: u64,
        position: [f32; 3],
    ) {
        let Some(areas) = vars.book.locations.scenes.get(scene) else {
            self.clear();
            return;
        };
        if !position.iter().all(|p| p.is_finite()) {
            self.clear();
            return;
        }
        let (generation, scope) = vars.session_revision();
        let identity = (observer, generation, scope);
        let same = self.session.as_ptr() == Arc::as_ptr(&vars.generation)
            && self.identity == Some(identity)
            && self.scene == scene;
        if same && self.position == position {
            return;
        }
        self.hits.clear();
        self.hits
            .extend(areas.locate_all_at_point(position).map(|area| area.data));
        self.hits.sort_unstable();
        if same {
            // 空间索引先筛选位移包围盒，再检查线段，防止单步越过整个窄区域而漏报。
            self.crossings.clear();
            for area in
                areas.locate_in_envelope_intersecting(AABB::from_corners(self.position, position))
            {
                let before = self.inside.binary_search(&area.data).is_ok();
                let after = self.hits.binary_search(&area.data).is_ok();
                if before && after {
                    continue;
                }
                if let Some((enter, exit)) = segment_interval(self.position, position, area.geom())
                {
                    if !before && (after || enter < exit) {
                        self.crossings.push((enter, true, area.data));
                    }
                    if !after && (before || enter < exit) {
                        self.crossings.push((exit, false, area.data));
                    }
                }
            }
            // 沿位移顺序发布，同一点相邻区域先离开再进入，同类边沿按稳定 ID 排序。
            self.crossings.sort_unstable_by(|a, b| {
                a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
            });
            for &(_, enter, id) in &self.crossings {
                vars.event_in_instance(
                    if enter {
                        "location.entered"
                    } else {
                        "location.exited"
                    },
                    &vars.book.locations.ids[id],
                    scene,
                );
            }
        } else {
            scene.clone_into(&mut self.scene);
            self.session = Arc::downgrade(&vars.generation);
            self.identity = Some(identity);
        }
        std::mem::swap(&mut self.hits, &mut self.inside);
        self.position = position;
    }
}

/// 以双精度计算参数区间，有限 f32 端点相减也不会溢出。只擦过一个角不产生完整穿越。
fn segment_interval(
    from: [f32; 3],
    to: [f32; 3],
    area: &Rectangle<[f32; 3]>,
) -> Option<(f64, f64)> {
    let (min, max) = (area.lower(), area.upper());
    let (mut enter, mut exit) = (0.0_f64, 1.0_f64);
    for axis in 0..3 {
        let start = f64::from(from[axis]);
        let delta = f64::from(to[axis]) - start;
        let (lower, upper) = (f64::from(min[axis]), f64::from(max[axis]));
        if delta == 0. {
            if start < lower || start > upper {
                return None;
            }
        } else {
            let a = (lower - start) / delta;
            let b = (upper - start) / delta;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if enter > exit {
                return None;
            }
        }
    }
    Some((enter, exit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn crossing_in_one_step_publishes_ordered_edges_without_firing_on_load_or_tangent() {
        let vars = ProjectVariables::from_json(&json!({
            "version":1,
            "locations":{
                "a_far":{"scene":"town","min":[4,0,0],"max":[5,2,2]},
                "z_near":{"scene":"town","min":[0,0,0],"max":[1,2,2]}
            },
            "rules":[
                {"id":"in_near","when":{"event":"location.entered","target":"z_near"},"actions":[{"request":"edge","target":"near_in"}]},
                {"id":"out_near","when":{"event":"location.exited","target":"z_near"},"actions":[{"request":"edge","target":"near_out"}]},
                {"id":"in_far","when":{"event":"location.entered","target":"a_far"},"actions":[{"request":"edge","target":"far_in"}]},
                {"id":"out_far","when":{"event":"location.exited","target":"a_far"},"actions":[{"request":"edge","target":"far_out"}]}
            ]
        }).to_string()).unwrap();
        let mut tracker = LocationTracker::default();
        tracker.observe(&vars, "town", 1, [-1., 1., 1.]);
        tracker.observe(&vars, "town", 1, [6., 1., 1.]);
        let requests = vars.take_requests_with_instances("edge");
        assert_eq!(
            requests
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["near_in", "near_out", "far_in", "far_out"]
        );
        assert!(
            requests
                .iter()
                .all(|(_, instance)| instance.as_deref() == Some("town"))
        );
        tracker.observe(&vars, "town", 1, [-1., 1., 1.]);
        assert_eq!(
            vars.take_requests("edge"),
            ["far_in", "far_out", "near_in", "near_out"]
        );
        tracker.clear();
        tracker.observe(&vars, "town", 1, [-1., 1., 1.]);
        vars.restore(Default::default());
        tracker.observe(&vars, "town", 1, [6., 1., 1.]);
        assert!(vars.take_requests("edge").is_empty());

        // 同图传送经过加载态，驻留恢复可复用同一玩家；中间未观察也不能把瞬移当成路径。
        vars.set_active_scene(Some("town"));
        tracker.observe(&vars, "town", 1, [-1., 1., 1.]);
        vars.set_active_scene(None);
        vars.set_active_scene(Some("town"));
        tracker.observe(&vars, "town", 1, [6., 1., 1.]);
        assert!(vars.take_requests("edge").is_empty());

        tracker.clear();
        tracker.observe(&vars, "town", 1, [-1., 1., 1.]);
        tracker.observe(&vars, "town", 1, [1., -1., 1.]);
        assert!(vars.take_requests("edge").is_empty());

        let area = Rectangle::from_corners([0., 0., 0.], [1., 2., 2.]);
        assert_eq!(
            segment_interval([-1., 1., 1.], [1., -1., 1.], &area),
            Some((0.5, 0.5))
        );
        assert!(segment_interval([-1., 3., 1.], [2., 3., 1.], &area).is_none());
    }
}

impl ProjectVariables {
    /// 空工程无须每帧查询观察者。
    pub fn has_locations(&self) -> bool {
        !self.book.locations.scenes.is_empty()
    }
}
