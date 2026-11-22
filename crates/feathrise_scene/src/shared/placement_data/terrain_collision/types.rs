//! 分帧捕获和后台完成值，包含完整根代次与网格身份。
use super::*;

pub(super) type CollisionMode = feathrise_scene_content::MeshLodMode<feathrise_scene_content::MeshCollisionLodSettings>;

pub(super) struct CapturedMesh {
    pub(super) vertices: Vec<Vec3>,
    pub(super) indices: Vec<u32>,
    pub(super) matrix: Mat4,
    pub(super) group: Option<(Entity, CollisionMode)>,
}

pub(super) struct MeshCapture {
    pub(super) handle: Handle<Mesh>,
    pub(super) vertex_count: usize,
    pub(super) index_count: usize,
    pub(super) mesh: CapturedMesh,
}
pub(super) struct Capture {
    pub(super) root: Entity,
    pub(super) generation: u64,
    pub(super) inverse_root: Mat4,
    pub(super) scale: Vec3,
    pub(super) stack: VecDeque<Entity>,
    pub(super) current: Option<MeshCapture>,
    pub(super) meshes: Vec<CapturedMesh>,
    pub(super) sources: HashSet<AssetId<Mesh>>,
    pub(super) nodes: usize,
    pub(super) vertices: usize,
    pub(super) triangles: usize,
    pub(super) mesh_entities: Vec<(Entity, Handle<Mesh>)>,
    pub(super) scene_collision: bool, // 开启碰撞的普通完整GLB，不包含三维地形。
    pub(super) root_collision: bool,
    pub(super) partitioned: bool, // 只有带源节点碰撞设定的普通 GLB 才按节点划分。
    pub(super) excluded_mesh: bool, // 区分作者明确关闭全部网格与坏资产没有真实网格。
    pub(super) streamed: bool,
    pub(super) child_cursor: Option<(Entity, usize)>,
}
pub(super) struct Cooked {
    pub(super) inverse_root: Mat4,
    pub(super) scale: Vec3,
    pub(super) root: Entity,
    pub(super) generation: u64,
    pub(super) shape: Result<CookedShape, String>,
    pub(super) meshes: std::sync::Arc<[(Entity, Handle<Mesh>)]>,
}

pub(super) enum CookedShape {
    Single {
        collider: Collider, // 作者局部查询。
        physical: Collider, // 已缩放的同代物理。
        lod: Option<CollisionMode>,
    },
    Partitioned(VecDeque<(Collider, Option<CollisionMode>)>),
}

pub(super) struct PartitionInstall {
    pub(super) root: Entity,
    pub(super) generation: u64,
    pub(super) inverse_root: Mat4,
    pub(super) scale: Vec3,
    pub(super) body: Option<Entity>,
    pub(super) shapes: VecDeque<(Collider, Option<CollisionMode>)>,
}
