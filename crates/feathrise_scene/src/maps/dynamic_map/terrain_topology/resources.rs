//! 三维地形候选、正式洞口及光追集合的采用身份。
use super::*;

/// 每图共用PBR基底，每根独立绑定三维字段；只持有已有资产句柄，不复制像素或扫描作者数据。
#[derive(Resource, Default)]
pub(crate) struct VolumeSurfaceState {
    pub scope: Option<(MapId, Entity)>,
    pub source: Option<AssetId<feathrise_environment::terrain_surface::TerrainSurfaceMaterial>>,
    pub material: Option<Handle<feathrise_environment::terrain_surface::TerrainSurfaceMaterial>>,
    pub bindings: std::collections::HashMap<
        Entity,
        (
            Arc<[(Entity, Handle<Mesh>)]>,
            AssetId<feathrise_environment::terrain_surface::TerrainSurfaceMaterial>,
            usize,
        ),
    >, // 每根只保存同代Arc、材质身份与分帧游标；稳定帧不重新遍历所有显示块。
    // 最后一项记录已复制的基础材质身份；三维字段不变也必须跟随基础PBR颜色和渲染模式换代。
    pub fields: std::collections::HashMap<
        Entity,
        (
            usize,
            Handle<bevy::render::storage::ShaderBuffer>,
            Handle<feathrise_environment::terrain_surface::TerrainSurfaceMaterial>,
            AssetId<feathrise_environment::terrain_surface::TerrainSurfaceMaterial>,
        ),
    >,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerrainTopologySystems {
    Collect,
    Collision,
    Publish,
}

/// 仅F12局部体积原型提交；网格Ready及高度碰撞完成前不会采用。
#[derive(Resource, Default, Clone)]
pub struct TerrainVolumePreviewRequest {
    pub target: Option<TerrainVolumePreviewTarget>,
    pub(super) adopted: Option<Entity>,
    pub(super) adopted_removal: bool,
}
impl TerrainVolumePreviewRequest {
    /// 同一根的显示和撤销创建是两个联合版本，不能只用Entity当完成回执。
    pub fn is_adopted(&self, root: Entity, removal: bool) -> bool {
        self.adopted == Some(root) && self.adopted_removal == removal
    }
    pub fn adopted_root(&self) -> Option<Entity> {
        self.adopted
    }
}

#[derive(Clone)]
pub struct TerrainVolumePreviewTarget {
    pub map_id: MapId,
    pub entity: Entity,
    pub record: feathrise_map_content::MapPlacementRecord,
    pub world_cutout: MapTerrainCutout,
    pub removal: bool,
}

#[derive(Default)]
pub(super) struct VolumePreviewRuntime {
    pub active_proof: Option<TerrainMeshQuery>,
    pub active_holes: Arc<[MapTerrainCutout]>,
    pub base: Option<VolumeFormalBase>,
    pub pending: Option<VolumePreviewCandidate>,
    pub active_root: Option<Entity>,
    pub active_owned: bool,
    pub active_removal: bool,
    pub ignored_root: Option<Entity>, // 正式保存已接管同一预览时，不再借旧请求夺回。
    pub formal_key: Option<([u32; 2], u64)>,
    pub force_refresh: bool,
}
pub(super) struct VolumeFormalBase {
    pub holes: Arc<[MapTerrainCutout]>,
    pub surfaces: Arc<[TerrainMeshQuery]>,
    pub roots: Vec<Entity>,
}
pub(super) struct VolumePreviewCandidate {
    pub proof: Option<TerrainMeshQuery>, // 移除令牌也冻结形状，不能借同一矩形采用失效来源。
    pub root: Option<Entity>,            // None表示完整恢复正式集合。
    pub holes: Arc<[MapTerrainCutout]>,
    pub surfaces: Arc<[TerrainMeshQuery]>,
    pub roots: Vec<Entity>,
    pub batches: Vec<Arc<[(Entity, Handle<Mesh>)]>>,
    pub hidden_formal: Vec<Entity>, // 最多一个同ID的旧正式根；还原时重装其Ready形状。
    pub removal: bool,
    pub owned: bool,
}

/// 已联合采用的真实三维地形网格；预览和隐藏候选不取得光追准入证明。
#[derive(Resource, Default)]
pub struct StaticTerrainRaytracingSources {
    pub(super) revision: u64,
    pub(super) batches: Vec<Arc<[(Entity, Handle<Mesh>)]>>, // 至多32组，每组至多32项；无需重新扫描GLB层级。
}
impl StaticTerrainRaytracingSources {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn meshes(&self) -> impl Iterator<Item = &(Entity, Handle<Mesh>)> {
        self.batches.iter().flat_map(|batch| batch.iter())
    }
}

#[derive(Resource, Default)]
pub(crate) struct TerrainTopologyRuntime {
    pub(super) streamed_formal_ready: bool,
    pub(super) volume: VolumePreviewRuntime,
    pub(crate) map: Option<MapId>,
    pub(crate) root: Option<Entity>,
    pub(crate) desired: Arc<[MapTerrainCutout]>,
    pub(crate) applied: Arc<[MapTerrainCutout]>,
    pub(crate) revision: u64,
    pub(crate) surfaces: Arc<[TerrainMeshQuery]>,
    pub(super) active_roots: Vec<Entity>, // 至多32个已采用根，热更不扫描全图摆放。
    pub(super) candidate: Option<(
        [u32; 2],
        u64,
        Arc<[TerrainMeshQuery]>,
        Vec<Arc<[(Entity, Handle<Mesh>)]>>,
        Vec<Entity>,
    )>,
    pub(super) applied_candidate: Option<([u32; 2], u64)>,
}

impl TerrainTopologyRuntime {
    pub(super) fn retire_candidate(
        &mut self,
        garbage: Option<&mut crate::shared::placement_data::TerrainCollisionGarbage>,
    ) {
        if let Some(garbage) = garbage
            && let Some(candidate) = self.candidate.as_ref()
        {
            for surface in candidate.2.iter() {
                garbage.retain_shape(&surface.shape);
            }
        } // 根已移除且前一释放任务已完成时，候选查询可能是旧BVH的最后引用。
        self.candidate = None;
    }

    pub(crate) fn adopt(&mut self, holes: Arc<[MapTerrainCutout]>) {
        if self.applied != holes {
            self.applied = holes;
            self.revision = self.revision.wrapping_add(1);
        }
    }
}
