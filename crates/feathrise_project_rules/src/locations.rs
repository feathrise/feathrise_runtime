//! 用作者声明的三维区域发布进出地点事实。空间索引在工程启动时建立，移动只查询当前场景。

use crate::{ProjectVariables, types::Location};
use rstar::{
    RTree,
    primitives::{GeomWithData, Rectangle},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
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
    identity: Option<(u64, u64)>,
    position: [f32; 3],
    inside: Vec<usize>,
    hits: Vec<usize>,
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
        let identity = (observer, vars.generation.load(Ordering::Acquire));
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
            // 相邻地点在同一次位移中先离开、再进入；重叠区按稳定 ID 顺序产生事实。
            for (event, from, other) in [
                ("location.exited", &self.inside, &self.hits),
                ("location.entered", &self.hits, &self.inside),
            ] {
                for &id in from.iter().filter(|id| other.binary_search(id).is_err()) {
                    vars.event(event, &vars.book.locations.ids[id]);
                }
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

impl ProjectVariables {
    /// 空工程无须每帧查询观察者。
    pub fn has_locations(&self) -> bool {
        !self.book.locations.scenes.is_empty()
    }
}
