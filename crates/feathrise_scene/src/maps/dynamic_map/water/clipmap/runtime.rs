//! Clipmap 静水与样条水内容批次、GPU限流、原子实体采用与视图跟随。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod static_batch;
#[cfg(test)]
mod author_tests;
use static_batch::StaticBatch;
pub(crate) use static_batch::reap as reap_static_water;

use bevy::asset::LoadState;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::ecs::system::SystemParam;
use bevy::image::{ImageLoaderSettings, ImageSampler};
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use feathrise_scene_core::view::CameraRaycastIgnore;
use feathrise_map_content::{
    ActiveDynamicMap, MAP_SCHEMA_VERSION, MapContentRegistry, MapId, MapManifestAsset,
    MapTerrainKind, MapWaterAsset, MapWaterLayer, MapWaterMaskAsset, MapWaterSegmentKind,
    WaterPresetRegistry,
};
use feathrise_scene_core::state::ScenePhase;

use super::material::{ClipmapWaterExtension, ClipmapWaterMaterial};
use crate::DynamicSparseClipmapPrimitive;
use crate::maps::dynamic_map::clipmap::{
    ClipmapGroundHit, ClipmapHeightExtension, ClipmapHeightMaterial, DynamicClipmapGroundSurface,
    SparseClipmapSurfaceMaterial,
};
use crate::maps::dynamic_map::sparse_runtime::SparseClipmapRuntime;
use crate::shared::large_map::{
    BuiltWaterSurface, DynamicWaterCpuInteraction, DynamicWaterMaterial, DynamicWaterSurface,
    PaintedWaterPurpose, PaintedWaterSurface, PaintedWaterSurfaceSpawn, StagedWaterSurface,
    WaterGpuUploadQueue, WaterGpuUploadReceipts, WaterGpuUploadTicket, WaterSurfaceDescriptor,
    WaterSurfaceKind, create_water_surface_material, spawn_staged_water_surface,
    spawn_staged_water_warmup, stage_water_surface_chunk,
};

use super::super::build::build_dynamic_spline_water_network;
use super::super::resources::{
    DynamicClipmapWaterDraft, DynamicWaterPhase, DynamicWaterState, DynamicWaterStatus,
};

type PrepareIdentity = (MapId, u64, Option<u64>); // 地图、内容代次和可选作者草稿修订。
type PrepareResult = (PrepareIdentity, Result<PreparedBatch, String>);

struct PreparedBatch {
    layers: Vec<PreparedLayer>,
    spline_builds: Vec<BuiltWaterSurface>,
    river_count: usize,
    rapid_segment_count: usize,
    waterfall_count: usize,
}

struct FrozenLayer {
    layer: MapWaterLayer,
    descriptor: WaterSurfaceDescriptor,
    pixels: Arc<[u8]>,
    draft_image: Option<Handle<Image>>,
}

struct PreparedLayer {
    layer: MapWaterLayer,
    descriptor: WaterSurfaceDescriptor,
    pixels: Arc<[u8]>,
    coverage_bounds: Option<[u32; 4]>, // min inclusive、max exclusive，供选择和空层跳过。
    draft_image: Option<Handle<Image>>,
}

struct PendingLayer {
    prepared: PreparedLayer,
    image: Option<Handle<Image>>,
}

struct PendingBatch {
    identity: PrepareIdentity,
    layers: VecDeque<PendingLayer>,
    registered: Vec<PendingLayer>,
    static_batch: Option<StaticBatch>,
    image_cursor: usize,
    spline_remaining: VecDeque<BuiltWaterSurface>,
    spline_staged: Vec<StagedWaterSurface>,
    spline_warmup_entities: Vec<Entity>,
    spline_ticket: Option<WaterGpuUploadTicket>,
    spline_triangle_count: usize,
    river_count: usize,
    rapid_segment_count: usize,
    waterfall_count: usize,
}

struct InstalledStaticLayer {
    entity: Entity,
    material: Handle<ClipmapWaterMaterial>,
}

struct InstalledSplineLayer {
    spawn: PaintedWaterSurfaceSpawn,
}

/// 正式与作者预览共用单飞后台构建，图片、静水层和样条上传块逐帧准备，数量不限制。
#[derive(Resource, Default)]
pub(crate) struct ClipmapWaterRuntime {
    task: Option<Task<PrepareResult>>,
    pending: Option<PendingBatch>,
    static_installed: Option<StaticBatch>,
    static_retired: VecDeque<StaticBatch>,
    spline_installed: Vec<InstalledSplineLayer>,
    installed_map_id: Option<MapId>,
    observed_draft_revision: Option<u64>,
    attempted: Option<PrepareIdentity>,
}

impl ClipmapWaterRuntime {
    pub(in crate::maps::dynamic_map) fn background_busy(&self) -> bool { self.task.is_some() }
    pub(in crate::maps::dynamic_map) fn background_finished(&self) -> bool {
        self.task.as_ref().is_none_or(|task| task.is_finished())
    }
}

/// 标记由Clipmap水域事务安装的世界坐标Ribbon，F12草稿预览不得把它当成GLB旧批次隐藏。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ClipmapSplineWaterSurface;

/// CPU互动、选择和雨水代理读取的同一已采用静水足迹。
#[derive(Component, Clone)]
pub struct ClipmapWaterSurface {
    pub map_id: MapId,
    pub layer_id: String,
    pub resolution: [u32; 2],
    pub world_min: Vec2,
    pub world_max: Vec2,
    pub water_level: f32,
    pub minimum_depth: f32,
    pub height_revision: [u64; 3],
    pub coverage_bounds: [u32; 4],
    mask: Arc<[u8]>,
    volume: bool, // 外表面和三维派生水面互斥授权，防止同一洞内接触被处理两次。
    published: Option<Arc<AtomicBool>>, // 静水候选可逐帧建实体，渲染和CPU仍在同一整批边沿生效。
}

impl ClipmapWaterSurface {
    /// 分帧候选或已退役层不能用于编辑器高亮、选择和玩家互动。
    pub fn is_published(&self) -> bool {
        self.published
            .as_ref()
            .is_none_or(|state| state.load(Ordering::Acquire))
    }
    #[cfg(test)]
    pub(super) fn with_test_mask(mut self, resolution: [u32; 2], mask: Vec<u8>) -> Self {
        assert_eq!(mask.len(), resolution[0] as usize * resolution[1] as usize);
        self.mask = Arc::from(mask);
        self.resolution = resolution;
        self.coverage_bounds = [0, 0, resolution[0], resolution[1]];
        self
    }
    pub(super) fn volume_copy(mut self) -> Self {
        self.volume = true;
        self
    }
    pub(super) fn mask_identity(&self) -> usize {
        Arc::as_ptr(&self.mask) as *const u8 as usize
    }
    /// 使用与GPU纹理相同的归一化世界映射双线性读取R8；地图外严格为零。
    pub fn coverage_at(&self, world_xz: Vec2) -> f32 {
        if !self.is_published()
            || !world_xz.is_finite()
            || world_xz.cmplt(self.world_min).any()
            || world_xz.cmpgt(self.world_max).any()
        {
            return 0.0;
        }
        let size = self.world_max - self.world_min;
        let uv = ((world_xz - self.world_min) / size).clamp(Vec2::ZERO, Vec2::ONE);
        let last = UVec2::new(self.resolution[0] - 1, self.resolution[1] - 1);
        let texel = uv * last.as_vec2();
        let lo = texel.floor().as_uvec2().min(last);
        let hi = (lo + UVec2::ONE).min(last);
        let fraction = texel - lo.as_vec2();
        let sample = |point: UVec2| {
            self.mask[point.y as usize * self.resolution[0] as usize + point.x as usize] as f32
                / 255.0
        };
        let low = sample(lo).lerp(sample(UVec2::new(hi.x, lo.y)), fraction.x);
        let high = sample(UVec2::new(lo.x, hi.y)).lerp(sample(hi), fraction.x);
        low.lerp(high, fraction.y)
    }

    /// 同时核对R8、真实Clipmap三角面水深与高度修订；缺页和跨代均不产生互动。
    pub fn contains_surface(
        &self,
        ground: &crate::maps::dynamic_map::ClipmapGroundSnapshot,
        world_xz: Vec2,
    ) -> bool {
        self.surface_depth_at(ground, world_xz).is_some()
    }

    /// 返回水位与当前同代真实承托面的正水深；R8空洞、缺页、地图外及水位不高于地面均为None。
    pub fn surface_depth_at(
        &self,
        ground: &crate::maps::dynamic_map::ClipmapGroundSnapshot,
        world_xz: Vec2,
    ) -> Option<f32> {
        if ground.map_id != self.map_id
            || ground.revision != self.height_revision
            || self.coverage_at(world_xz) <= 0.0
        {
            return None;
        }
        if ground
            .volume_footprints()
            .iter()
            .any(|hole| hole.contains(world_xz))
            != self.volume
        {
            return None; // 室外环带在洞口被裁切，其CPU互动也必须由洞内真实水面接管。
        }
        // 室外按正式显示LOD三角裁岸，与GPU水面保持一致；洞内仍只接受水位下的真实承托面。
        let support = if self.volume {
            ground.sample_support_below(
                Vec3::new(world_xz.x, self.water_level, world_xz.y),
                100000.0,
            )
        } else {
            ground.sample_surface(world_xz)
        };
        let ClipmapGroundHit::Hit(point) = support else {
            return None;
        };
        let depth = self.water_level - point.y;
        // 安全水深用于波幅衰减，不能裁掉可见浅水；玩家是否入水另由足底深度门判定。
        (depth > 0.0).then_some(depth)
    }

    pub fn world_coverage_bounds(&self) -> (Vec2, Vec2) {
        let size = self.world_max - self.world_min;
        let last = UVec2::new(self.resolution[0] - 1, self.resolution[1] - 1);
        // 线性过滤会让一个非零纹素影响到相邻作者采样点；包围范围向外扩一格，
        // 供点选取景使用保守范围，真正命中仍由R8与真实三角水深共同裁切。
        let min_texel = UVec2::new(self.coverage_bounds[0], self.coverage_bounds[1])
            .saturating_sub(UVec2::ONE)
            .min(last);
        let max_texel = UVec2::new(self.coverage_bounds[2], self.coverage_bounds[3]).min(last);
        let grid_extent = last.as_vec2().max(Vec2::ONE);
        let min = self.world_min + min_texel.as_vec2() / grid_extent * size;
        let max = self.world_min + max_texel.as_vec2() / grid_extent * size;
        (min, max)
    }
}

#[cfg(test)]
pub(crate) fn test_clipmap_water_surface() -> ClipmapWaterSurface {
    ClipmapWaterSurface {
        map_id: MapId::parse("MAP-clipmap-rain-test").unwrap(),
        layer_id: "lake".to_owned(),
        resolution: [2, 2],
        world_min: Vec2::ZERO,
        world_max: Vec2::splat(64.0),
        water_level: 2.0,
        minimum_depth: 0.2,
        height_revision: [1, 2, 3],
        coverage_bounds: [0, 0, 2, 2],
        mask: Arc::from(vec![255; 4]),
        volume: false,
        published: None,
    }
}

pub(crate) fn begin_clipmap_water(mut runtime: InstanceResMut<ClipmapWaterRuntime>) {
    runtime.task = None;
    runtime.pending = None;
    runtime.observed_draft_revision = None;
    runtime.attempted = None;
}

fn remove_installed(
    commands: &mut Commands,
    _meshes: &mut Assets<Mesh>,
    _clip_materials: &mut Assets<ClipmapWaterMaterial>,
    _dynamic_materials: &mut Assets<DynamicWaterMaterial>,
    runtime: &mut ClipmapWaterRuntime,
    uploads: &mut WaterGpuUploadQueue,
) {
    if let Some(installed) = runtime.static_installed.take() {
        installed.retire(commands, runtime, uploads);
    }
    for installed in runtime.spline_installed.drain(..) {
        installed.spawn.retire(commands);
    }
    runtime.installed_map_id = None;
}

/// 换图、热更或失败时回收尚未发布的样条候选；迟到RenderApp回执按ticket自然失效。
fn discard_pending(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<DynamicWaterMaterial>,
    uploads: &mut WaterGpuUploadQueue,
    runtime: &mut ClipmapWaterRuntime,
) {
    let Some(mut pending) = runtime.pending.take() else {
        return;
    };
    if let Some(batch) = pending.static_batch.take() {
        batch.retire(commands, runtime, uploads);
    }
    if let Some(ticket) = pending.spline_ticket.take() {
        uploads.retire(ticket);
    }
    for entity in pending.spline_warmup_entities.drain(..) {
        commands.entity(entity).try_despawn();
    }
    for staged in pending.spline_staged.drain(..) {
        staged.discard(commands, meshes, materials);
    }
    crate::retire_water_cpu(commands, pending);
}

pub(crate) fn clear_clipmap_water(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut clip_materials: ResMut<Assets<ClipmapWaterMaterial>>,
    mut dynamic_materials: ResMut<Assets<DynamicWaterMaterial>>,
    mut uploads: ResMut<WaterGpuUploadQueue>,
    mut runtime: InstanceResMut<ClipmapWaterRuntime>,
) {
    discard_pending(
        &mut commands,
        &mut meshes,
        &mut dynamic_materials,
        &mut uploads,
        &mut runtime,
    );
    remove_installed(
        &mut commands,
        &mut meshes,
        &mut clip_materials,
        &mut dynamic_materials,
        &mut runtime,
        &mut uploads,
    );
    runtime.task = None;
    runtime.observed_draft_revision = None;
    runtime.attempted = None;
}

fn coverage_bounds(pixels: &[u8], resolution: [u32; 2]) -> Option<[u32; 4]> {
    let mut min = UVec2::splat(u32::MAX);
    let mut max = UVec2::ZERO;
    let mut found = false;
    for (index, value) in pixels.iter().copied().enumerate() {
        if value == 0 {
            continue;
        }
        let index = index as u32;
        let point = UVec2::new(index % resolution[0], index / resolution[0]);
        min = min.min(point);
        max = max.max(point + UVec2::ONE);
        found = true;
    }
    found.then_some([min.x, min.y, max.x, max.y])
}

fn request_numeric_image(asset_server: &AssetServer, path: String) -> Handle<Image> {
    asset_server
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| {
            settings.is_srgb = false;
            settings.sampler = ImageSampler::linear();
        })
        .load(path)
}

fn fail(
    state: &mut DynamicWaterState,
    status: &mut DynamicWaterStatus,
    runtime: &mut ClipmapWaterRuntime,
    detail: String,
) {
    state.dirty = false;
    runtime.pending = None;
    status.phase = DynamicWaterPhase::Failed;
    status.detail = detail;
    error!("{}", status.detail);
}

/// F12逐点编辑允许空路径和单点路径；正式磁盘资产仍执行完整网络与连接校验。
fn validate_clipmap_water_input(
    definition: &MapWaterAsset,
    map_id: &MapId,
    authoring_draft: bool,
) -> Result<(), String> {
    if !authoring_draft {
        return definition.validate(map_id);
    }
    let mut completed_static = definition.clone();
    completed_static.rivers.clear();
    completed_static.waterfalls.clear();
    completed_static.validate(map_id)
}

/// 只有静水环带需要当前高度页；世界XYZ样条批次不能被未驻留的地形页阻塞。
fn clipmap_batch_requires_ground(static_layer_count: usize) -> bool {
    static_layer_count > 0
}

/// 诊断统计读取正式采用的LOD计划，不能反读仅供RenderWorld的组合Mesh。
fn clipmap_plan_triangle_count(index_count: Option<u64>) -> usize {
    index_count
        .map(|count| usize::try_from(count / 3).unwrap_or(usize::MAX))
        .unwrap_or(0)
}

fn source_height(
    surfaces: &Assets<SparseClipmapSurfaceMaterial>,
    fallbacks: &Assets<ClipmapHeightMaterial>,
    surface: Option<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
    fallback: Option<&MeshMaterial3d<ClipmapHeightMaterial>>,
) -> Option<ClipmapHeightExtension> {
    surface
        .and_then(|handle| {
            surfaces
                .get(&handle.0)
                .map(|material| material.extension.clone())
        })
        .or_else(|| {
            fallback.and_then(|handle| {
                fallbacks
                    .get(&handle.0)
                    .map(|material| material.extension.clone())
            })
        })
}

#[derive(SystemParam)]
pub(crate) struct ClipmapWaterRuntimeParams<'w, 's> {
    fallback: Res<'w,feathrise_environment::terrain_surface::TerrainSurfaceFallbackImages>,
    clipmap_runtime: InstanceRes<'w, SparseClipmapRuntime>,
    asset_server: Res<'w, AssetServer>,
    active: InstanceRes<'w, ActiveDynamicMap>,
    registry: Option<feathrise_scene_core::residency::InstanceRes<'w, MapContentRegistry>>,
    manifests: Res<'w, Assets<MapManifestAsset>>,
    waters: Res<'w, Assets<MapWaterAsset>>,
    masks: Res<'w, Assets<MapWaterMaskAsset>>,
    images: Res<'w, Assets<Image>>,
    device: Option<Res<'w, bevy::render::renderer::RenderDevice>>,
    presets: Option<Res<'w, WaterPresetRegistry>>,
    // 当前对象的未保存水网借用编辑器作者根；其余对象继续读取各自实例表里的停放版本。
    draft: super::super::super::author_sources::TerrainAuthorRes<'w, 's, DynamicClipmapWaterDraft>,
    ground: InstanceRes<'w, DynamicClipmapGroundSurface>,
    surface_materials: Res<'w, Assets<SparseClipmapSurfaceMaterial>>,
    fallback_materials: Res<'w, Assets<ClipmapHeightMaterial>>,
    sources: InstanceQuery<
        'w,
        's,
        (
            &'static Mesh3d,
            &'static Transform,
            Option<&'static MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
            Option<&'static MeshMaterial3d<ClipmapHeightMaterial>>,
        ),
        With<DynamicSparseClipmapPrimitive>,
    >,
    meshes: ResMut<'w, Assets<Mesh>>,
    clip_materials: ResMut<'w, Assets<ClipmapWaterMaterial>>,
    dynamic_materials: ResMut<'w, Assets<DynamicWaterMaterial>>,
    uploads: ResMut<'w, WaterGpuUploadQueue>,
    receipts: Res<'w, WaterGpuUploadReceipts>,
    state: InstanceResMut<'w, DynamicWaterState>,
    status: InstanceResMut<'w, DynamicWaterStatus>,
    runtime: InstanceResMut<'w, ClipmapWaterRuntime>,
}

pub(crate) fn drive_clipmap_water(
    mut commands: Commands,
    time: Res<Time>,
    params: ClipmapWaterRuntimeParams,
) {
    let ClipmapWaterRuntimeParams {
        fallback,
        clipmap_runtime,
        asset_server,
        active,
        registry,
        manifests,
        waters,
        masks,
        images,
        device,
        presets,
        draft,
        ground,
        surface_materials,
        fallback_materials,
        sources,
        mut meshes,
        mut clip_materials,
        mut dynamic_materials,
        mut uploads,
        receipts,
        mut state,
        mut status,
        mut runtime,
    } = params;
    let Some(map_id) = active.map_id.as_ref() else {
        return;
    };
    let Some(registry) = registry.as_deref() else {
        return;
    };
    let Some(handles) = registry.handles(map_id) else {
        return;
    };
    let Some(manifest) = manifests.get(&handles.manifest) else {
        return;
    };
    if !matches!(manifest.terrain.kind,MapTerrainKind::SparseClipmap|MapTerrainKind::Voxel) {
        discard_pending(
            &mut commands,
            &mut meshes,
            &mut dynamic_materials,
            &mut uploads,
            &mut runtime,
        );
        if runtime.installed_map_id.is_some() {
            remove_installed(
                &mut commands,
                &mut meshes,
                &mut clip_materials,
                &mut dynamic_materials,
                &mut runtime,
                &mut uploads,
            );
        }
        return;
    }

    let draft_revision = draft.revision_for_map(map_id);
    if runtime.observed_draft_revision != draft_revision {
        runtime.observed_draft_revision = draft_revision;
        state.mark_dirty(&mut status);
    }
    status.input_draft_revision = draft_revision;
    if runtime
        .installed_map_id
        .as_ref()
        .is_some_and(|installed| installed != map_id)
    {
        discard_pending(
            &mut commands,
            &mut meshes,
            &mut dynamic_materials,
            &mut uploads,
            &mut runtime,
        );
        remove_installed(
            &mut commands,
            &mut meshes,
            &mut clip_materials,
            &mut dynamic_materials,
            &mut runtime,
            &mut uploads,
        );
        status.applied_generation = 0;
        status.height_binding_revision = [0; 3];
    }

    if runtime.pending.as_ref().is_some_and(|pending| {
        pending.identity.0 != *map_id
            || pending.identity.1 != state.generation
            || pending.identity.2 != draft_revision
    }) {
        discard_pending(
            &mut commands,
            &mut meshes,
            &mut dynamic_materials,
            &mut uploads,
            &mut runtime,
        );
    }
    if let Some(result) = runtime.task.as_mut().and_then(check_ready) {
        runtime.task = None;
        match result {
            (identity, Ok(prepared))
                if identity.0 == *map_id
                    && identity.1 == state.generation
                    && identity.2 == draft_revision =>
            {
                runtime.pending = Some(PendingBatch {
                    identity,
                    layers: prepared
                        .layers
                        .into_iter()
                        .filter(|layer| layer.coverage_bounds.is_some())
                        .map(|prepared| PendingLayer {
                            image: None,
                            prepared,
                        })
                        .collect(),
                    registered: Vec::new(),
                    static_batch: None,
                    image_cursor: 0,
                    spline_remaining: prepared.spline_builds.into(),
                    spline_staged: Vec::new(),
                    spline_warmup_entities: Vec::new(),
                    spline_ticket: None,
                    spline_triangle_count: 0,
                    river_count: prepared.river_count,
                    rapid_segment_count: prepared.rapid_segment_count,
                    waterfall_count: prepared.waterfall_count,
                });
            }
            (_, Ok(obsolete)) => {
                crate::retire_water_cpu(&mut commands, obsolete);
            }
            (identity, Err(error))
                if identity.0 == *map_id
                    && identity.1 == state.generation
                    && identity.2 == draft_revision =>
            {
                fail(&mut state, &mut status, &mut runtime, error);
                return;
            }
            _ => {}
        }
    }

    if let Some(pending) = runtime.pending.as_mut() {
        if let Some(layer) = pending.layers.front() {
            if let Some(device) = device.as_deref() {
                let limit = device.limits().max_texture_dimension_2d;
                if layer
                    .prepared
                    .layer
                    .resolution
                    .iter()
                    .any(|&size| size > limit)
                {
                    let detail = format!(
                        "静水遮罩{}×{}超出当前GPU二维纹理能力{limit}",
                        layer.prepared.layer.resolution[0], layer.prepared.layer.resolution[1]
                    );
                    discard_pending(
                        &mut commands,
                        &mut meshes,
                        &mut dynamic_materials,
                        &mut uploads,
                        &mut runtime,
                    );
                    state.dirty = false;
                    status.phase = DynamicWaterPhase::Failed;
                    status.detail = detail;
                    return;
                }
            }
        }
        if let Some(mut layer) = pending.layers.pop_front() {
            layer.image = Some(layer.prepared.draft_image.clone().unwrap_or_else(|| {
                request_numeric_image(
                    &asset_server,
                    layer.prepared.layer.mask_texture_path.clone(),
                )
            }));
            pending.registered.push(layer); // 每帧至多登记或采用一张图片句柄。
            status.phase = DynamicWaterPhase::Loading;
            status.detail = format!(
                "正在分帧登记Clipmap静水遮罩：已登记{}张，剩余{}张",
                pending.registered.len(),
                pending.layers.len()
            );
            return;
        }
        for _ in 0..32 {
            let Some(layer) = pending.registered.get(pending.image_cursor) else {
                break;
            };
            let handle = layer.image.as_ref().expect("已登记层必须持有图片句柄");
            if matches!(asset_server.load_state(handle.id()), LoadState::Failed(_)) {
                let detail = format!("Clipmap静水层{}的KTX2遮罩加载失败", layer.prepared.layer.id);
                discard_pending(
                    &mut commands,
                    &mut meshes,
                    &mut dynamic_materials,
                    &mut uploads,
                    &mut runtime,
                );
                state.dirty = false;
                status.phase = DynamicWaterPhase::Failed;
                status.detail = detail;
                return;
            }
            if images.get(handle).is_none() {
                status.phase = DynamicWaterPhase::Loading;
                status.detail = format!(
                    "正在等待Clipmap静水层{}的GPU遮罩",
                    layer.prepared.layer.label
                );
                return;
            }
            pending.image_cursor += 1;
        }
        if pending.image_cursor < pending.registered.len() {
            return;
        }

        // 静水图片就绪后再消费全局大候选额度；同一帧最多登记一张Ribbon Mesh。
        for _ in 0..32 {
            if !pending
                .spline_remaining
                .front()
                .is_some_and(|build| build.mesh.is_empty())
            {
                break;
            }
            pending.spline_remaining.pop_front();
        }
        if pending
            .spline_remaining
            .front()
            .is_some_and(|build| build.mesh.is_empty())
        {
            return;
        }
        let next_spline_bytes = pending
            .spline_remaining
            .front()
            .map(|build| build.mesh.estimated_gpu_bytes());
        if let Some(next_spline_bytes) = next_spline_bytes {
            match uploads.try_reserve_registration(next_spline_bytes) {
                Ok(false) => return,
                Err(error) => {
                    let detail = format!("地图{map_id}的Clipmap样条水GPU分帧预算失败：{error}");
                    discard_pending(
                        &mut commands,
                        &mut meshes,
                        &mut dynamic_materials,
                        &mut uploads,
                        &mut runtime,
                    );
                    state.dirty = false;
                    status.phase = DynamicWaterPhase::Failed;
                    status.detail = detail;
                    error!("{}", status.detail);
                    return;
                }
                Ok(true) => {
                    let build = pending
                        .spline_remaining
                        .pop_front()
                        .expect("上方已读取样条候选队首");
                    pending.spline_triangle_count += stage_water_surface_chunk(
                        &mut commands,
                        &mut meshes,
                        &mut dynamic_materials,
                        &mut pending.spline_staged,
                        build,
                        time.elapsed_secs(),
                    );
                }
            }
        }
        if !pending.spline_remaining.is_empty() {
            status.phase = DynamicWaterPhase::Building;
            status.detail = format!(
                "正在按每帧一张Mesh登记Clipmap河流/急流/瀑布：已登记{}张，剩余{}张",
                pending.spline_staged.len(),
                pending.spline_remaining.len()
            );
            return;
        }
        if !pending.spline_staged.is_empty() && pending.spline_ticket.is_none() {
            for staged in &pending.spline_staged {
                if let Err(error) =
                    staged.rebase_material(&mut dynamic_materials, time.elapsed_secs())
                {
                    let detail = format!("地图{map_id}的Clipmap样条水材质重置失败：{error}");
                    discard_pending(
                        &mut commands,
                        &mut meshes,
                        &mut dynamic_materials,
                        &mut uploads,
                        &mut runtime,
                    );
                    state.dirty = false;
                    status.phase = DynamicWaterPhase::Failed;
                    status.detail = detail;
                    error!("{}", status.detail);
                    return;
                }
            }
            pending.spline_warmup_entities = pending
                .spline_staged
                .iter()
                .map(|staged| {
                    let entity = spawn_staged_water_warmup(&mut commands, staged);
                    commands
                        .entity(entity)
                        .insert(DespawnOnExit(ScenePhase::Active));
                    entity
                })
                .collect();
            pending.spline_ticket =
                Some(uploads.submit(&pending.spline_staged, &pending.spline_warmup_entities));
        }
        if pending
            .spline_ticket
            .is_some_and(|ticket| receipts.has_failed(ticket))
        {
            let detail =
                format!("地图{map_id}的Clipmap河流/急流/瀑布透明PBR管线编译失败；保留上一完整水域");
            discard_pending(
                &mut commands,
                &mut meshes,
                &mut dynamic_materials,
                &mut uploads,
                &mut runtime,
            );
            state.dirty = false;
            status.phase = DynamicWaterPhase::Failed;
            status.detail = detail;
            error!("{}", status.detail);
            return;
        }
        if pending
            .spline_ticket
            .is_some_and(|ticket| !receipts.is_ready(ticket))
        {
            status.phase = DynamicWaterPhase::Building;
            status.detail = format!(
                "地图{map_id}的{}张Clipmap样条水正在等待RenderApp连续GPU准备回执",
                pending.spline_staged.len()
            );
            return;
        }

        let needs_surface = clipmap_batch_requires_ground(pending.registered.len());
        let snapshot = ground
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.map_id == *map_id && snapshot.ready);
        let source = sources.single().ok();
        let height = source.and_then(|(_, _, surface, fallback)| {
            source_height(&surface_materials, &fallback_materials, surface, fallback)
        });
        let density=manifest.terrain.kind==MapTerrainKind::Voxel;
        if needs_surface && (snapshot.is_none() || (!density && (source.is_none() || height.is_none()))) {
            status.phase = DynamicWaterPhase::Loading;
            status.detail = "正在等待Clipmap地形与静水使用同一完整高度修订".to_owned();
            return;
        }

        if density && needs_surface {
            let snapshot=snapshot.expect("原生静水已取得真实BVH");
            let batch=pending.static_batch.get_or_insert_with(||StaticBatch::new(&mut commands,snapshot.revision));
            if let Some(layer)=pending.registered.get(batch.layers.len()) {
                static_batch::stage_density_layer(&mut commands,&mut clip_materials,batch,layer,map_id,
                    (Vec2::from_array(manifest.world_min),Vec2::from_array(manifest.world_max)),snapshot,&fallback,time.elapsed_secs());
                status.phase=DynamicWaterPhase::Building;
                status.detail="正在准备原生静水作者层；真实水深网格随后共同GPU采用".into();
                return;
            }
        }
        if let (Some(snapshot), Some((mesh, transform, _, _)), Some(height)) =
            (snapshot, source, height.as_ref())
        {
            if pending
                .static_batch
                .as_ref()
                .is_some_and(|batch| batch.height_revision != snapshot.revision)
            {
                let old = pending.static_batch.take().expect("已经核对静水候选");
                old.retire(&mut commands, &mut runtime, &mut uploads);
                return; // 高度换代后按完整新身份重做，不发布混合高度的候选。
            }
            if needs_surface {
                let batch = pending
                    .static_batch
                    .get_or_insert_with(|| StaticBatch::new(&mut commands, snapshot.revision));
                if let Some(ticket) = batch.ticket {
                    if receipts.has_failed(ticket) {
                        discard_pending(
                            &mut commands,
                            &mut meshes,
                            &mut dynamic_materials,
                            &mut uploads,
                            &mut runtime,
                        );
                        state.dirty = false;
                        status.phase = DynamicWaterPhase::Failed;
                        status.detail =
                            "Clipmap静水GPU材质或透明管线准备失败；保留上一完整水域".into();
                        return;
                    }
                    if !receipts.is_ready(ticket) {
                        return;
                    }
                    uploads.retire(ticket);
                    batch.ticket = None;
                    if let Some(entity) = batch.warmups.pop_front() {
                        commands.entity(entity).try_despawn();
                    }
                }
                if let Some(layer) = pending.registered.get(batch.layers.len()) {
                    static_batch::stage_layer(
                        &mut commands,
                        &mut clip_materials,
                        batch,
                        layer,
                        map_id,
                        (
                            Vec2::from_array(manifest.world_min),
                            Vec2::from_array(manifest.world_max),
                        ),
                        snapshot,
                        mesh,
                        *transform,
                        height,
                        time.elapsed_secs(),
                    );
                    let material = batch
                        .layers
                        .back()
                        .expect("本帧登记了一层")
                        .material
                        .id()
                        .untyped();
                    batch.ticket = Some(uploads.submit_assets(
                        vec![mesh.0.id()],
                        vec![material],
                        vec![*batch.warmups.back().expect("本层预热实体存在")],
                    ));
                    status.phase = DynamicWaterPhase::Building;
                    status.detail = format!(
                        "正在逐帧准备Clipmap静水：{}/{}层",
                        batch.layers.len(),
                        pending.registered.len()
                    );
                    return;
                }
            }
        }

        let mut pending = runtime
            .pending
            .take()
            .expect("上方已确认Clipmap水域候选存在");
        let generation = pending.identity.1;
        let draft_revision = pending.identity.2;
        if let Some(ticket) = pending.spline_ticket.take() {
            uploads.retire(ticket);
        }
        for entity in pending.spline_warmup_entities.drain(..) {
            commands.entity(entity).try_despawn();
        }
        let next_static = pending.static_batch.take();
        let mut next_spline = Vec::with_capacity(pending.spline_staged.len());
        for staged in pending.spline_staged.drain(..) {
            let spawn = spawn_staged_water_surface(
                &mut commands,
                map_id.clone(),
                PaintedWaterPurpose::Runtime,
                staged,
            );
            commands.entity(spawn.entity).insert((
                ClipmapSplineWaterSurface,
                DespawnOnExit(ScenePhase::Active),
            )); // 世界XYZ Ribbon独立于移动环带，但仍属于同一Clipmap水域原子批次。
            next_spline.push(InstalledSplineLayer { spawn });
        }
        // Bevy 0.19 提取仅含 RENDER_WORLD 用法的 Mesh 后会释放主世界 CPU 索引；
        // 水体诊断必须读取与正式地形实体同代的编译计划，避免 F12 采用批次时反读 Mesh panic。
        let source_triangles = clipmap_plan_triangle_count(
            clipmap_runtime
                .blade_geometry(map_id)
                .map(|(_, plan)| plan.index_count),
        );
        remove_installed(
            &mut commands,
            &mut meshes,
            &mut clip_materials,
            &mut dynamic_materials,
            &mut runtime,
            &mut uploads,
        );
        if let Some(batch) = &next_static {
            batch.publish(&mut commands);
        }
        let native_static=density && next_static.as_ref().is_some_and(|batch|!batch.layers.is_empty());
        runtime.static_installed = next_static;
        runtime.spline_installed = next_spline;
        runtime.installed_map_id = Some(map_id.clone());
        state.installed_map_id = Some(map_id.clone());
        status.phase = if native_static {DynamicWaterPhase::Building} else {DynamicWaterPhase::Ready};
        status.map_id = Some(map_id.clone());
        status.revision = status.revision.wrapping_add(1);
        status.desired_generation = generation;
        if !native_static {status.applied_generation = generation;}
        status.input_draft_revision = draft_revision;
        status.height_binding_revision = snapshot.map_or([0; 3], |snapshot| snapshot.revision);
        let static_count = runtime
            .static_installed
            .as_ref()
            .map_or(0, |batch| batch.layers.len());
        status.layer_count = static_count.saturating_add(runtime.spline_installed.len());
        status.triangle_count = source_triangles
            .saturating_mul(static_count)
            .saturating_add(pending.spline_triangle_count);
        status.detail = format!(
            "Clipmap水域已原子采用：静水{static_count}层、河流{}条（急流{}段）、瀑布{}条，共{}个三角形",
            pending.river_count,
            pending.rapid_segment_count,
            pending.waterfall_count,
            status.triangle_count
        );
        crate::retire_water_cpu(&mut commands, pending);
        return;
    }

    if runtime.task.is_some() || !state.dirty {
        return;
    }
    let identity = (map_id.clone(), state.generation, draft_revision);
    if runtime.attempted.as_ref() == Some(&identity) {
        return;
    }
    if manifest.schema_version != MAP_SCHEMA_VERSION
        || manifest.id != *map_id
        || !manifest.is_data_driven()
        || manifest.world_bounds().is_none()
    {
        fail(
            &mut state,
            &mut status,
            &mut runtime,
            format!("地图{map_id}不是有效的Geometry Clipmap清单"),
        );
        return;
    }

    let (definition, draft_sources, draft_images) = if let Some(snapshot) = draft.for_map(map_id) {
        (
            snapshot.definition.clone(),
            Some(&snapshot.sources),
            Some(&snapshot.masks),
        )
    } else {
        let Some(water_handle) = handles.water.as_ref() else {
            remove_installed(
                &mut commands,
                &mut meshes,
                &mut clip_materials,
                &mut dynamic_materials,
                &mut runtime,
                &mut uploads,
            );
            state.dirty = false;
            state.installed_map_id = Some(map_id.clone());
            status.phase = DynamicWaterPhase::Disabled;
            status.desired_generation = 0;
            status.applied_generation = 0;
            status.height_binding_revision = [0; 3];
            status.layer_count = 0;
            status.triangle_count = 0;
            status.detail = format!("地图{map_id}未声明水域文件");
            return;
        };
        let Some(water) = waters.get(water_handle) else {
            status.phase = DynamicWaterPhase::Loading;
            status.detail = format!("正在加载地图{map_id}的水域JSON");
            return;
        };
        (water.clone(), None, None)
    };
    let validation = validate_clipmap_water_input(&definition, map_id, draft_sources.is_some());
    if let Err(error) = validation {
        fail(&mut state, &mut status, &mut runtime, error);
        return;
    }

    let paths = definition
        .layers
        .iter()
        .map(|layer| (layer.id.clone(), layer.mask_source_path.clone()))
        .collect::<BTreeMap<_, _>>();
    if draft_sources.is_none() && state.mask_paths != paths {
        state.mask_handles = paths
            .iter()
            .map(|(id, path)| {
                (
                    id.clone(),
                    asset_server.load::<MapWaterMaskAsset>(path.clone()),
                )
            })
            .collect();
        state.mask_asset_ids = state.mask_handles.values().map(Handle::id).collect();
        state.mask_paths = paths;
    }

    let mut frozen = Vec::with_capacity(definition.layers.len());
    for layer in &definition.layers {
        let descriptor = if layer.preset_id.is_empty() {
            WaterSurfaceDescriptor::from_layer(layer)
        } else {
            let Some(presets) = presets.as_deref().filter(|presets| presets.revision > 0) else {
                status.phase = DynamicWaterPhase::Loading;
                status.detail = format!("正在等待静水体{}引用的水体预设目录", layer.label);
                return;
            };
            let Some(preset) = presets.resolve(&layer.preset_id) else {
                fail(
                    &mut state,
                    &mut status,
                    &mut runtime,
                    format!("静水体{}引用不存在的水体预设{}", layer.id, layer.preset_id),
                );
                return;
            };
            let Some(texture_handles) = presets.image_handles(&preset.id) else {
                fail(
                    &mut state,
                    &mut status,
                    &mut runtime,
                    format!("水体预设{}缺少微法线句柄快照", preset.id),
                );
                return;
            };
            for handle in texture_handles.iter().flatten() {
                if matches!(asset_server.load_state(handle.id()), LoadState::Failed(_)) {
                    fail(
                        &mut state,
                        &mut status,
                        &mut runtime,
                        format!("水体预设{}的微法线加载失败", preset.id),
                    );
                    return;
                }
                if !asset_server.is_loaded_with_dependencies(handle.id()) {
                    status.phase = DynamicWaterPhase::Loading;
                    status.detail = format!("正在加载水体预设{}的微法线", preset.label);
                    return;
                }
            }
            WaterSurfaceDescriptor::from_layer_with_preset(layer, preset, texture_handles.clone())
        };
        let (pixels, draft_image) =
            if let (Some(sources), Some(images)) = (draft_sources, draft_images) {
                let Some(pixels) = sources.get(&layer.id) else {
                    fail(
                        &mut state,
                        &mut status,
                        &mut runtime,
                        format!("Clipmap静水草稿层{}缺少R8快照", layer.id),
                    );
                    return;
                };
                let Some(image) = images.get(&layer.id) else {
                    fail(
                        &mut state,
                        &mut status,
                        &mut runtime,
                        format!("Clipmap静水草稿层{}缺少GPU图片", layer.id),
                    );
                    return;
                };
                (Arc::clone(pixels), Some(image.clone()))
            } else {
                let Some(handle) = state.mask_handles.get(&layer.id) else {
                    return;
                };
                if matches!(asset_server.load_state(handle.id()), LoadState::Failed(_)) {
                    fail(
                        &mut state,
                        &mut status,
                        &mut runtime,
                        format!("Clipmap静水层{}的R8作者源加载失败", layer.id),
                    );
                    return;
                }
                let Some(mask) = masks.get(handle) else {
                    status.phase = DynamicWaterPhase::Loading;
                    status.detail = format!("正在加载Clipmap静水层{}的R8作者源", layer.label);
                    return;
                };
                (mask.shared_pixels(), None)
            };
        let expected = (layer.resolution[0] as usize).checked_mul(layer.resolution[1] as usize);
        if expected != Some(pixels.len()) {
            fail(
                &mut state,
                &mut status,
                &mut runtime,
                format!("Clipmap静水层{}的R8尺寸与JSON不一致", layer.id),
            );
            return;
        }
        frozen.push(FrozenLayer {
            layer: layer.clone(),
            descriptor,
            pixels,
            draft_image,
        });
    }

    let rivers = definition
        .rivers
        .into_iter()
        .filter(|river| river.points.len() >= 2)
        .collect::<Vec<_>>();
    let waterfalls = definition
        .waterfalls
        .into_iter()
        .filter(|waterfall| waterfall.points.len() >= 2)
        .collect::<Vec<_>>();
    let river_count = rivers.len();
    let rapid_segment_count = rivers
        .iter()
        .map(|river| {
            river
                .points
                .iter()
                .take(river.points.len().saturating_sub(1))
                .filter(|point| point.outgoing == MapWaterSegmentKind::Rapid)
                .count()
        })
        .sum();
    let waterfall_count = waterfalls.len();
    runtime.attempted = Some(identity.clone());
    runtime.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let layers = frozen
            .into_iter()
            .map(|frozen| {
                let bounds = coverage_bounds(&frozen.pixels, frozen.layer.resolution);
                PreparedLayer {
                    layer: frozen.layer,
                    descriptor: frozen.descriptor,
                    pixels: frozen.pixels,
                    coverage_bounds: bounds,
                    draft_image: frozen.draft_image,
                }
            })
            .collect();
        let result = build_dynamic_spline_water_network(rivers, waterfalls).map(|spline_builds| {
            PreparedBatch {
                layers,
                spline_builds,
                river_count,
                rapid_segment_count,
                waterfall_count,
            }
        });
        (identity, result)
    }));
    state.dirty = false;
    status.phase = DynamicWaterPhase::Building;
    status.detail = format!("正在后台准备地图{map_id}的Clipmap静水与样条水完整批次");
}

/// 视角吸附更新已采用层的Transform和高度绑定，复用同一环带Mesh；内容代次保持不变。
#[allow(clippy::too_many_arguments)]
pub(crate) fn follow_clipmap_water(
    active: InstanceRes<ActiveDynamicMap>,
    ground: InstanceRes<DynamicClipmapGroundSurface>,
    surface_materials: Res<Assets<SparseClipmapSurfaceMaterial>>,
    fallback_materials: Res<Assets<ClipmapHeightMaterial>>,
    sources: InstanceQuery<
        (
            &Mesh3d,
            &Transform,
            Option<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
            Option<&MeshMaterial3d<ClipmapHeightMaterial>>,
        ),
        With<DynamicSparseClipmapPrimitive>,
    >,
    mut targets: InstanceQuery<
        (
            Option<&mut Mesh3d>,
            &mut Transform,
            &mut Visibility,
            &mut ClipmapWaterSurface,
            &mut DynamicWaterSurface,
        ),
        Without<DynamicSparseClipmapPrimitive>,
    >,
    mut materials: ResMut<Assets<ClipmapWaterMaterial>>,
    runtime: InstanceRes<ClipmapWaterRuntime>,
    mut status: InstanceResMut<DynamicWaterStatus>,
) {
    let Some(installed_batch) = runtime.static_installed.as_ref() else {
        return;
    };
    let snapshot = ground
        .snapshot
        .as_ref()
        .filter(|snapshot| snapshot.ready && active.map_id.as_ref() == Some(&snapshot.map_id));
    let source = sources.single().ok();
    let height = source.and_then(|(_, _, surface, fallback)| {
        source_height(&surface_materials, &fallback_materials, surface, fallback)
    });
    let density=snapshot.is_some_and(|snapshot|snapshot.is_density_surface());
    let ready = snapshot.is_some() && (density || source.is_some() && height.is_some());
    for installed in &installed_batch.layers {
        let Ok((mut mesh, mut transform, mut visibility, mut surface, mut interaction)) =
            targets.get_mut(installed.entity)
        else {
            continue;
        };
        if !ready {
            *visibility = Visibility::Hidden;
            continue;
        }
        let snapshot = snapshot.expect("就绪门保证地表快照存在");
        if density {
            surface.height_revision=snapshot.revision;
            interaction.sync_clipmap_footprint(&surface,snapshot);
            *visibility=Visibility::Hidden; // 作者层没有共享环带，实际三维块拥有唯一draw。
            continue;
        }
        let (source_mesh, source_transform, _, _) = source.expect("就绪门保证环带存在");
        if let Some(mesh)=mesh.as_mut() && mesh.0 != source_mesh.0 {
            **mesh = source_mesh.clone();
        }
        if *transform != *source_transform {
            *transform = *source_transform;
        }
        *visibility = Visibility::Inherited;
        surface.height_revision = snapshot.revision;
        interaction.sync_clipmap_footprint(&surface, snapshot); // 高度换代不清空玩家波纹历史，只替换同代查询绑定。
        if let Some(mut material) = materials.get_mut(&installed.material) {
            material.extension.sync_height(
                height.as_ref().expect("就绪门保证高度绑定存在"),
                snapshot.max_mip(),
                snapshot.missing_pages,
            );
        }
    }
    if ready {
        let revision = snapshot.expect("就绪门保证地表快照存在").revision;
        status.height_binding_revision = revision;
        if status.applied_generation == status.desired_generation {
            status.phase = DynamicWaterPhase::Ready;
        }
    } else if status.phase == DynamicWaterPhase::Ready {
        status.phase = DynamicWaterPhase::Loading;
        status.detail = "Clipmap高度换代尚未完整采用，静水已暂时隐藏".to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::dynamic_map::clipmap::test_ground_snapshot;
    use feathrise_map_content::{MapWaterRiver, MapWaterSplinePoint};

    fn surface(mask: Vec<u8>) -> ClipmapWaterSurface {
        ClipmapWaterSurface {
            map_id: MapId::parse("MAP-ground-test").unwrap(),
            layer_id: "lake".to_owned(),
            resolution: [3, 3],
            world_min: Vec2::ZERO,
            world_max: Vec2::new(96.0, 64.0),
            water_level: 20.0,
            minimum_depth: 0.25,
            height_revision: [1, 2, 3],
            coverage_bounds: [0, 0, 3, 3],
            mask: Arc::from(mask),
            volume: false,
            published: None,
        }
    }

    fn spline_point(id: &str, position: [f32; 3]) -> MapWaterSplinePoint {
        MapWaterSplinePoint {
            id: id.to_owned(),
            position,
            width: 4.0,
            depth: 1.0,
            flow_speed: 2.0,
            outgoing: MapWaterSegmentKind::River,
        }
    }

    fn river(points: Vec<MapWaterSplinePoint>) -> MapWaterRiver {
        MapWaterRiver {
            id: "river-test".to_owned(),
            label: "测试河流".to_owned(),
            source_layer_id: None,
            target_layer_id: None,
            points,
            base_color_srgba: [0.05, 0.24, 0.29, 0.72],
            perceptual_roughness: 0.22,
        }
    }

    fn discard_pending_test_system(
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<DynamicWaterMaterial>>,
        mut uploads: ResMut<WaterGpuUploadQueue>,
        mut runtime: InstanceResMut<ClipmapWaterRuntime>,
    ) {
        discard_pending(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut uploads,
            &mut runtime,
        );
    }

    #[test]
    fn cpu_coverage_uses_gpu_world_mapping_and_exclusive_bounds() {
        let mut mask = vec![0; 9];
        mask[4] = 255;
        let mut surface = surface(mask);
        surface.coverage_bounds = [1, 1, 2, 2];
        assert_eq!(surface.coverage_at(Vec2::new(48.0, 32.0)), 1.0);
        assert_eq!(surface.coverage_at(Vec2::new(-0.01, 32.0)), 0.0);
        assert_eq!(surface.coverage_at(Vec2::new(48.0, 64.01)), 0.0);
        let (min, max) = surface.world_coverage_bounds();
        assert_eq!(min, Vec2::ZERO);
        assert_eq!(max, surface.world_max);
    }

    #[test]
    fn interaction_requires_same_height_revision_real_depth_and_r8() {
        let ground = test_ground_snapshot();
        let point = Vec2::new(47.2, 31.3);
        let ClipmapGroundHit::Hit(hit) = ground.sample_surface(point) else {
            panic!("测试点必须命中正式Clipmap三角面");
        };
        let mut surface = surface(vec![255; 9]);
        surface.water_level = hit.y + 1.0;
        assert!((surface.surface_depth_at(&ground, point).unwrap() - 1.0).abs() < 1.0e-5);
        surface.minimum_depth = 1.01;
        assert!((surface.surface_depth_at(&ground, point).unwrap() - 1.0).abs() < 1.0e-5);
        surface.minimum_depth = 0.25;
        surface.height_revision[2] += 1;
        assert_eq!(surface.surface_depth_at(&ground, point), None);
        surface.height_revision = ground.revision;
        surface.mask = Arc::from(vec![0; 9]);
        assert_eq!(surface.surface_depth_at(&ground, point), None);
        surface.mask = Arc::from(vec![255; 9]);
        let mut waiting = ground.clone();
        waiting.ready = false;
        assert_eq!(surface.surface_depth_at(&waiting, point), None);
    }

    #[test]
    fn positive_shallow_water_is_visible_but_zero_and_negative_depth_are_rejected() {
        let ground = test_ground_snapshot();
        let point = Vec2::new(47.2, 31.3);
        let ClipmapGroundHit::Hit(bed) = ground.sample_surface(point) else {
            panic!("测试点必须命中同代正式显示三角面");
        };
        let mut surface = surface(vec![255; 9]);
        surface.minimum_depth = 0.8; // 波幅安全长度大于浅水深度，仍不能把岸边可见足迹裁掉。
        surface.water_level = bed.y + 0.1;
        assert!((surface.surface_depth_at(&ground, point).unwrap() - 0.1).abs() < 1.0e-5);
        assert!(surface.contains_surface(&ground, point));
        assert_eq!(surface.minimum_depth, 0.8);

        for offset in [0.0, -0.1] {
            surface.water_level = bed.y + offset;
            assert_eq!(surface.surface_depth_at(&ground, point), None,
                "零水深或地面高于水位时不能授权水面：偏移{offset}");
            assert!(!surface.contains_surface(&ground, point));
        }
    }

    #[test]
    fn outdoor_depth_and_shoreline_follow_displayed_coarse_lod_not_mip_zero() {
        let ground = test_ground_snapshot();
        // 此点位于粗显示环带；非线性夹具保证正式三角面与基础作者Mip0高度有可测差异。
        let point = Vec2::new(72.3, 45.7);
        let ClipmapGroundHit::Hit(displayed) = ground.sample_surface(point) else {
            panic!("测试点必须命中正式粗LOD显示三角面");
        };
        let ClipmapGroundHit::Hit(author) = ground.sample_authoritative_surface(point) else {
            panic!("同代基础作者页必须完整");
        };
        assert!((displayed.y - author.y).abs() > 0.01);

        let mut surface = surface(vec![255; 9]);
        surface.water_level = displayed.y + 0.125;
        assert!(
            (surface.surface_depth_at(&ground, point).unwrap() - 0.125).abs() < 1.0e-5
        );
        // 水位置于两种地面之间时，两种算法会给出相反的岸线授权，必须服从实际可见三角面。
        surface.water_level = (displayed.y + author.y) * 0.5;
        let displayed_depth = surface.water_level - displayed.y;
        let expected = (displayed_depth > 0.).then_some(displayed_depth);
        assert_eq!(surface.surface_depth_at(&ground, point), expected);
        assert_eq!(surface.contains_surface(&ground, point), expected.is_some());
    }

    #[test]
    fn authoring_accepts_empty_and_single_point_splines_but_formal_assets_do_not() {
        let map_id = MapId::parse("MAP-clipmap-water-draft").unwrap();
        let mut definition = MapWaterAsset {
            schema_version: MAP_SCHEMA_VERSION,
            map_id: map_id.clone(),
            layers: Vec::new(),
            rivers: Vec::new(),
            waterfalls: Vec::new(),
        };
        assert!(validate_clipmap_water_input(&definition, &map_id, true).is_ok());
        definition
            .rivers
            .push(river(vec![spline_point("point-a", [4.0, 3.0, 2.0])]));
        assert!(validate_clipmap_water_input(&definition, &map_id, true).is_ok());
        assert!(validate_clipmap_water_input(&definition, &map_id, false).is_err());
    }

    #[test]
    fn spline_only_batch_does_not_wait_for_clipmap_ground() {
        assert!(!clipmap_batch_requires_ground(0));
        assert!(clipmap_batch_requires_ground(1));
    }

    #[test]
    fn static_triangle_diagnostic_uses_compiled_plan_count_without_mesh_cpu_data() {
        assert_eq!(clipmap_plan_triangle_count(Some(18)), 6);
        assert_eq!(clipmap_plan_triangle_count(None), 0);
        // 测试只提供正式计划里的索引数，不创建Mesh；防止诊断再次依赖已被RenderWorld提取的CPU数据。
    }

    #[test]
    fn discarding_stale_spline_candidate_releases_warmup_and_assets() {
        let build = build_dynamic_spline_water_network(
            vec![river(vec![
                spline_point("point-a", [0.0, 2.0, 0.0]),
                spline_point("point-b", [8.0, 1.0, 8.0]),
            ])],
            Vec::new(),
        )
        .unwrap()
        .pop()
        .unwrap();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<DynamicWaterMaterial>::default();
        let staged = crate::shared::large_map::stage_water_surface_assets(
            &mut meshes,
            &mut materials,
            build.descriptor,
            build.mesh,
            0.0,
        );
        let mesh_id = staged.mesh_handle().id();
        let material_id = staged.material_handle().id();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        crate::shared::large_map::install_water_retirement(&mut app);
        let warmup = app.world_mut().spawn_empty().id();
        let mut uploads = WaterGpuUploadQueue::default();
        let ticket = uploads.submit(std::slice::from_ref(&staged), &[warmup]);
        let map_id = MapId::parse("MAP-clipmap-stale-water").unwrap();
        app.insert_resource(meshes)
            .insert_resource(materials)
            .insert_resource(uploads)
            .insert_resource(ClipmapWaterRuntime {
                pending: Some(PendingBatch {
                    identity: (map_id, 7, Some(11)),
                    layers: VecDeque::new(),
                    registered: Vec::new(),
                    static_batch: None,
                    image_cursor: 0,
                    spline_remaining: VecDeque::new(),
                    spline_staged: vec![staged],
                    spline_warmup_entities: vec![warmup],
                    spline_ticket: Some(ticket),
                    spline_triangle_count: 2,
                    river_count: 1,
                    rapid_segment_count: 0,
                    waterfall_count: 0,
                }),
                ..default()
            })
            .add_systems(Update, discard_pending_test_system);
        app.update();

        assert!(
            app.world()
                .resource::<ClipmapWaterRuntime>()
                .pending
                .is_none()
        );
        assert!(app.world().get_entity(warmup).is_err());
        app.update(); // 退役先隐藏，下一帧First只回收一个上传块。
        assert!(
            app.world()
                .resource::<Assets<Mesh>>()
                .get(mesh_id)
                .is_none()
        );
        assert!(
            app.world()
                .resource::<Assets<DynamicWaterMaterial>>()
                .get(material_id)
                .is_none()
        );
    }
}
