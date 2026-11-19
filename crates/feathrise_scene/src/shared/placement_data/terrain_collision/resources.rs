//! 三维地形单飞任务、根登记及旧BVH有界释放。
use super::*;
#[derive(Resource, Default)]
pub(in crate::shared::placement_data) struct TerrainCollisionRuntime {
    pub(super) registered: BTreeSet<Entity>, // 只登记请求碰撞的根，游标可跨帧继续且不复制全表。
    pub(super) capture_cursor: Option<Entity>,
    pub(super) pending_mesh_changes: HashSet<AssetId<Mesh>>,
    pub(super) mesh_sweep: Option<MeshChangeSweep>,
    pub(super) started: Option<(Entity, u64, Instant)>,
    pub(super) capture: Option<Capture>,
    pub(super) task: Option<Task<Cooked>>,
    pub(super) release: Option<Task<()>>,
    pub(super) failed: HashMap<Entity, u64>,
}

pub(super) struct MeshChangeSweep {
    pub(super) changed: HashSet<AssetId<Mesh>>,
    pub(super) cursor: Option<Entity>,
}

/// 旧形状引用移出帧关键路径；主线程与地表快照都完成切换后由同一个释放槽析构。
#[derive(Resource, Default)]
pub(crate) struct TerrainCollisionGarbage(pub(super) HashMap<usize, Collider>);
impl TerrainCollisionGarbage {
    pub(crate) fn retain_shape(&mut self, shape: &Collider) {
        let key = std::ptr::from_ref(shape.shape_scaled().as_ref()).cast::<()>() as usize;
        self.0.entry(key).or_insert_with(|| shape.clone()); // 缩放BVH可能与原BVH独立，不能按共同原形状误去重。
    }
}
