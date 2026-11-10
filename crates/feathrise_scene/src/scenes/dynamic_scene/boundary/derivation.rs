//! 空气墙分帧捕获、后台空间分块与不可变碰撞缓存；不按玩家距离卸载墙。

use super::helpers::{BoundaryColliderSpec, collider_spec};
use avian3d::prelude::Collider;
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_scene_content::{SceneBoundariesAsset, SceneBoundaryPath, SceneId};
use std::{collections::BTreeMap, sync::Arc};

const CAPTURE_POINTS: usize = 4096; // 单次Update只复制固定数量点，不在启动任务前深拷贝整图。
const CAPTURE_PATHS: usize = 64; // 空路径也占路径预算，避免空草稿数组绕过点预算。
const CHUNK_METERS: f32 = 128.0; // 空间局部compound供Avian静态宽相裁剪，全部场景角色共享。
const MAX_DERIVED_PARTS: usize = 2_000_000; // 长墙切分有独立内存保护，超限保留旧碰撞并报告错误。
pub(super) type ChunkKey = (i32, i32);

#[derive(Debug)]
pub(super) struct Chunk {
    pub specs: Vec<BoundaryColliderSpec>, // 几何精确比较，不用可能碰撞的hash决定复用。
    pub collider: Collider,               // 后台构造的共享compound形状，主线程只登记组件。
}

#[derive(Debug)]
struct Capture {
    generation: u64,              // 场景、目录、覆盖和资产事件共同推进的输入票据。
    document: SceneBoundariesAsset, // 仅含已复制部分；不会提前发布给物理。
    path: usize,
    point: usize,
}

#[derive(Debug)]
pub(super) struct Completed {
    pub generation: u64,
    pub scene_id: SceneId,
    pub result: Result<Arc<BTreeMap<ChunkKey, Arc<Chunk>>>, String>,
    pub document: Option<Arc<SceneBoundariesAsset>>, // 保留原始作者点，显示层不读取物理切分端点。
    pub segments: usize,
    pub formal: bool,                     // 本次输入来自文件还是未保存覆盖。
    pub validation_error: Option<String>, // 草稿预览和关闭安全校验共享同一后台输入。
    pub changed: Vec<ChunkKey>,           // 后台生成差异，主线程不再整图扫描缓存。
    pub removed: Vec<ChunkKey>,
}

impl Completed {
    pub fn is_current(&self, scene_id: Option<&SceneId>, generation: u64) -> bool {
        self.generation == generation && scene_id == Some(&self.scene_id) // 成功、错误和取消结果统一拒绝迟到身份。
    }
}

#[derive(Debug, Default)]
pub(super) struct Derivation {
    capture: Option<Capture>,
    task: Option<Task<Completed>>, // 过期任务仍占槽直到完成，禁止拖动时堆积CPU任务。
    pub document: Option<Arc<SceneBoundariesAsset>>, // 完整采用的作者源，包含停用但可见墙。
    pub installed: Arc<BTreeMap<ChunkKey, Arc<Chunk>>>,
    pub entities: BTreeMap<ChunkKey, Entity>,
}

impl Derivation {
    pub(super) fn background_busy(&self) -> bool { self.capture.is_some() || self.task.is_some() }
    pub(super) fn background_finished(&self) -> bool { self.task.as_ref().is_none_or(|task| task.is_finished()) }

    /// 只轮询一次；调用方必须在当前场景/代次匹配后采用。
    pub fn poll(&mut self) -> Option<Completed> {
        let completed = self.task.as_mut().and_then(check_ready)?;
        self.task = None; // 真正完成才归还single-flight槽。
        Some(completed)
    }

    pub fn clear_installed(&mut self) {
        self.capture = None;
        self.document = None;
        self.installed = Arc::default();
        self.entities.clear(); // 实体由场景根生命周期统一回收，后台任务保留至完成。
    }

    /// 文档仍归Assets/编辑器所有，分帧复制期间每帧先验证来源代次。
    pub fn request(
        &mut self,
        source: &SceneBoundariesAsset,
        scene_id: &SceneId,
        generation: u64,
        bounds: Rect,
        formal: bool,
    ) {
        if self.task.is_some() {
            return;
        } // 合并为最新目标，不排队每次鼠标输入。
        if self
            .capture
            .as_ref()
            .is_none_or(|capture| capture.generation != generation)
        {
            self.capture = Some(Capture {
                generation,
                document: SceneBoundariesAsset {
                    schema_version: source.schema_version,
                    scene_id: source.scene_id.clone(),
                    paths: Vec::new(),
                },
                path: 0,
                point: 0,
            });
        }
        let capture = self.capture.as_mut().expect("已建立捕获");
        let mut point_budget = CAPTURE_POINTS;
        let mut path_budget = CAPTURE_PATHS;
        while capture.path < source.paths.len() && point_budget > 0 && path_budget > 0 {
            let path = &source.paths[capture.path];
            if capture.point == 0 && capture.document.paths.len() == capture.path {
                capture.document.paths.push(SceneBoundaryPath {
                    id: path.id.clone(), // 字符串元数据只在路径开始复制一次。
                    label: path.label.clone(),
                    role: path.role,
                    enabled: path.enabled,
                    visible: path.visible,
                    points: Vec::new(), // 不分配最大容量，避免单条超长墙首帧集中初始化。
                    closed: path.closed,
                    bottom_y: path.bottom_y,
                    top_y: path.top_y,
                    thickness: path.thickness,
                });
            }
            let end = (capture.point + point_budget).min(path.points.len());
            capture.document.paths[capture.path]
                .points
                .extend_from_slice(&path.points[capture.point..end]);
            point_budget -= end - capture.point;
            capture.point = end;
            if end == path.points.len() {
                capture.path += 1;
                capture.point = 0;
                path_budget -= 1;
            }
        }
        if capture.path < source.paths.len() {
            return;
        }
        let capture = self.capture.take().expect("完整捕获");
        self.start(
            Arc::new(capture.document),
            scene_id.clone(),
            generation,
            bounds,
            formal,
        );
    }

    /// 编辑器已经发布不可变Arc草稿时直接共享，不进行第二次整图捕获。
    pub fn request_shared(
        &mut self,
        source: Arc<SceneBoundariesAsset>,
        scene_id: &SceneId,
        generation: u64,
        bounds: Rect,
    ) {
        if self.task.is_some() {
            return;
        }
        self.capture = None;
        self.start(source, scene_id.clone(), generation, bounds, false);
    }

    fn start(
        &mut self,
        document: Arc<SceneBoundariesAsset>,
        scene_id: SceneId,
        generation: u64,
        bounds: Rect,
        formal: bool,
    ) {
        let previous = Arc::clone(&self.installed); // 大型旧缓存零深拷贝交给worker比较。
        self.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            let validation = document.validate(&scene_id, bounds); // 关闭门只读后台结果，不在主线程遍历全部墙。
            let validation_error = validation.as_ref().err().cloned();
            let result = if formal {
                validation.and_then(|()| derive(&document, &previous))
            } else {
                derive(&document, &previous) // 开放草稿可缺点，但仍过滤非有限与退化线段。
            };
            let segments = result.as_ref().map_or(0, |chunks| {
                chunks.values().map(|chunk| chunk.specs.len()).sum()
            });
            let changed = result.as_ref().map_or_else(
                |_| Vec::new(),
                |chunks| {
                    chunks
                        .iter()
                        .filter_map(|(key, chunk)| {
                            (!previous.get(key).is_some_and(|old| Arc::ptr_eq(old, chunk)))
                                .then_some(*key)
                        })
                        .collect()
                },
            );
            let removed = result.as_ref().map_or_else(
                |_| Vec::new(),
                |chunks| {
                    previous
                        .keys()
                        .filter(|key| !chunks.contains_key(key))
                        .copied()
                        .collect()
                },
            );
            Completed {
                document: Some(document),
                generation,
                scene_id,
                result,
                segments,
                formal,
                validation_error,
                changed,
                removed,
            }
        }));
    }
}

/// 长墙按固定长度分段后归入中点所在空间块，避免一条横跨场景的compound包围盒。
fn derive(
    document: &SceneBoundariesAsset,
    previous: &BTreeMap<ChunkKey, Arc<Chunk>>,
) -> Result<Arc<BTreeMap<ChunkKey, Arc<Chunk>>>, String> {
    let mut grouped: BTreeMap<ChunkKey, Vec<BoundaryColliderSpec>> = BTreeMap::new();
    let mut count = 0usize;
    for segment in document.collision_segments() {
        let length = segment.start.distance(segment.end);
        if !length.is_finite() {
            continue;
        }
        let parts = (length / CHUNK_METERS).ceil().max(1.0) as usize;
        count = count.saturating_add(parts);
        if count > MAX_DERIVED_PARTS {
            return Err("空气墙派生段数超过安全内存预算，请缩小异常长墙".to_owned());
        }
        for part in 0..parts {
            let mut split = segment.clone();
            split.start = segment.start.lerp(segment.end, part as f32 / parts as f32);
            split.end = segment
                .start
                .lerp(segment.end, (part + 1) as f32 / parts as f32);
            let Some(mut spec) = collider_spec(&split) else {
                continue;
            };
            let position = spec.transform.translation;
            if !position.is_finite() {
                continue;
            }
            let key = (
                (position.x / CHUNK_METERS).floor() as i32,
                (position.z / CHUNK_METERS).floor() as i32,
            );
            spec.name.clear(); // chunk缓存由几何决定，改名不触发重建碰撞。
            grouped.entry(key).or_default().push(spec);
        }
    }
    let chunks = grouped
        .into_iter()
        .map(|(key, specs)| {
            if let Some(old) = previous.get(&key).filter(|old| old.specs == specs) {
                return (key, Arc::clone(old)); // 未变空间块保持同一Collider与实体。
            }
            let collider = Collider::compound(
                specs
                    .iter()
                    .map(|spec| {
                        (
                            spec.transform.translation,
                            spec.transform.rotation,
                            Collider::cuboid(spec.size.x, spec.size.y, spec.size.z),
                        )
                    })
                    .collect(),
            ); // Parry compound加速结构完全在后台构建。
            (key, Arc::new(Chunk { specs, collider }))
        })
        .collect();
    Ok(Arc::new(chunks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use feathrise_scene_content::{SCENE_SCHEMA_VERSION, SceneBoundaryRole};

    fn document() -> SceneBoundariesAsset {
        SceneBoundariesAsset {
            schema_version: SCENE_SCHEMA_VERSION,
            scene_id: SceneId::parse("MAP-wall-runtime").unwrap(),
            paths: vec![SceneBoundaryPath {
                id: "internal".to_owned(),
                label: "挡墙".to_owned(),
                role: SceneBoundaryRole::Internal,
                enabled: true,
                visible: true,
                closed: false,
                bottom_y: -10.0,
                top_y: 30.0,
                thickness: 2.0,
                points: vec![[0.0, 0.0], [512.0, 0.0]],
            }],
        }
    }

    #[test]
    fn long_internal_wall_uses_spatial_compounds_and_reuses_unchanged_geometry() {
        let mut doc = document();
        let first = derive(&doc, &BTreeMap::new()).unwrap();
        assert_eq!(first.len(), 4); // 长墙不会成为覆盖整个场景的单个宽相盒。
        assert!(
            first
                .values()
                .flat_map(|chunk| &chunk.specs)
                .all(|spec| spec.size.x <= CHUNK_METERS + 2.0)
        );
        doc.paths[0].label = "改名".to_owned();
        doc.paths[0].visible = false;
        let second = derive(&doc, &first).unwrap();
        assert!(
            first
                .iter()
                .all(|(key, chunk)| Arc::ptr_eq(chunk, &second[key]))
        ); // 改名/隐藏辅助线不重新派生碰撞。
        doc.paths[0].enabled = false;
        assert!(derive(&doc, &second).unwrap().is_empty()); // 只有停用碰撞才移除墙。
    }

    #[test]
    fn editing_one_internal_wall_preserves_distant_chunk_shape() {
        let mut doc = document();
        let mut other = doc.paths[0].clone();
        other.id = "second".to_owned();
        other.points = vec![[2048.0, 0.0], [2058.0, 0.0]];
        doc.paths.push(other);
        let first = derive(&doc, &BTreeMap::new()).unwrap();
        doc.paths[1].top_y = 90.0;
        let second = derive(&doc, &first).unwrap();
        assert!(Arc::ptr_eq(&first[&(0, 0)], &second[&(0, 0)]));
        assert!(!Arc::ptr_eq(&first[&(16, 0)], &second[&(16, 0)]));
    }

    #[test]
    fn capture_is_bounded_and_restarts_when_author_revision_changes() {
        let mut doc = document();
        doc.paths[0].points = (0..10_000).map(|index| [index as f32, 0.0]).collect();
        let mut state = Derivation::default();
        let bounds = Rect::new(-1.0, -1.0, 20_000.0, 20_000.0);
        state.request(&doc, &doc.scene_id, 1, bounds, false);
        assert_eq!(state.capture.as_ref().unwrap().point, CAPTURE_POINTS);
        assert!(state.task.is_none()); // 未捕获完整不能启动或发布半套墙。
        doc.paths[0].points[0][1] = 20.0;
        state.request(&doc, &doc.scene_id, 2, bounds, false);
        let capture = state.capture.as_ref().unwrap();
        assert_eq!(capture.generation, 2);
        assert_eq!(capture.point, CAPTURE_POINTS); // 新修订重启而不拼接旧点。
        assert_eq!(capture.document.paths[0].points[0][1], 20.0);
    }

    #[test]
    fn completed_failure_and_success_share_scene_and_generation_guard() {
        let doc = document();
        let mut completed = Completed {
            generation: 7,
            scene_id: doc.scene_id.clone(),
            result: Err("失败".to_owned()),
            segments: 0,
            formal: true,
            validation_error: None,
            document: None,
            changed: vec![],
            removed: vec![],
        };
        assert!(completed.is_current(Some(&doc.scene_id), 7));
        assert!(!completed.is_current(Some(&doc.scene_id), 8));
        assert!(!completed.is_current(None, 7));
        assert!(!completed.is_current(Some(&SceneId::parse("MAP-other").unwrap()), 7));
        completed.result = Ok(Arc::default());
        assert!(!completed.is_current(Some(&doc.scene_id), 8)); // 取消/退出后即使成功也不能发布。
    }

    #[test]
    fn leaving_scene_keeps_inflight_slot_until_worker_really_finishes() {
        let pool = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut state = Derivation::default();
        state.task = Some(pool.spawn(std::future::pending::<Completed>()));
        state.clear_installed();
        assert!(state.task.is_some()); // 退图不能靠drop句柄假装抢占CPU。
        assert!(state.background_busy());
        assert!(!state.background_finished()); // 子场景退役也须等真实worker结清，再后台释放结果。
        let doc = document();
        state.request(
            &doc,
            &doc.scene_id,
            8,
            Rect::new(-1000.0, -1000.0, 1000.0, 1000.0),
            false,
        );
        assert!(state.capture.is_none()); // 旧任务仍在途时连新快照也不积压。
        assert!(state.poll().is_none()); // 暂停未完成的worker不阻塞主线程。
        assert!(state.task.is_some());
    }
}
