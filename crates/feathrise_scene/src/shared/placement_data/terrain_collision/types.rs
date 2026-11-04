//! 分帧捕获和后台完成值，包含完整根代次与网格身份。
use super::*;
pub(super) struct MeshCapture {
    pub(super) handle: Handle<Mesh>,
    pub(super) matrix: Mat4,
    pub(super) vertex_count: usize,
    pub(super) index_count: usize,
    pub(super) vertices: Vec<Vec3>,
    pub(super) indices: Vec<u32>,
}
pub(super) struct Capture {
    pub(super) root: Entity,
    pub(super) generation: u64,
    pub(super) inverse_root: Mat4,
    pub(super) scale: Vec3,
    pub(super) stack: VecDeque<Entity>,
    pub(super) current: Option<MeshCapture>,
    pub(super) meshes: Vec<(Vec<Vec3>, Vec<u32>)>,
    pub(super) sources: Vec<AssetId<Mesh>>,
    pub(super) nodes: usize,
    pub(super) vertices: usize,
    pub(super) triangles: usize,
    pub(super) matrices: Vec<Mat4>,
    pub(super) mesh_entities: Vec<(Entity, Handle<Mesh>)>,
    pub(super) streamed: bool,
    pub(super) child_cursor: Option<(Entity, usize)>,
}
pub(super) struct Cooked {
    pub(super) inverse_root: Mat4,
    pub(super) scale: Vec3,
    pub(super) root: Entity,
    pub(super) generation: u64,
    pub(super) shape: Result<(Collider, Collider), String>, // 作者局部查询、已缩放物理，同次后台结果。
    pub(super) meshes: std::sync::Arc<[(Entity, Handle<Mesh>)]>,
}
