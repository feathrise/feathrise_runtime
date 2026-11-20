//! 从工程 World 分帧取出空间对象的身份，后台生成场景树的完整只读快照。

use bevy::{
    camera::RenderTarget,
    ecs::{
        archetype::Archetype, component::ComponentId, entity::EntityHashMap,
        entity_disabling::Disabled,
    },
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_atmosphere::{WorldCelestialLight, time::components::CelestialVisual};
use feathrise_editor_protocol::{MAX_PREVIEW_BYTES, Message, Mode, PreviewKind};
use feathrise_scene::{
    MeshLodAuxiliary, ScenePlacementInstance, SubsceneInstance, TerrainObjectInstance,
};
use feathrise_scene_core::view::PrimarySceneView;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, LinkedList},
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

use super::{EmbeddedProjectState, preview_toolbar::ToolbarCamera, transport::Transport};

// 作为可选实时预览格式使用现有 IPC 正文，公开编辑器仍可独立使用当前已发布的协议库。
pub(super) const CAPABILITY: &str = "scene-tree-v1";
pub(super) const REQUEST: &str = "feathrise-scene-tree-request-v1";
const RESPONSE: &str = "feathrise-scene-tree-v1";
pub(super) const EMPTY_SCENE: &str = "__empty_world__";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Query {
    revision: u64,
    known: Option<u64>,
}

pub(super) struct Request {
    pub id: u64,
    pub generation: u64,
    pub scene_id: String,
    pub query: Query,
}

#[derive(Clone, Copy, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    MainCamera,
    Camera,
    Light,
    Scene,
    Mesh,
    Node,
}

#[derive(Hash, Serialize)]
struct SceneTreeNode {
    depth: u32,
    index: u32,
    name: Option<String>,
    kind: Kind,
}

#[derive(Serialize)]
struct TreeReply<'a> {
    world_id: u64,
    revision: u64,
    // 同版本可省略行表；无版本表示本次采样未能完成。
    version: Option<u64>,
    rows: Option<&'a [SceneTreeNode]>,
}

#[derive(Default)]
pub(super) struct SceneTreeCapture {
    latest: Option<Request>,
    work: Option<(Request, Work)>,
}

type Reply = Option<Arc<Vec<u8>>>;

enum Work {
    Scan(Scan),
    Encode(Task<Reply>),
    Reply(Reply),
}

#[derive(Clone, Copy)]
enum EntryKind {
    Object(Kind),
    Parent,
    Authored,
    Excluded,
}

struct Entry {
    parent: Option<Entity>,
    name: Option<String>,
    kind: EntryKind,
}

struct Scan {
    limit: usize,
    archetype: usize,
    row: usize,
    bytes: usize,
    // 每帧追加自己的采样块，跨帧不会搬动已收集的整表。
    chunks: LinkedList<Vec<(Entity, Entry)>>,
}

impl SceneTreeCapture {
    pub(super) fn request(&mut self, request: Request) {
        self.latest = Some(request);
    }

    pub(super) fn advance(
        &mut self,
        world: &World,
        transport: &mut Transport,
        available: bool,
    ) -> Result<(), String> {
        let state = world.resource::<EmbeddedProjectState>();
        let current = |request: &Request| {
            available
                && state.mode == Mode::Editing
                && !state.transitioning
                && request.generation == transport.identity.generation
                && request.query.revision == state.revision
        };
        let Some((request, work)) = self.work.take().or_else(|| {
            let request = self.latest.take()?;
            let work = if current(&request) {
                Work::Scan(Scan::new(world))
            } else {
                Work::Reply(None)
            };
            Some((request, work))
        }) else {
            return Ok(());
        };
        let valid = current(&request);
        let (world_id, revision, known) =
            (state.world_id, request.query.revision, request.query.known);
        let next = match work {
            Work::Scan(mut scan) => {
                match valid.then(|| scan.step(world)) {
                    Some(Ok(false)) => Work::Scan(scan),
                    result => Work::Encode(AsyncComputeTaskPool::get().spawn(async move {
                        // 过期采样与超额采样也在同一后台槽释放，不占用工程帧处理整表。
                        if result == Some(Ok(true)) {
                            encode(scan.into_entries(), world_id, revision, known)
                        } else {
                            None
                        }
                    })),
                }
            }
            Work::Encode(mut task) => match check_ready(&mut task) {
                Some(reply) if valid => Work::Reply(reply),
                Some(_) => Work::Reply(None),
                None => Work::Encode(task),
            },
            Work::Reply(reply) => {
                if request.generation != transport.identity.generation {
                    return Ok(());
                }
                let payload = reply.filter(|_| valid).unwrap_or_else(|| {
                    // 采样失败的固定小回执释放查询槽，旧层级仍由宿主持有。
                    Arc::new(
                        serde_json::to_vec(&TreeReply {
                            world_id,
                            revision,
                            version: None,
                            rows: None,
                        })
                        .expect("固定场景树回执可序列化"),
                    )
                });
                if transport.try_send(
                    Message::Preview {
                        kind: PreviewKind::AuthorDomain,
                        sequence: request.id,
                        scene_id: request.scene_id.clone(),
                        format: RESPONSE.into(),
                    },
                    Some(payload.clone()),
                )? {
                    return Ok(());
                }
                Work::Reply(Some(payload))
            }
        };
        self.work = Some((request, next));
        Ok(())
    }
}

impl Scan {
    fn new(world: &World) -> Self {
        Self {
            limit: world.archetypes().len(),
            archetype: 0,
            row: 0,
            bytes: 0,
            chunks: LinkedList::new(),
        }
    }

    fn into_entries(self) -> EntityHashMap<Entry> {
        // 实体跨帧换原型后可能再次被采样，后台按采样顺序采用最后一条记录。
        self.chunks.into_iter().flatten().collect()
    }

    fn step(&mut self, world: &World) -> Result<bool, ()> {
        // 这里只读取 ECS；排序、层级修剪和编码都交给后台。按实时时间让出帧，不限制对象数量。
        let deadline = Instant::now() + Duration::from_micros(250);
        let mut chunk = Vec::new();
        'frame: while self.archetype < self.limit && Instant::now() < deadline {
            let archetype = world.archetypes().iter().nth(self.archetype).unwrap();
            // ECS 保留空原型，重复刷新时可直接跳过这部分组件类型查询。
            if !archetype.is_empty() && let Some(kind) = classify(world, archetype) {
                while let Some(row) = archetype.entities().get(self.row) {
                    if Instant::now() >= deadline {
                        break 'frame;
                    }
                    self.row += 1;
                    let entity = world.entity(row.id());
                    // 离屏相机不占行，其下的有效场景对象仍挂到最近的可见父级。
                    let kind = if matches!(kind, EntryKind::Object(Kind::Camera))
                        && entity.get::<RenderTarget>().is_some_and(|target| !matches!(target, RenderTarget::Window(_)))
                    {
                        if !entity.contains::<Children>() { continue; }
                        EntryKind::Parent
                    } else {
                        kind
                    };
                    let name = match kind {
                        EntryKind::Object(kind) => entity.get::<Name>()
                            .map(Name::as_str)
                            .filter(|name| !name.trim().is_empty())
                            .or_else(|| {
                                if !matches!(kind, Kind::Scene) { return None; }
                                // 未命名的模型实例沿用资产文件名，使导入对象仍有可识别的一行。
                                let path = entity.get::<WorldAssetRoot>().and_then(|root| root.0.path())
                                    .or_else(|| entity.get::<DynamicWorldRoot>().and_then(|root| root.0.path()))?;
                                path.path().file_name()?.to_str()
                            }),
                        _ => None,
                    };
                    // 同一 IPC 字节预算也约束采样内存，超额时保留编辑器上次的完整层级。
                    self.bytes += size_of::<(Entity, Entry)>() + name.map_or(0, str::len);
                    if self.bytes > MAX_PREVIEW_BYTES {
                        self.chunks.push_back(chunk);
                        return Err(());
                    }
                    chunk.push((
                        row.id(),
                        Entry {
                            parent: entity.get::<ChildOf>().map(ChildOf::parent),
                            name: name.map(str::to_owned),
                            kind,
                        },
                    ));
                }
            }
            self.archetype += 1;
            self.row = 0;
        }
        if !chunk.is_empty() {
            self.chunks.push_back(chunk);
        }
        Ok(self.archetype == self.limit)
    }
}

fn classify(world: &World, archetype: &Archetype) -> Option<EntryKind> {
    let has = |id: Option<ComponentId>| id.is_some_and(|id| archetype.contains(id));
    macro_rules! has {
        ($component:ty) => {
            has(world.component_id::<$component>())
        };
    }
    let parent = has!(Children);
    // 日月由环境设置控制，派生的方向光和视觉层都不作为独立工程对象。
    if has!(Disabled) || has!(Node) || has!(ToolbarCamera) || has!(MeshLodAuxiliary)
        || has!(WorldCelestialLight) || has!(CelestialVisual)
    {
        return parent.then_some(EntryKind::Excluded);
    }
    if has!(ScenePlacementInstance) || has!(SubsceneInstance) || has!(TerrainObjectInstance) {
        return parent.then_some(EntryKind::Authored);
    }
    let named = has!(Name);
    let kind = if has!(Camera) {
        if has!(PrimarySceneView) {
            Kind::MainCamera
        } else {
            Kind::Camera
        }
    } else if named && (has!(DirectionalLight) || has!(PointLight) || has!(SpotLight)) {
        Kind::Light
    } else if has!(WorldAssetRoot) || has!(DynamicWorldRoot) {
        Kind::Scene
    } else if named && (has!(Mesh3d) || has!(Mesh2d) || has!(Sprite) || has!(Text2d)) {
        Kind::Mesh
    } else if parent && named && has!(Transform) {
        Kind::Node
    } else {
        return parent.then_some(EntryKind::Parent);
    };
    Some(EntryKind::Object(kind))
}

fn project(entries: EntityHashMap<Entry>) -> Vec<SceneTreeNode> {
    let mut children = HashMap::<Option<Entity>, Vec<Entity>>::new();
    for (&id, entry) in &entries {
        children
            .entry(entry.parent.filter(|parent| entries.contains_key(parent)))
            .or_default()
            .push(id);
    }
    for siblings in children.values_mut() {
        siblings.sort_unstable_by(|left, right| {
            let rank = |entry: &Entry| match entry.kind {
                EntryKind::Object(kind) => kind as u8,
                _ => Kind::Node as u8,
            };
            let a = &entries[left];
            let b = &entries[right];
            (rank(a), &a.name, left).cmp(&(rank(b), &b.name, right))
        });
    }

    enum Visit {
        Enter(Entity, u32, bool),
        EndGroup(usize),
    }
    let mut todo = Vec::new();
    if let Some(roots) = children.remove(&None) {
        todo.extend(roots.into_iter().rev().map(|id| Visit::Enter(id, 0, false)));
    }
    let mut rows = Vec::new();
    let mut entries = entries;
    while let Some(visit) = todo.pop() {
        let (id, depth, covered) = match visit {
            Visit::Enter(id, depth, covered) => (id, depth, covered),
            Visit::EndGroup(start) => {
                // 先判断真实可列出的子项，避免只露出 SDK 摆放候选的管理分组。
                if rows.len() == start + 1 { rows.pop(); }
                continue;
            }
        };
        let entry = entries.remove(&id).unwrap();
        if matches!(entry.kind, EntryKind::Excluded) { continue; }
        let covered = covered || matches!(entry.kind, EntryKind::Authored);
        // 模型实例代表整份导入内容。保留窗口相机，跳过导入网格、骨骼等内部节点。
        let children_covered = covered || matches!(entry.kind, EntryKind::Object(Kind::Scene));
        let mut child_depth = depth;
        if let EntryKind::Object(kind) = entry.kind
            && (matches!(kind, Kind::MainCamera | Kind::Camera) || !covered && entry.name.is_some())
        {
            if matches!(kind, Kind::Node) { todo.push(Visit::EndGroup(rows.len())); }
            rows.push(SceneTreeNode { depth, index: id.index_u32(), name: entry.name, kind });
            child_depth += 1;
        }
        // 无名中间节点不占行，子对象挂到最近的可见上层。
        if let Some(children) = children.remove(&Some(id)) {
            todo.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|id| Visit::Enter(id, child_depth, children_covered)),
            );
        }
    }
    rows
}

fn encode(
    entries: EntityHashMap<Entry>,
    world_id: u64,
    revision: u64,
    known: Option<u64>,
) -> Reply {
    let rows = project(entries);
    let mut hash = DefaultHasher::new();
    rows.hash(&mut hash);
    let version = Some(hash.finish());
    let reply = TreeReply {
        world_id,
        revision,
        version,
        rows: (version != known).then_some(&rows),
    };
    // JSON 转义后的字节数由最终正文决定，限定写入缓冲，不发布截断的层级。
    #[derive(Default)]
    struct Body(Vec<u8>);
    impl std::io::Write for Body {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_PREVIEW_BYTES - self.0.len() {
                return Err(std::io::ErrorKind::OutOfMemory.into());
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut body = Body::default();
    serde_json::to_writer(&mut body, &reply)
        .ok()
        .map(|()| Arc::new(body.0))
}


#[cfg(test)]
mod tests {
    use super::*;

    fn collect(world: &World) -> EntityHashMap<Entry> {
        let mut scan = Scan::new(world);
        while !scan.step(world).unwrap() {}
        scan.into_entries()
    }

    #[test]
    fn scene_tree_uses_latest_sample_and_sorts_equal_names_by_entity() {
        let mut world = World::new();
        let group = world.spawn_empty().id();
        let mut siblings = [world.spawn_empty().id(), world.spawn_empty().id()];
        siblings.sort_unstable();
        let [first, second] = siblings;
        let mesh = |parent| Entry {
            parent,
            name: Some("同名模型".into()),
            kind: EntryKind::Object(Kind::Mesh),
        };
        let mut scan = Scan::new(&world);
        // 父项晚于子项被采样，同名对象的顺序仍按实体区分。
        scan.chunks
            .push_back(vec![(second, mesh(Some(group))), (first, mesh(None))]);
        scan.chunks.push_back(vec![
            (first, mesh(Some(group))),
            (
                group,
                Entry {
                    parent: None,
                    name: Some("分组".into()),
                    kind: EntryKind::Object(Kind::Node),
                },
            ),
        ]);
        let rows = project(scan.into_entries());
        assert_eq!(
            rows.iter()
                .map(|row| (row.index, row.depth))
                .collect::<Vec<_>>(),
            [
                (group.index_u32(), 0),
                (first.index_u32(), 1),
                (second.index_u32(), 1),
            ],
        );
    }

    #[test]
    fn scene_tree_samples_native_hierarchy_and_tracks_changes() {
        let mut world = World::new();
        let unnamed_parent = world.spawn_empty().id();
        let group = world.spawn((Name::new("角色组"), Transform::default(), ChildOf(unnamed_parent))).id();
        let offscreen = world.spawn((
            Name::new("离屏烘焙"),
            Camera3d::default(),
            RenderTarget::None { size: UVec2::ONE },
            ChildOf(group),
        )).id();
        let model = world
            .spawn((Name::new("角色模型"), Mesh3d(default()), ChildOf(offscreen)))
            .id();
        let light = world.spawn((Name::new("命名灯光"), PointLight::default(), ChildOf(model))).id();
        let current_camera = world.spawn((Camera3d::default(), PrimarySceneView)).id();
        world.spawn(Mesh3d(default()));
        world.spawn((Name::new("辅助变换"), Transform::default()));
        let staging_group = world.spawn((Name::new("候选分组"), Transform::default())).id();
        world.spawn((Mesh3d(default()), ChildOf(staging_group)));
        world.spawn(ToolbarCamera);
        let ui = world.spawn(Node::default()).id();
        world.spawn((Name::new("界面子项"), Mesh3d(default()), ChildOf(ui)));
        world.spawn((Name::new("LOD 辅助"), Mesh3d(default()), MeshLodAuxiliary));

        let rows = project(collect(&world));
        assert_eq!(
            rows.iter()
                .map(|row| (row.index, row.depth))
                .collect::<Vec<_>>(),
            [
                (current_camera.index_u32(), 0),
                (group.index_u32(), 0),
                (model.index_u32(), 1),
                (light.index_u32(), 2),
            ]
        );
        world.despawn(light);
        world
            .entity_mut(model)
            .insert((Name::new("已改名"), ChildOf(current_camera)));
        let rows = project(collect(&world));
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[1].depth, rows[1].name.as_deref()),
            (1, Some("已改名"))
        );
    }

    #[test]
    fn scene_tree_keeps_one_row_per_imported_model_and_preserves_window_cameras() {
        use feathrise_scene_content::{SceneId, ScenePlacementRecord};
        let mut world = World::new();
        let placement: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
            "id": "placed", "translation": [0., 0., 0.], "yaw_degrees": 0., "scale": [1., 1., 1.]
        }))
        .unwrap();
        let authored = world
            .spawn(ScenePlacementInstance::new(
                &SceneId("fixture".into()),
                &placement,
            ))
            .id();
        let visual = world.spawn((Mesh3d(default()), ChildOf(authored))).id();
        let camera = world.spawn((Camera3d::default(), ChildOf(visual))).id();
        let instance = world.spawn((Name::new("外部模型"), WorldAssetRoot(default()))).id();
        let bone = world.spawn((Name::new("模型骨骼"), Transform::default(), ChildOf(instance))).id();
        world.spawn((Name::new("内部网格"), Mesh3d(default()), ChildOf(bone)));
        let body = encode(collect(&world), 7, 3, None).unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let rows = reply["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["index"], instance.index_u32());
        assert_eq!(rows[0]["depth"], 0);
        assert_eq!(rows[1]["index"], camera.index_u32());
        assert_eq!(rows[1]["depth"], 0);
        let known = reply["version"].as_u64();
        let unchanged = encode(collect(&world), 7, 3, known).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&unchanged).unwrap()["rows"].is_null());
    }
}
