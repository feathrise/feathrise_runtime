//! 只冻结单对象的真实表面和源坐标桥；调用方有界捕获后在后台完成跨对象排序。
use avian3d::prelude::Collider;
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_content::{SceneContentRegistry, SceneId, TerrainId};
use feathrise_scene_core::residency::{InstanceResourceTable, SceneSourceFrame};
use std::ops::Bound::{Excluded, Unbounded};
use super::{instances::{SubsceneActive, SubsceneInstance, TerrainObjectInstance},
    clipmap::{DynamicClipmapGroundSurface, ClipmapGroundSnapshot},
    voxel_objects::VoxelObjectRuntime};

#[derive(Clone)]
pub enum TerrainObjectRaySnapshot {
    Clipmap(ClipmapGroundSnapshot, SceneSourceFrame),
    Mesh(Collider, SceneSourceFrame),
}
impl TerrainObjectRaySnapshot {
    /// 作者载体分类只针对同次最近命中；公共表面可命中并不等于需要三维字段。
    pub fn ray_hit_is_three_dimensional(&self, origin: Vec3, direction: Vec3, point: Vec3) -> bool {
        match self {
            // 直接读取公共最近面选择的来源；后方即使只差半毫米也不能获得三维资格。
            // 查询仍在原单飞后台槽，局部最大交距与公共法线查询使用相同缩放。
            Self::Clipmap(ground, frame) => {
                let Some(direction) = direction.try_normalize() else { return false; };
                let local_origin = frame.to_local(origin);
                let local_direction = frame.world_to_source.transform_vector3(direction);
                let scale = local_direction.length();
                ground.ray_surface_with_kind(local_origin, local_direction / scale, 1_000_000.0 * scale)
                    .is_some_and(|(surface, _, volume)| volume && frame.to_world(surface).distance(point) < 0.05)
            }
            Self::Mesh(..) => self.ray_surface(origin, direction, 1_000_000.0)
                .is_some_and(|(surface, _)| surface.distance(point) < 0.05),
        }
    }
    /// 字段投射也使用真实三维面与逆转置法线；调用方在worker中执行密集射线。
    pub fn ray_surface(&self, origin: Vec3, direction: Vec3, maximum: f32) -> Option<(Vec3,Vec3)> {
        let direction = direction.try_normalize()?;
        match self {
            Self::Mesh(collider, frame) => {
                let query = super::terrain_topology::TerrainMeshQuery::new(collider.clone(), frame.source_to_world).ok()?;
                query.ray_normal(origin, direction, maximum).map(|(distance,normal)| (origin+direction*distance,normal))
            }
            Self::Clipmap(ground, frame) => {
                let local_direction = frame.world_to_source.transform_vector3(direction);
                let scale = local_direction.length();
                let (point,normal) = ground.ray_surface(frame.to_local(origin), local_direction/scale, maximum*scale)?;
                Some((frame.to_world(point), frame.world_to_source.transpose().transform_vector3(normal).try_normalize()?))
            }
        }
    }
    /// 垂直承托只接受朝上的真实面；对象缩放时法线按逆转置矩阵换回世界。
    pub fn support_below(&self, origin: Vec3, distance: f32) -> Option<Vec3> {
        match self.support_below_status(origin, distance) { super::clipmap::ClipmapGroundHit::Hit(point) => Some(point), _ => None }
    }
    pub fn support_below_status(&self, origin: Vec3, distance: f32) -> super::clipmap::ClipmapGroundHit {
        use super::clipmap::ClipmapGroundHit;
        match self {
            Self::Clipmap(ground, frame) => {
                if !source_up_is_world_up(*frame) {
                    return ground.support_below_in_frame(origin, distance, *frame);
                }
                let local = frame.to_local(origin);
                let scale = frame.world_to_source.transform_vector3(Vec3::NEG_Y).length();
                match ground.sample_support_below(local, distance * scale) {
                    ClipmapGroundHit::Hit(point) => ClipmapGroundHit::Hit(frame.to_world(point)),
                    status => status,
                }
            }
            Self::Mesh(collider, frame) => {
                let Ok(query) = super::terrain_topology::TerrainMeshQuery::new(collider.clone(), frame.source_to_world) else { return ClipmapGroundHit::Failed; };
                match query.ray_normal(origin,Vec3::NEG_Y,distance) {
                    Some((distance,normal)) if normal.y > 0.05 => ClipmapGroundHit::Hit(origin + Vec3::NEG_Y*distance),
                    _ => ClipmapGroundHit::Miss,
                }
            }
        }
    }
    /// 【功能设计】网格吸附沿实际对象的原承托层移动；输入与返回均为世界坐标。
    /// 只在调用方唯一后台查询槽执行，共享原BVH并仅收集XZ线段穿越的真实三角面。
    pub fn sample_support_along_surface(&self, point: Vec3, target_xz: Vec2) -> super::clipmap::ClipmapGroundHit {
        use super::clipmap::ClipmapGroundHit;
        if !point.is_finite() || !target_xz.is_finite() {
            return ClipmapGroundHit::Failed;
        }
        match self {
            Self::Mesh(collider, frame) => {
                let Ok(query) = super::terrain_topology::TerrainMeshQuery::new(collider.clone(), frame.source_to_world) else {
                    return ClipmapGroundHit::Failed;
                };
                let mut triangles = Vec::new();
                query.append_surface_triangles(point.xz(), target_xz, &mut triangles);
                feathrise_terrain::follow_terrain_surface(point, target_xz, triangles)
                    .map_or(ClipmapGroundHit::Miss, ClipmapGroundHit::Hit)
            }
            Self::Clipmap(ground, frame) => {
                if !source_up_is_world_up(*frame) {
                    return ground.support_along_surface_in_frame(point, target_xz, *frame);
                }
                // 水平对象沿用源XZ快路径；倾斜对象必须在完整世界三角面上找目标高度。
                let local = frame.to_local(point);
                let local_target = frame.to_local(Vec3::new(target_xz.x, point.y, target_xz.y));
                match ground.sample_support_along_surface(local, local_target.xz()) {
                    ClipmapGroundHit::Hit(hit) => ClipmapGroundHit::Hit(frame.to_world(hit)),
                    status => status,
                }
            }
        }
    }
    /// 真实三角范围只供后台索引构建；不按源码初始高度截断洞底或边界体素盖印。
    pub fn mesh_bounds(&self) -> Option<(Vec3, Vec3)> {
        match self {
            Self::Mesh(shape,frame) => super::terrain_topology::TerrainMeshQuery::new(shape.clone(),frame.source_to_world).ok().map(|query|query.bounds()),
            Self::Clipmap(ground,frame) => ground.surface_bounds().map(|(min,max)| {
                let center=frame.to_world((min+max)*0.5); let half=(max-min)*0.5;
                let matrix=frame.source_to_world; let world_half=matrix.transform_vector3(Vec3::X).abs()*half.x
                    +matrix.transform_vector3(Vec3::Y).abs()*half.y+matrix.transform_vector3(Vec3::Z).abs()*half.z;
                (center-world_half,center+world_half)
            }),
        }
    }
    pub fn ray(&self, origin: Vec3, direction: Vec3) -> Option<Vec3> {
        match self {
            Self::Clipmap(ground, frame) => {
                let local_origin = frame.to_local(origin);
                let local_direction = frame.world_to_source.transform_vector3(direction).try_normalize()?;
                match ground.ray_intersection(local_origin, local_direction) {
                    super::clipmap::ClipmapGroundHit::Hit(point) => Some(frame.to_world(point)),
                    _ => None,
                }
            }
            // 薄板实测中Parry的仅距离查询与完整面查询会出现分歧；拾取和种植资格
            // 必须复用同一次真实三角面命中，不扰动射线或增厚来源来掩盖空命中。
            Self::Mesh(..) => self.ray_surface(origin, direction, 1_000_000.0).map(|(point, _)| point),
        }
    }
}

#[cfg(test)]
#[path = "terrain_surface_kind_tests.rs"]
mod surface_kind_tests;

fn source_up_is_world_up(frame: SceneSourceFrame) -> bool {
    frame.source_to_world.transform_vector3(Vec3::Y).try_normalize()
        .is_some_and(|up| up.distance_squared(Vec3::Y) < 1.0e-12)
}

#[derive(SystemParam)]
pub struct TerrainObjectQueries<'w, 's> {
    runtime: Option<Res<'w, super::instances::SubsceneRuntime>>,
    lookup: Option<Res<'w, super::instances::TerrainInstanceLookup>>,
    sources: Option<Res<'w, InstanceResourceTable<SceneContentRegistry>>>,
    definitions: Query<'w, 's, (&'static SubsceneInstance, &'static TerrainObjectInstance, &'static SceneSourceFrame)>,
    contexts: Query<'w, 's, &'static SubsceneInstance>,
    active: Query<'w, 's, (), With<SubsceneActive>>,
    ground: Option<Res<'w, InstanceResourceTable<DynamicClipmapGroundSurface>>>,
    voxels: Option<Res<'w, InstanceResourceTable<VoxelObjectRuntime>>>,
    scenes: Option<Res<'w, InstanceResourceTable<super::resources::DynamicSceneRuntimeState>>>,
    root_scene: Option<Res<'w, super::resources::DynamicSceneRuntimeState>>,
    root_clipmap: Option<Res<'w, super::sparse_runtime::SparseClipmapRuntime>>,
    clipmaps: Option<Res<'w, InstanceResourceTable<super::sparse_runtime::SparseClipmapRuntime>>>,
    render_meshes: Query<'w, 's, (&'static Mesh3d, &'static Transform)>,
    render_children: Query<'w, 's, &'static Children>,
    colliders: Query<'w, 's, Ref<'static, Collider>>,
}
/// 身份只由实际采用的地表和源坐标组成，相机游走不会使完整场景重复重建索引。
#[derive(Clone, PartialEq)]
pub struct TerrainSurfaceVersion {
    entity: Entity,
    collider: Option<(Entity, u64)>,
    content: Option<([u64; 7], bool)>,
    query: Option<([u64; 3], [UVec4; 16], Option<u64>)>,
    frame: Mat4,
}
impl TerrainSurfaceVersion {
    pub fn same_content(&self, other: &Self) -> bool {
        self.entity == other.entity && self.collider == other.collider && self.content == other.content && self.frame == other.frame
    }
    /// 几何修订不含相机换页和对象姿态；公共表面命中仍另带完整实例地址与局部坐标。
    pub fn geometry_revision(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(self.entity.to_bits().to_le_bytes());
        if let Some((entity, revision)) = self.collider {
            digest.update([1]); digest.update(entity.to_bits().to_le_bytes()); digest.update(revision.to_le_bytes());
        } else { digest.update([0]); }
        if let Some((words, ready)) = self.content {
            digest.update([1, u8::from(ready)]);
            for word in words { digest.update(word.to_le_bytes()); }
        } else { digest.update([0]); }
        digest.finalize().into()
    }
}
impl TerrainObjectQueries<'_, '_> {
    /// 整体子场景预览与取证只按稳定地址定位当前活动根，不扫描实体或借用候选批次。
    pub fn subscene_instance(&self, owner: &SceneId, id: &feathrise_scene_content::SceneInstanceId) -> Option<Entity> {
        let path = feathrise_scene_content::SceneInstancePath::root(owner.clone()).child(id.clone());
        let entity = self.runtime.as_ref()?.active_entity(&path)?;
        let instance = self.contexts.get(entity).ok()?;
        (instance.path == path && self.active.contains(instance.batch)).then_some(entity)
    }
    /// 内容仍属于场景容器；同源多实例通过完整路径查询，不能用TerrainId跨实例借姿态。
    pub fn binding_pose(&self, owner: &SceneId, scope: Option<Entity>, binding: &feathrise_scene_content::SceneTerrainBinding)
        -> Result<feathrise_scene_content::TerrainObjectTransform,String> {
        let context = scope.map(|entity| self.contexts.get(entity).map_err(|_| "关联场景实例已不存在")).transpose()?;
        if context.is_some_and(|context| &context.source_scene_id != owner) {
            return Err("关联内容不能借用另一源场景实例的地形姿态".into());
        }
        let path=context.map_or_else(||feathrise_scene_content::SceneInstancePath::root(owner.clone()),|context|context.path.clone())
            .terrain(binding.terrain_id.clone());
        let entity = if let Some(context) = context {
            let found = self.lookup.as_ref().and_then(|lookup| lookup.instance(context.batch, &path));
            if let Some(entity) = found { entity }
            else if self.lookup.as_ref().is_some_and(|lookup| lookup.contexts_complete(context.batch)) {
                return Err(format!("关联地形{}已删除、禁用或不属于本实例", binding.terrain_id.as_str()));
            } else { return Err("关联地形仍在加载".into()); }
        } else {
            // 根查询只消费已采用Runtime，不能从候选只读行借用尚未激活的地形。
            self.runtime.as_ref().and_then(|runtime| runtime.active_entity(&path)).ok_or("关联地形仍在加载")?
        };
        let (instance,object,_) = self.definitions.get(entity).map_err(|_|"关联地形定义已失效")?;
        if &instance.source_scene_id != owner || instance.path != path || object.definition.id != binding.terrain_id
            || context.is_some_and(|context| instance.batch != context.batch || instance.generation != context.generation) {
            return Err("关联地形实例身份已换代".into());
        }
        if !object.definition.enabled {return Err("关联地形已禁用".into());}
        binding.resolve(object.definition.transform)
    }
    pub fn instance(&self, owner: &SceneId, id: &TerrainId) -> Option<Entity> {
        self.instance_source(owner, id).map(|(entity, _, _)| entity)
    }
    /// 显式诊断只克隆已采用显示查询及小身份；三角摘要和射线仍须由调用方放入后台。
    #[cfg(feature = "test_panel")]
    pub fn voxel_diagnostic_snapshot(&self, owner: &SceneId, id: &TerrainId)
        -> Option<(Collider, SceneSourceFrame, serde_json::Value)> {
        let (entity, _, frame) = self.instance_source(owner, id)?;
        let runtime = self.voxels.as_ref()?.values.get(&entity)?;
        Some((runtime.display_query.clone()?, frame, runtime.diagnostic_identity()))
    }
    /// 仅报告该已采用地形实例的正式碰撞发布门；显示查询不能代替物理完成证据。
    pub fn collision_is_ready(&self, owner: &SceneId, id: &TerrainId) -> bool {
        let Some(entity) = self.instance(owner, id) else { return false; };
        self.scenes.as_ref().and_then(|table| table.values.get(&entity))
            .is_some_and(|map| map.active_scene_id.as_ref() == Some(owner) && map.collision_ready)
    }
    /// 活动地址表可能晚于退役实体失效；父级查询须同时证明真实身份、启用状态和活动批次。
    /// 返回已采用定义与完整源矩阵，供作者比较类型和姿态，不能把旧父级冒充新作者框架。
    pub fn instance_source(&self, owner: &SceneId, id: &TerrainId)
        -> Option<(Entity, &feathrise_scene_content::SceneTerrainObject, SceneSourceFrame)> {
        let path = feathrise_scene_content::SceneInstancePath::root(owner.clone()).terrain(id.clone());
        let entity = self.runtime.as_ref()?.active_entity(&path)?;
        let (instance, object, frame) = self.definitions.get(entity).ok()?;
        if &instance.source_scene_id != owner || instance.path != path || object.definition.id != *id
            || !object.definition.enabled || !self.active.contains(instance.batch) {
            return None;
        }
        Some((entity, &object.definition, *frame))
    }
    /// 作者预热与辅助显示复用已采用对象的几何；固定地址查询不扫描其他地形或子场景。
    /// 返回源框架与显示局部姿态的完整组合，调用方不得再叠加一次对象变换。
    pub fn render_mesh(&self, owner: &SceneId, id: &TerrainId) -> Option<(Handle<Mesh>, Mat4, feathrise_scene_content::SceneTerrainKind)> {
        let entity = self.instance(owner, id)?;
        let (instance, object, frame) = self.definitions.get(entity).ok()?;
        if &instance.source_scene_id != owner || !instance.path.instances.is_empty()
            || object.definition.id != *id || !object.definition.enabled || !self.active.contains(instance.batch) {
            return None;
        }
        let kind = object.definition.terrain.kind;
        let map = self.scenes.as_ref()?.values.get(&entity)?;
        if map.active_scene_id.as_ref() != Some(owner) { return None; }
        let display = match kind {
            feathrise_scene_content::SceneTerrainKind::SparseClipmap => {
                let clipmap = self.clipmaps.as_ref()?.values.get(&entity)?;
                let applied = clipmap.applied.as_ref()?;
                if applied.scene_id != *owner || Some(applied.root) != map.root { return None; }
                clipmap.display?
            }
            feathrise_scene_content::SceneTerrainKind::Voxel => {
                // 首块只证明共同顶点布局，足够GPU材质预热；不能代表整套体素块辅助覆盖。
                let root = self.voxels.as_ref()?.values.get(&entity)?.active?;
                *self.render_children.get(root).ok()?.first()?
            }
            _ => return None,
        };
        let (mesh, local) = self.render_meshes.get(display).ok()?;
        Some((mesh.0.clone(), frame.source_to_world * local.to_matrix(), kind))
    }
    /// 旧单地形也读取自己的固定运行地址；空容器不会提供隐式平面或另一对象后备。
    pub fn root_render_mesh(&self, owner: &SceneId) -> Option<(Handle<Mesh>, Mat4, feathrise_scene_content::SceneTerrainKind)> {
        let map = self.root_scene.as_ref()?;
        if map.active_scene_id.as_ref() != Some(owner) { return None; }
        let (display, kind) = if map.sparse_height_path.is_some() {
            let clipmap = self.root_clipmap.as_ref()?;
            let applied = clipmap.applied.as_ref()?;
            if applied.scene_id != *owner || Some(applied.root) != map.root { return None; }
            (clipmap.display?, feathrise_scene_content::SceneTerrainKind::SparseClipmap)
        } else { return None; };
        let (mesh, local) = self.render_meshes.get(display).ok()?;
        Some((mesh.0.clone(), local.to_matrix(), kind))
    }
    /// BTreeMap游标只访问本帧固定数量的上下文；不会反复从头跳过整个实体查询。
    pub fn next_instance(&self, after: Option<Entity>) -> Option<Entity> {
        let sources = &self.sources.as_ref()?.values;
        if let Some(after) = after { sources.range((Excluded(after), Unbounded)).next().map(|(entity, _)| *entity) }
        else { sources.keys().next().copied() }
    }
    pub fn snapshot(&self, entity: Entity, owner: &SceneId) -> Option<(TerrainId, TerrainObjectRaySnapshot)> {
        self.snapshot_versioned(entity, owner).map(|(object, snapshot, _)| (object.id.clone(), snapshot))
    }
    pub fn snapshot_versioned(&self, entity: Entity, owner: &SceneId) -> Option<(std::sync::Arc<feathrise_scene_content::SceneTerrainObject>, TerrainObjectRaySnapshot, TerrainSurfaceVersion)> {
        let (instance, _, _) = self.definitions.get(entity).ok()?;
        if !instance.path.instances.is_empty() { return None; }
        self.snapshot_at(entity, owner, &instance.path)
    }
    /// 通过完整地址读取种植表面，兄弟子场景不能以相同 TerrainId 借用彼此的变换。
    pub fn paint_surface_snapshot(&self, path: &feathrise_scene_content::SceneInstancePath) -> Option<crate::paint_surface::PaintSurfaceSnapshot> {
        let entity = self.runtime.as_ref()?.active_entity(path)?;
        let (instance, _, _) = self.definitions.get(entity).ok()?;
        let (object, geometry, version) = self.snapshot_at(entity, &instance.source_scene_id, path)?;
        Some(crate::paint_surface::PaintSurfaceSnapshot {
            target: feathrise_scene_content::SurfaceTarget::Terrain { terrain_id: object.id.clone() },
            instance_path: path.clone(), geometry_revision: version.geometry_revision(),
            geometry: crate::paint_surface::PaintSurfaceGeometry::Terrain(geometry),
        })
    }
    fn snapshot_at(&self, entity: Entity, owner: &SceneId, path: &feathrise_scene_content::SceneInstancePath) -> Option<(std::sync::Arc<feathrise_scene_content::SceneTerrainObject>, TerrainObjectRaySnapshot, TerrainSurfaceVersion)> {
        let (instance, object, frame) = self.definitions.get(entity).ok()?;
        if &instance.source_scene_id != owner || instance.path != *path || !object.definition.enabled || !self.active.contains(instance.batch) { return None; }
        if let Some(snapshot) = self.ground.as_ref().and_then(|table| table.values.get(&entity)).and_then(|surface| surface.snapshot.clone()) {
            let version = TerrainSurfaceVersion { entity, collider: None,
                content: Some((snapshot.content_revision_words(), snapshot.ready)),
                query: Some((snapshot.revision, snapshot.missing_pages, snapshot.authoritative_page_revision())), frame: frame.source_to_world };
            return Some((object.definition.clone(), TerrainObjectRaySnapshot::Clipmap(snapshot, *frame), version));
        }
        if let Some(runtime) = self.voxels.as_ref().and_then(|table| table.values.get(&entity))
            && let (Some(active), Some(query)) = (runtime.active, &runtime.display_query) {
            let version = TerrainSurfaceVersion { entity, collider: Some((active, runtime.generation)), content: None, query: None, frame: frame.source_to_world };
            return Some((object.definition.clone(), TerrainObjectRaySnapshot::Mesh(query.clone(), *frame), version));
        }
        let collider_entity = self.voxels.as_ref().and_then(|table| table.values.get(&entity)).and_then(|runtime| runtime.active)
            .or_else(|| self.scenes.as_ref().and_then(|table| table.values.get(&entity))
                .and_then(|map| map.terrain_display.or_else(|| (!map.derive_glb_height && map.glb_path.is_some()).then_some(map.root).flatten())))?;
        let collider = self.colliders.get(collider_entity).ok()?;
        let version = TerrainSurfaceVersion { entity, collider: Some((collider_entity, u64::from(collider.last_changed().get()))), content: None, query: None, frame: frame.source_to_world };
        Some((object.definition.clone(), TerrainObjectRaySnapshot::Mesh((*collider).clone(), *frame), version))
    }
}
