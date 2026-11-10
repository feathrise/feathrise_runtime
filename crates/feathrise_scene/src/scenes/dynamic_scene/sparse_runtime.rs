//! 新建场景的稀疏清单采用与相机中心 Geometry Clipmap 显示。
//!
//! 这里只在后台构造经过16MiB预算预检的可变环带组合网格；场景格点数绝不直接成为 Mesh 分配大小。
//! 高度页和局部碰撞由独立系统构建，三者均核对 SceneId 与当前地形根。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use std::time::Instant; // 后台拓扑构建墙钟时间只写诊断，不参与候选身份。

use bevy::asset::{AssetEvent, LoadState}; // 清单热更和失败均来自 Bevy 资产系统。
use bevy::camera::visibility::NoFrustumCulling; // GPU 位移后的真实高度可能超出平面 Mesh 的 CPU AABB。
use bevy::ecs::system::SystemParam; // 将GPU登记资源收拢，保持系统参数低于Bevy元组上限。
use bevy::gltf::{GltfMaterialName, GltfMeshName}; // 复用场景地表唯一正式源的身份契约。
use bevy::prelude::*; // Bevy ECS、材质和可见网格。
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready}; // 生成组合网格和释放迟到大结果都避开主帧。
use feathrise_scene_content::{
    ActiveDynamicScene, CompiledDisplayLodPlan, SceneId, SceneManifestAsset,
    SceneTerrainDisplayLod, SceneTerrainDisplayLodPhase, SceneTerrainDisplayLodStatus, SceneTerrainKind,
    SparseHeightManifestAsset, SparseHeightSpec, compile_display_lod_plan,
    default_display_lods_for_spec,
}; // 稀疏作者清单与场景总清单必须相互验证。

use super::clipmap::{ClipmapBounds, build_display_lod_mesh, snapped_display_focus}; // 可变环带几何和世界固定焦点吸附。
use super::resources::{DynamicSceneRuntimePhase, DynamicSceneRuntimeState, DynamicSceneRuntimeStatus};
use super::sparse_lod::{ClipmapViews, ViewLodKey, compile_view_plan};
use crate::{DynamicSceneTerrainPrimitive, DynamicSparseClipmapPrimitive, WaterGpuUploadQueue};

/// 仅供本域区分正式 Clipmap 显示和旧 GLB 显示。
#[derive(Component)]
pub(super) struct SparseClipmapDisplay;

/// F12离屏作者相机的显式登记；高度Clipmap按此相机的实际机位与作者档位决定显示LOD。
/// 轨道观察中心继续供体积流送等近区消费者使用，不能替代高度网格的真实相机距离。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct DynamicClipmapAuthoringView {
    pub focus_xz: Vec2, // 世界 X/Z；由 F12 相机同步系统随轨道中心更新。
    pub focus_y: f32,   // 体积流送需要完整三维观察中心，不能只跟随高空相机机位。
}

/// 一次后台网格任务的完整场景、根和编译计划身份。
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DisplayBuildIdentity {
    pub(super) scene_id: SceneId, // 切换场景后即使配置相同也不得采用旧结果。
    pub(super) root: Entity,  // 同场景重建场景根同样使旧任务失效。
    pub(super) plan: CompiledDisplayLodPlan, // 网格与分页后续必须共享完整量化计划。
}

/// 后台只返回连续CPU Mesh和诊断；主世界尚未登记任何GPU资产。
struct BuiltDisplayMesh {
    identity: DisplayBuildIdentity, // 完成后与唯一最新期望逐值比较。
    mesh: Mesh,                     // 受检16MiB以内的一份组合Mesh。
    build_milliseconds: f32,        // 不含排队、资产登记或GPU预热等待。
}

/// 已进入Assets、尚未与匹配高度页和材质完成GPU预热的唯一网格候选。
pub(super) struct RegisteredDisplayMesh {
    pub(super) identity: DisplayBuildIdentity, // 页集与候选材质必须逐值匹配该计划。
    pub(super) mesh: Handle<Mesh>,             // 候选期间与正式Mesh并存至多一份。
    pub(super) build_milliseconds: f32,        // 最终采用时一并发布诊断。
    pub(super) registered_at: Instant,         // 30秒GPU预热超时从首次大资产登记开始。
}

/// 网格任务单飞；失败热更和迟到完成不能替换上一完整显示。
#[derive(Resource, Default)]
pub(super) struct SparseClipmapRuntime {
    scene_id: Option<SceneId>,                                // 当前目标场景身份。
    root: Option<Entity>,                                 // 父级场景根，切换场景后旧任务结果不得采用。
    path: Option<String>,                                 // 被场景总清单声明的稀疏清单路径。
    manifest: Option<Handle<SparseHeightManifestAsset>>,  // 强句柄维持 AssetServer 生命周期。
    task: Option<Task<Result<BuiltDisplayMesh, String>>>, // 至多一个后台拓扑构建任务。
    release_task: Option<Task<()>>, // 至多一个后台析构任务，避免迟到16MiB结果在主线程释放。
    pending_cpu: Option<BuiltDisplayMesh>, // 等待共享每帧大候选额度，不提前登记Assets。
    building: Option<DisplayBuildIdentity>, // 当前在途身份；新保存只更新desired而不丢句柄。
    pub(super) desired: Option<DisplayBuildIdentity>, // 最新场景清单要求的完整显示计划。
    pub(super) prepared: Option<RegisteredDisplayMesh>, // 已登记但仍未正式采用的唯一Mesh。
    pub(super) applied: Option<DisplayBuildIdentity>, // 当前正式Mesh已经采用的完整身份。
    pub(super) failed: Option<DisplayBuildIdentity>, // 同一失败身份等待新保存，不在稳定帧无限重试。
    bounds: Option<ClipmapBounds>,  // 相机贴边和场景外裁切共用范围。
    pub(super) display: Option<Entity>, // 唯一正式地形可见实体。
    spec: Option<SparseHeightSpec>, // 当前受检显示规格；热更边界必须重进场景。
    authored_plan: Option<CompiledDisplayLodPlan>, // 作者约束只在清单换代时校验；相机派生不能写回它。
    view_lod: Option<ViewLodKey>, // 作者有效档位及覆盖包络稳定时复用现有拓扑。
    pub(super) requested_focus: Option<Vec2>, // 细环共用实际世界相机；全源末档使用稳定均匀布局。
    manifest_dirty: bool,           // 仅首载或相关清单事件校验，不每帧扫描索引。
}

#[derive(SystemParam)]
pub(super) struct SparseDisplayUploadAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>, // 正式与唯一候选组合Mesh。
    white_materials: ResMut<'w, Assets<StandardMaterial>>, // 首载隐藏源的基础PBR。
    uploads: Option<ResMut<'w, WaterGpuUploadQueue>>, // 与水面和高度页共享每帧额度。
}

/// LOD只取同图的固定大小范围摘要；高山、深谷不能再被当成源Y=0平面。
#[derive(SystemParam)]
pub(super) struct SparseDisplayHeights<'w> {
    live: InstanceRes<'w, feathrise_scene_content::SparseHeightLiveOverride>,
    pages: InstanceRes<'w, super::clipmap::SparseHeightPageRuntime>,
}

impl SparseDisplayHeights<'_> {
    fn bounds(&self, map: &SceneId, mut bounds: [f32; 2]) -> [f32; 2] {
        for extra in [
            self.pages.adopted_height_bounds(map),
            self.live.height_bounds().filter(|_| self.live.scene_id() == Some(map)),
        ].into_iter().flatten() {
            bounds = [bounds[0].min(extra[0]), bounds[1].max(extra[1])];
        }
        bounds // 保守合并旧完整显示与新作者根，不遍历页、Mesh或碰撞。
    }
}

/// 宿主只把并发席位归还给真正结清后台工作的实例。
impl SparseClipmapRuntime {
    pub(super) fn stitch_step(&self,applied_focus:Vec2) -> Vec4 {
        let Some(spec)=&self.spec else{return Vec4::ZERO};
        let min=Vec2::from_array(spec.world_min);let max=Vec2::from_array(spec.world_max);
        let base=Vec2::new(spec.actual_spacing_meters[0] as f32,spec.actual_spacing_meters[1] as f32);
        let mut result=Vec4::ZERO;
        for (identity,focus) in self.applied.iter().map(|p|(p,applied_focus)).chain(self.desired.iter().map(|p|(p,self.requested_focus.unwrap_or(applied_focus)))) {
            for band in &identity.plan.bands {
                let step=base*band.spacing_multiplier as f32;
                let center=min+((focus-min)/step+Vec2::splat(0.5)).floor()*step;
                let outer=Vec2::new(band.outer_half_cells[0] as f32,band.outer_half_cells[1] as f32)*step;
                let inner=Vec2::new(band.inner_half_cells[0] as f32,band.inner_half_cells[1] as f32)*step;
                for edge in 0..4 {
                    let tangent=if edge<2 {1}else{0};let normal=1-tangent;
                    let bound=if edge%2==0 {min[normal]}else{max[normal]};
                    if (bound-center[normal]).abs()>outer[normal]+step[normal]
                        || max[tangent]<center[tangent]-outer[tangent]-step[tangent]
                        || min[tangent]>center[tangent]+outer[tangent]+step[tangent] {continue;}
                    // 完全落在细层洞内的外环不参与此边；两格保护涵盖环带吸附和2:1过渡。
                    if inner.cmpgt(Vec2::ZERO).all() && (bound-center[normal]).abs()<inner[normal]-2.0*step[normal]
                        && min[tangent]>center[tangent]-inner[tangent]+2.0*step[tangent]
                        && max[tangent]<center[tangent]+inner[tangent]-2.0*step[tangent] {continue;}
                    result[edge]=result[edge].max(step[tangent]);
                }
            }
        }result
    }
    pub(in crate::scenes::dynamic_scene) fn background_busy(&self) -> bool {
        self.task.is_some() || self.release_task.is_some()
    }
    pub(in crate::scenes::dynamic_scene) fn background_finished(&self) -> bool {
        self.task.as_ref().is_none_or(|task| task.is_finished()) && self.release_task.as_ref().is_none_or(|task| task.is_finished())
    }
}

impl SparseClipmapRuntime {
    #[cfg(test)]
    pub(in crate::scenes::dynamic_scene) fn adopted_blade_geometry_for_test(map: SceneId, root: Entity, display: Entity, plan: CompiledDisplayLodPlan) -> Self {
        Self { scene_id: Some(map.clone()), root: Some(root), display: Some(display),
            applied: Some(DisplayBuildIdentity { scene_id: map, root, plan }), ..Default::default() }
    }

    /// 体积环带只读已经采用的高度计划，避免新配置与旧高度网格混用。
    pub(super) fn volume_display_lods(&self, map: &SceneId) -> Option<Vec<feathrise_terrain::volume::sparse::paged::VolumeDisplayLod>> {
        let (_, plan)=self.blade_geometry(map)?;
        let spacing=self.spec.as_ref()?.actual_spacing_meters;
        Some(plan.bands.iter().zip(&plan.actual_half_extents_meters).map(|(band, extent)| {
            feathrise_terrain::volume::sparse::paged::VolumeDisplayLod {
                outer_half_extent_meters: extent[0].max(extent[1]),
                spacing_meters: spacing[0].min(spacing[1]) as f32 * band.spacing_multiplier as f32,
            }
        }).collect())
    }
    /// 草地与作者地表查询只读取当前已经原子采用的组合网格计划。
    pub(super) fn blade_geometry(
        &self,
        scene_id: &SceneId,
    ) -> Option<(Entity, &CompiledDisplayLodPlan)> {
        let applied = self.applied.as_ref()?;
        (self.scene_id.as_ref() == Some(scene_id)
            && applied.scene_id == *scene_id
            && self.root == Some(applied.root))
        .then_some((self.display?, &applied.plan)) // 不能让候选计划提前驱动正式草根或笔刷。
    }

    /// 分页器只读取当前场景根的最新编译计划，不能用已采用旧计划冒充新候选。
    pub(super) fn desired_display_plan(
        &self,          // 当前显示LOD单飞状态。
        scene_id: &SceneId, // 分页器正在处理的场景身份。
        root: Entity,   // 分页器正在处理的场景根。
    ) -> Option<&CompiledDisplayLodPlan> {
        self.desired
            .as_ref()
            .filter(|identity| identity.scene_id == *scene_id && identity.root == root)
            .map(|identity| &identity.plan) // 完整计划引用不复制作者列表或环带。
    }
}

/// 轮询唯一后台析构任务；未完成时禁止启动下一次网格构建，因而不会形成隐藏并发。
fn poll_display_mesh_release(clipmap: &mut SparseClipmapRuntime) {
    let completed = clipmap
        .release_task
        .as_mut()
        .is_some_and(|task| check_ready(task).is_some()); // 主线程只做一次非阻塞轮询。
    if completed {
        clipmap.release_task = None; // 完整析构结束后才归还唯一后台槽位。
    }
}

/// 把不再可能采用的大型CPU Mesh交给有界后台槽位析构。
fn release_display_mesh_in_background(clipmap: &mut SparseClipmapRuntime, built: BuiltDisplayMesh) {
    debug_assert!(
        clipmap.release_task.is_none(),
        "显示网格构建/候选状态保证同一时刻最多产生一个待释放CPU结果"
    );
    clipmap.release_task = Some(AsyncComputeTaskPool::get().spawn(async move {
        drop(built); // Vec容量可能接近16MiB，析构和归还分配器均不占用主帧。
    }));
}

/// 缺字段旧图生成只用于运行时的等价临时列表；不会回写场景清单。
fn plan_for(
    spec: &SparseHeightSpec,                        // 当前受检稀疏高度规格。
    explicit_lods: Option<&[SceneTerrainDisplayLod]>, // Some仅来自作者显式字段。
) -> Result<CompiledDisplayLodPlan, String> {
    let legacy_equivalent; // 仅None分支持有临时默认列表。
    let lods = if let Some(explicit) = explicit_lods {
        explicit // 显式作者值不排序、不钳制也不修复。
    } else {
        legacy_equivalent = default_display_lods_for_spec(spec)?; // 精确复现64半格逐级翻倍。
        &legacy_equivalent // 临时借用到本函数结束。
    };
    compile_display_lod_plan(spec, lods) // 量化、补带、Mesh和页预算统一预检。
}

/// 稀疏作者清单必须与已登记的场景总清单描述完全同源。
fn validate_sparse_manifest(
    scene_id: &SceneId,
    map: &SceneManifestAsset,
    sparse: &SparseHeightManifestAsset,
) -> Result<CompiledDisplayLodPlan, String> {
    let spec = &sparse.manifest.spec;
    if sparse.manifest.scene_id != *scene_id
        || map.id != *scene_id
        || map.terrain.kind != SceneTerrainKind::SparseClipmap
        || spec.world_min != map.world_min
        || spec.world_max != map.world_max
        || spec.base_height != map.terrain.base_height
        || spec.height_scale != map.terrain.height_scale
        || spec.cells[0] + 1 != u64::from(map.terrain.resolution[0])
        || spec.cells[1] + 1 != u64::from(map.terrain.resolution[1])
    {
        return Err(format!("场景{scene_id}的稀疏高度规格与场景总清单不一致"));
    }
    sparse.manifest.validate()?; // AssetLoader 已验证；热更采用前仍显式检查格式契约。
    plan_for(spec, map.terrain.display_lods.as_deref())
}

/// 首载、切换场景及清单热更驱动一个有界网格任务；完成后才生成正式可见源。
#[allow(clippy::too_many_arguments)]
pub(super) fn drive_sparse_clipmap(
    frame: feathrise_scene_core::residency::InstanceFrame,
    views: ClipmapViews,
    heights: SparseDisplayHeights,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    active_scene: InstanceRes<ActiveDynamicScene>,
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,
    scenes: Res<Assets<SceneManifestAsset>>,
    sparse_assets: Res<Assets<SparseHeightManifestAsset>>,
    mut asset_events: ParamSet<(
        feathrise_scene_core::residency::ResidentAssetEventReader<SceneManifestAsset>,
        feathrise_scene_core::residency::ResidentAssetEventReader<SparseHeightManifestAsset>,
    )>, // 两类热更游标合并为一个系统参数，仍各自独立消费。
    mut gpu: SparseDisplayUploadAssets,
    runtime: InstanceRes<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
    mut display_status: InstanceResMut<SceneTerrainDisplayLodStatus>,
    mut clipmap: InstanceResMut<SparseClipmapRuntime>,
) {
    poll_display_mesh_release(&mut clipmap); // 无论清单是否就绪，都持续归还有界后台释放槽位。
    let (Some(scene_id), Some(root), Some(path)) = (
        active_scene.scene_id.as_ref(),
        runtime.root,
        runtime.sparse_height_path.as_deref(),
    ) else {
        return; // 旧 GLB 场景或总清单仍在等待，稀疏系统没有写权限。
    };
    if clipmap.scene_id.as_ref() != Some(scene_id)
        || clipmap.root != Some(root)
        || clipmap.path.as_deref() != Some(path)
    {
        if let Some(prepared) = clipmap.prepared.take() {
            gpu.meshes.remove(prepared.mesh.id()); // 切换场景立即回收尚未提交的候选Mesh，正式旧图由场景根生命周期处理。
        }
        clipmap.scene_id = Some(scene_id.clone());
        clipmap.root = Some(root);
        clipmap.path = Some(path.to_owned());
        clipmap.manifest = Some(asset_server.load(path.to_owned()));
        clipmap.desired = None; // 在途旧任务继续占用唯一槽位，完成后因身份不匹配被丢弃。
        if let Some(pending) = clipmap.pending_cpu.take() {
            release_display_mesh_in_background(&mut clipmap, pending); // 未登记CPU候选不得跨场景复用，也不在切换场景帧析构。
        }
        clipmap.applied = None; // 新根没有任何正式显示身份。
        clipmap.failed = None; // 新场景从独立事务重新开始。
        clipmap.bounds = None;
        clipmap.display = None; // 旧场景根由父系统撤销。
        clipmap.spec = None; // 新根重新验收显示规格。
        clipmap.authored_plan = None;
        clipmap.view_lod = None;
        clipmap.requested_focus = None;
        clipmap.manifest_dirty = true;
        *display_status = SceneTerrainDisplayLodStatus {
            scene_id: Some(scene_id.clone()),                   // 新场景先隔离旧诊断。
            phase: SceneTerrainDisplayLodPhase::Queued,       // 等待两份清单完成受检。
            detail: "等待显示LOD清单与稀疏高度规格".into(), // 不把等待误报为失败。
            ..default()
        };
    }
    let Some(handle) = clipmap.manifest.clone() else {
        return; // 初始化分支已经创建强句柄，此处仅防御异常资源状态。
    };
    let sparse_changed = asset_events.p1().read().any(|event| match event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id } => *id == handle.id(),
        AssetEvent::Unused { .. } => false,
    });
    let scene_manifest_id = registry
        .as_deref()
        .and_then(|catalog| catalog.handles(scene_id))
        .map(|handles| handles.manifest.id()); // 场景资料保存只修改总清单，不会触发高度清单事件。
    let scene_changed = asset_events.p0().read().any(|event| match event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id } => Some(*id) == scene_manifest_id,
        AssetEvent::Unused { .. } => false,
    }); // 显式显示LOD保存必须唤醒同一单飞构建链。
    clipmap.manifest_dirty |= sparse_changed || scene_changed;
    if matches!(asset_server.load_state(handle.id()), LoadState::Failed(_)) {
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = format!("场景{scene_id}稀疏高度清单加载失败：{path}");
        return; // 失败不发布空白伪地形，也不撤销旧完整版本。
    }
    let mut authoring_changed = false;
    if clipmap.manifest_dirty {
        let Some(sparse) = sparse_assets.get(&handle) else {
            return; // AssetServer 后台读取未完成。
        };
        let Some(map) = registry
            .as_deref()
            .and_then(|catalog| catalog.handles(scene_id))
            .and_then(|handles| scenes.get(&handles.manifest))
        else {
            return; // 场景总清单仍在异步加载。
        };
        display_status.scene_id = Some(scene_id.clone()); // 当前清单诊断只属于当前场景。
        display_status.saved_lods = map.terrain.display_lods.clone(); // None继续明确表示兼容自动模式。
        let plan = match validate_sparse_manifest(scene_id, map, sparse) {
            Ok(plan) => plan,
            Err(error) => {
                status.phase = DynamicSceneRuntimePhase::Failed;
                status.detail.clone_from(&error);
                display_status.phase = SceneTerrainDisplayLodPhase::Failed;
                display_status.detail = error;
                clipmap.manifest_dirty = false; // 等待下一次文件修订再试，稳定帧不重复报错。
                return;
            }
        };
        if clipmap
            .spec
            .as_ref()
            .is_some_and(|spec| spec != &sparse.manifest.spec)
        {
            // 显示自己约束热更规格，不借旧碰撞运行时判断。
            status.detail =
                format!("场景{scene_id}的Clipmap格距或边界热更需要重新载入场景，保留上一完整地形");
            display_status.phase = SceneTerrainDisplayLodPhase::Failed;
            display_status.detail.clone_from(&status.detail); // 显示域单独回显失败且保留旧版。
            clipmap.manifest_dirty = false; // 同图规格不能让新材质与旧显示/物理拆版。
            return;
        }
        clipmap.spec = Some(sparse.manifest.spec.clone()); // 固定大小元数据快照。
        let bounds = ClipmapBounds::new(
            Vec2::from_array(sparse.manifest.spec.world_min),
            Vec2::from_array(sparse.manifest.spec.world_max),
        );
        clipmap.bounds = bounds.ok(); // 前置规格验证已确保场景矩形有效。
        clipmap.manifest_dirty = false;
        clipmap.authored_plan = Some(plan);
        authoring_changed = true;
    }
    let view = views.resolve(runtime.for_instance(None).and_then(|state| state.camera));
    let choice = view.and_then(|view| view.choice(clipmap.spec.as_ref()?, frame.frame(),
        heights.bounds(scene_id, sparse_assets.get(&handle)?.manifest.height_bounds()),
        clipmap.view_lod, clipmap.authored_plan.as_ref()?));
    let next_plan = if let Some((focus, key)) = choice {
        if authoring_changed || clipmap.view_lod != Some(key) {
            match compile_view_plan(clipmap.spec.as_ref().unwrap(), clipmap.authored_plan.as_ref().unwrap(), key) {
                Ok(plan) => {
                    clipmap.view_lod = Some(key);
                    clipmap.requested_focus = Some(focus);
                    Some(plan)
                }
                Err(error) => {
                    display_status.phase = SceneTerrainDisplayLodPhase::Failed;
                    display_status.detail = format!("相机LOD计划无效：{error}");
                    if clipmap.applied.is_none() {
                        // 首载没有可保留的画面，必须把失败交给加载门，不能永久等待地表和首屏。
                        status.phase = DynamicSceneRuntimePhase::Failed;
                        status.detail.clone_from(&display_status.detail);
                    }
                    None
                }
            }
        } else { clipmap.requested_focus = Some(focus); None }
    } else if authoring_changed || clipmap.desired.is_none() {
        clipmap.authored_plan.clone() // 相机尚未取得视口尺寸时，保留合法作者显示作为启动托底。
    } else { None };
    if let Some(plan) = next_plan {
        let desired = DisplayBuildIdentity {
            scene_id: scene_id.clone(), // 与总清单稳定ID绑定。
            root,                   // 与当前场景根绑定。
            plan,                   // 完整作者、量化环带和预算身份。
        };
        if clipmap.desired.as_ref() != Some(&desired) {
            if clipmap
                .prepared
                .as_ref()
                .is_some_and(|prepared| prepared.identity != desired)
                && let Some(prepared) = clipmap.prepared.take()
            {
                gpu.meshes.remove(prepared.mesh.id()); // 再次保存淘汰旧候选，不触及当前正式Mesh。
            }
            if clipmap
                .pending_cpu
                .as_ref()
                .is_some_and(|pending| pending.identity != desired)
            {
                let pending = clipmap.pending_cpu.take().expect("上方已确认旧CPU候选存在");
                release_display_mesh_in_background(&mut clipmap, pending); // 再次保存不让大型旧结果在主线程析构。
            }
            clipmap.desired = Some(desired); // 连续保存只覆盖一个最新期望。
            clipmap.failed = None; // 新配置修订允许从失败旧候选恢复。
            display_status.phase = SceneTerrainDisplayLodPhase::Queued;
            display_status.detail = if display_status.saved_lods.is_some() {
                "显示LOD正在按配置档位与当前相机距离准备，保留上一完整显示".into()
            } else {
                "兼容自动显示LOD等待后台构建".into()
            }; // 不在草稿阶段触发运行时工作。
        }
    }
    if let Some(task) = clipmap.task.as_mut() {
        if let Some(result) = check_ready(task) {
            clipmap.task = None;
            let building = clipmap.building.take(); // 在途槽位与结果同帧归还。
            let built = match result {
                Ok(built) => built, // 身份仍须与最新期望逐值比较。
                Err(error) => {
                    if building.as_ref() == clipmap.desired.as_ref() {
                        status.phase = DynamicSceneRuntimePhase::Failed;
                        status.detail.clone_from(&error);
                        display_status.phase = SceneTerrainDisplayLodPhase::Failed;
                        display_status.detail = format!("{error}；保留上一完整显示");
                        clipmap.failed = building; // 不在下一稳定帧自动重复同一失败构建。
                    } // 迟到旧任务失败不污染新目标诊断。
                    return; // 下一帧再为最新期望启动唯一新任务。
                }
            };
            if clipmap.desired.as_ref() != Some(&built.identity) {
                display_status.work.mesh_discarded = display_status.work.mesh_discarded.saturating_add(1);
                release_display_mesh_in_background(&mut clipmap, built); // 连续保存或切换场景后的迟到Mesh不登记到Assets。
                return;
            }
            clipmap.pending_cpu = Some(built); // 等共享大资源额度，不提前换正式实体。
            display_status.phase = SceneTerrainDisplayLodPhase::Uploading;
            display_status.detail = "显示LOD组合网格已构建，等待共享GPU登记额度".into();
        }
    }
    if clipmap.prepared.is_none()
        && let Some(pending) = clipmap.pending_cpu.as_ref()
        && clipmap.desired.as_ref() == Some(&pending.identity)
    {
        let estimated = usize::try_from(pending.identity.plan.mesh_bytes)
            .map_err(|_| "显示LOD Mesh字节数无法由当前平台表达".to_owned())
            .and_then(|bytes| {
                gpu.uploads.as_mut().map_or(Ok(true), |uploads| {
                    uploads.try_reserve_terrain_registration(bytes)
                })
            });
        match estimated {
            Ok(true) => {
                let pending = clipmap
                    .pending_cpu
                    .take()
                    .expect("上方已确认CPU网格候选存在");
                let mesh = gpu.meshes.add(pending.mesh); // 只登记候选，不替换当前正式网格。
                display_status.work.mesh_uploads = display_status.work.mesh_uploads.saturating_add(1);
                display_status.work.mesh_upload_bytes = display_status.work.mesh_upload_bytes.saturating_add(pending.identity.plan.mesh_bytes);
                if clipmap.display.is_none() {
                    let material = gpu.white_materials.add(StandardMaterial {
                        base_color: Color::srgb(0.72, 0.72, 0.68),
                        perceptual_roughness: 1.0,
                        ..default()
                    });
                    let display = commands
                        .spawn((
                            Name::new(format!("DynamicSceneSparseClipmapDisplay_{scene_id}")),
                            SparseClipmapDisplay,
                            DynamicSceneTerrainPrimitive,
                            DynamicSparseClipmapPrimitive,
                            Mesh3d(mesh.clone()), // 首载隐藏源供地表系统解析；正式可见性仍等GPU事务采用。
                            MeshMaterial3d(material),
                            GltfMeshName(super::DYNAMIC_TERRAIN_MESH_NAME.to_owned()),
                            GltfMaterialName(super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.to_owned()),
                            Transform::IDENTITY,
                            Visibility::Hidden,
                            NoFrustumCulling,
                        ))
                        .id();
                    commands.entity(pending.identity.root).add_child(display);
                    clipmap.display = Some(display);
                } else if clipmap.applied.is_none()
                    && let Some(display) = clipmap.display
                {
                    commands.entity(display).insert(Mesh3d(mesh.clone())); // 首载失败后新修订复用同一隐藏源实体。
                }
                clipmap.prepared = Some(RegisteredDisplayMesh {
                    identity: pending.identity,
                    mesh,
                    build_milliseconds: pending.build_milliseconds,
                    registered_at: Instant::now(),
                });
                display_status.detail = "显示LOD Mesh已登记，等待匹配高度页候选".into();
            }
            Ok(false) => {} // 水面或高度页已经占用本帧大候选额度，下一帧重试。
            Err(error) => {
                display_status.phase = SceneTerrainDisplayLodPhase::Failed;
                display_status.detail = format!("{error}；保留上一完整显示");
                let pending = clipmap
                    .pending_cpu
                    .take()
                    .expect("额度错误分支仍持有CPU网格候选");
                clipmap.failed = Some(pending.identity.clone()); // 同一坏配置等待新保存，不在稳定帧重建。
                release_display_mesh_in_background(&mut clipmap, pending); // 失败候选也走有界后台析构。
            }
        }
    }
    if clipmap.task.is_none()
        && clipmap.release_task.is_none()
        && clipmap.pending_cpu.is_none()
        && clipmap.prepared.is_none()
        && clipmap.desired != clipmap.applied
        && clipmap.desired != clipmap.failed
        && let (Some(identity), Some(spec)) = (clipmap.desired.clone(), clipmap.spec.as_ref())
    {
        let finest_cell_size = Vec2::new(
            spec.actual_spacing_meters[0] as f32, // X轴真实作者格距。
            spec.actual_spacing_meters[1] as f32, // Z轴真实作者格距。
        );
        clipmap.building = Some(identity.clone()); // 先占唯一在途槽位，再派发任务。
        display_status.work.mesh_builds = display_status.work.mesh_builds.saturating_add(1);
        clipmap.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            let started = Instant::now(); // 纯后台墙钟不读取主世界时间资源。
            let mesh = build_display_lod_mesh(&identity.plan, finest_cell_size)
                .map_err(|error| format!("构建Clipmap显示LOD网格失败：{error:?}"))?;
            Ok(BuiltDisplayMesh {
                identity,                                                     // 完整任务身份随结果返回。
                mesh, // 尚未登记Assets的CPU组合网格。
                build_milliseconds: started.elapsed().as_secs_f32() * 1000.0, // 诊断毫秒。
            })
        })); // 仅内容或量化视图计划变化才重建；同档平移复用拓扑。
        display_status.phase = SceneTerrainDisplayLodPhase::BuildingMesh;
        display_status.detail = "显示LOD正在后台构建组合网格".into();
    }
}

/// 完整候选采用后才移动环带；独立网格共享父世界相机，并在自身源坐标中吸附。
pub(super) fn follow_sparse_clipmap(
    frame: feathrise_scene_core::residency::InstanceFrame,
    mut commands: Commands,
    stamps: Query<&super::clipmap::stitches::StitchLodStamp>,
    runtime: InstanceRes<DynamicSceneRuntimeState>,
    clipmap: InstanceRes<SparseClipmapRuntime>,
    pages: InstanceRes<super::clipmap::SparseHeightPageRuntime>,
    camera_transforms: Query<&GlobalTransform, With<Camera3d>>,
    authoring_views: Query<(&Camera, &DynamicClipmapAuthoringView)>,
    mut display_transforms: InstanceQuery<&mut Transform, With<SparseClipmapDisplay>>,
) {
    if let Some(instance)=frame.instance() {
        let applied_focus=clipmap.display.and_then(|display|display_transforms.get(display).ok()).map_or(Vec2::ZERO,|t|t.translation.xz());
        let stamp=super::clipmap::stitches::StitchLodStamp(clipmap.stitch_step(applied_focus));
        if stamps.get(instance).map_or(true,|old|old!=&stamp) {commands.entity(instance).insert(stamp);}
    }
    if let (Some(bounds), Some(display), Some(spec)) = (
        clipmap.bounds,
        clipmap.display,
        clipmap.spec.as_ref(),
    )
    {
        // 子场景不创建相机，其局部runtime.camera始终为空；必须显式读取父世界唯一玩法相机。
        // 编辑焦点优先且不依赖玩法相机存在，保留完整Y后再逆变换，支持旋转/缩放的独立网格。
        let world_focus = authoring_views
            .iter()
            .find_map(|(camera, view)| camera.is_active.then_some(
                Vec3::new(view.focus_xz.x, view.focus_y, view.focus_xz.y)))
            .or_else(|| {
                let camera = runtime.for_instance(None)?.camera?;
                camera_transforms.get(camera).ok().map(GlobalTransform::translation)
            });
        let focus = if let Some(focus) = clipmap.requested_focus {
            // 新精度/新页窗口尚未完整采用时冻结旧中心；不能让旧Mesh先滑到缺页区。
            if clipmap.applied != clipmap.desired || !pages.resident_window_is_current() { return; }
            focus
        } else {
            let Some(world_focus) = world_focus else { return; };
            frame.to_local(world_focus).xz() // 首帧相机尚未发布投影尺寸时沿用已有显示。
        };
        let finest_cell_size = Vec2::new(
            spec.actual_spacing_meters[0] as f32, // 与网格构建和Shader高度参数同源。
            spec.actual_spacing_meters[1] as f32, // 矩形场景保留两轴独立实际格距。
        );
        if let Ok(center) = snapped_display_focus(focus, bounds, finest_cell_size) {
            if let Ok(mut transform) = display_transforms.get_mut(display) {
                // 相机和F12继续独立控制显示环带。
                let target = Transform::from_xyz(center.x, 0.0, center.y); // 只移动受检最细格焦点，不重建网格。
                if *transform != target {
                    *transform = target; // 分块Collider按世界固定坐标独立流送，显示可逐细格跟随相机。
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "sparse_view_tests.rs"]
mod view_tests;

/// 离开动态场景时释放句柄并把仍可能产出大型CPU结果的任务交给后台完成。
pub(super) fn clear_sparse_clipmap(
    mut clipmap: InstanceResMut<SparseClipmapRuntime>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if let Some(prepared) = clipmap.prepared.take() {
        meshes.remove(prepared.mesh.id()); // 未采用候选不依赖场景根级联回收Assets条目。
    }
    if let Some(task) = clipmap.task.take() {
        task.detach(); // 结果没有接收者，完成后直接在工作线程释放，不把大Vec带回主世界。
    }
    if let Some(task) = clipmap.release_task.take() {
        task.detach(); // 已开始的析构必须在工作线程自然完成。
    }
    if let Some(pending) = clipmap.pending_cpu.take() {
        AsyncComputeTaskPool::get()
            .spawn(async move { drop(pending) })
            .detach(); // 离图时系统不再轮询，显式分离这一份有界CPU候选。
    }
    *clipmap = SparseClipmapRuntime::default(); // 场景实体统一由 DespawnOnExit 回收。
}

#[cfg(test)]
mod tests {
    use super::*; // 只测场景尺寸到固定几何配置，不启动窗口。

    #[test]
    fn max_scene_config_is_bounded_and_covers_edge_view() {
        let spec =
            SparseHeightSpec::new([0.0, 0.0], [8192.0, 8192.0], 0.125, -1024.0, 2048.0).unwrap(); // 最大允许尺寸和最密作者格距。
        let plan = plan_for(&spec, None).unwrap(); // 缺字段走兼容自动计划但不写回清单。
        assert_eq!(plan.bands.len(), 11); // 半径从8米扩到8192米。
        assert!(plan.mesh_bytes < 16 * 1024 * 1024); // 共享编译预算通过。
        assert_eq!(
            plan.actual_half_extents_meters.last(),
            Some(&[8192.0, 8192.0])
        ); // 最终层从边缘焦点覆盖对角远端。
        let bounds = ClipmapBounds::new(Vec2::ZERO, Vec2::splat(8192.0)).unwrap();
        assert_eq!(
            snapped_display_focus(Vec2::ZERO, bounds, Vec2::splat(0.125)).unwrap(),
            Vec2::ZERO
        ); // 相机在角落时实体焦点保持世界固定基础格。
    }

}
