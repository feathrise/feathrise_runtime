//! 三维地形请求、形状完成与安装事件组件。
use super::*;
#[derive(Component, Default)]
pub struct TerrainPatchCollisionRequest {
    pub(super) generation: u64,
}

/// 作者体积候选先准备真实形状，必须等高度面让位后才安装独立物理。
#[derive(Component)]
pub struct DeferredTerrainPreviewCollider;

/// 真实失败显式通知候选所有者；大地形不再靠固定总耗时取消来猜测后台是否失败。
#[derive(Component)]
pub struct TerrainPatchColliderFailure(pub(super) String);
impl TerrainPatchColliderFailure {
    pub fn detail(&self) -> &str {
        &self.0
    }
}

impl TerrainPatchColliderReady {
    pub(crate) fn matches(&self, request: &TerrainPatchCollisionRequest) -> bool {
        self.surface_ready && self.matches_geometry(request)
    }
    pub(crate) fn matches_geometry(&self, request: &TerrainPatchCollisionRequest) -> bool {
        self.generation == request.generation
    }
}

#[derive(Component, Clone)]
pub(crate) struct TerrainPatchColliderReady {
    pub(crate) collider: Collider,
    pub(super) physical: Collider, // 已烘焙世界缩放，物理实体自身固定单位缩放。
    pub(crate) scale: Vec3,
    pub(super) generation: u64,
    pub(crate) meshes: std::sync::Arc<[(Entity, Handle<Mesh>)]>, // 与碰撞同代的真实可视源，正式光追事务只读复用。
    pub(crate) surface_ready: bool, // 流送分块的材质也完整绑定后才能共同采用，避免逐帧出现白块。
}

#[derive(Component, Default)]
pub(in crate::shared::placement_data) struct TerrainMeshSources(pub(super) Vec<AssetId<Mesh>>);

#[derive(Component)]
pub(in crate::shared::placement_data) struct TerrainWorldReady;

#[derive(Component)]
pub(crate) struct InstallTerrainPatchCollider;

/// 已进入正式拓扑的根只由联合发布安装碰撞，热更完成不能单独替换物理。
#[derive(Component)]
pub(in crate::shared::placement_data) struct FormalTerrainPatchCollider;

#[derive(Component)]
/// 已烘焙世界缩放的独立物理根；共享LOD仅识别精确单位尺度采用，生命周期仍由地形域拥有。
pub(crate) struct TerrainPhysicsProxy;

/// 独立物理根不会被GLB热重建删除；作者根拥有其唯一生命期。
#[derive(Component)]
pub(in crate::shared::placement_data) struct TerrainColliderProxy(
    pub(in crate::shared::placement_data) Entity,
    pub(in crate::shared::placement_data) Mat4, // 上次已安装作者位姿；只变平移/朝向时复用同一后台形状。
);
