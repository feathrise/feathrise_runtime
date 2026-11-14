//! 动态场景场景进入、旧白模或新稀疏地形加载和碰撞就绪系统。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use avian3d::prelude::RigidBody; // 动态地形根保持静态刚体，Trimesh由共享派生任务完整构建后安装。
use bevy::asset::{AssetLoadFailedEvent, LoadState}; // 清单与GLB热加载事件。
use bevy::gltf::{GltfMaterialName, GltfMeshName}; // 正式GLB primitive完成实例化后登记稳定地形身份。
use bevy::prelude::*; // ECS、场景根、资源和日志。
use feathrise_atmosphere::{
    AuthoredSkyMode, AuthoredSkyboxRequest, DynamicSceneWeatherClimate, UNIFORM_CLIMATE_PROFILE,
    WeatherClimateProfile, WorldCelestialLight, WorldCelestialLightKind,
}; // 动态玩法相机复用天空状态机，清单气候复用统一天气模拟。
use feathrise_environment::terrain_surface::TerrainSurfaceCarrier; // 离屏预热会复制GLB名称，但绝不属于原始地形输入。
use feathrise_environment::vegetation::{VegetationMeshBatchView, blade::GpuBladeView}; // 同一玩法相机同时驱动GPU叶片窗口和Mesh批次逐实例LOD。
use feathrise_scene_content::{
    ActiveDynamicScene, CelestialLightingProfile, DynamicCelestialLightingPreview,
    SceneContentRegistry, SceneManifestAsset, SceneSkyMode, SceneTerrainKind, SceneWeatherClimate,
    SceneWeatherDescriptor, SceneWeatherRegion, WorldEnvironmentRegistry, resolve,
}; // 动态场景读取编辑器共享的清单和当前SceneId。
use feathrise_portal::PortalDestination; // 跨场景传送可在进入前携带动态SceneId。
use feathrise_scene_core::state::{ScenePhase, LoadingProgress, LoadingStage, SceneFallBoundary}; // 动态场景同时发布启动状态、失败回退进度和地形量程下方虚空边界。

use super::resources::{DynamicSceneRuntimePhase, DynamicSceneRuntimeState, DynamicSceneRuntimeStatus};
use super::terrain_derived::{DynamicTerrainHeightPhase, DynamicTerrainHeightStatus}; // 首载失败桥接统一读取GLB派生阶段，不让输入错误停在SceneInit。
use crate::gpu_grass_content::dynamic_scene_blade_view_lod; // 新场景相机复用HN-01已验收的屏幕LOD启动基线。
use crate::shared::scene_runtime::{
    SceneEnvironment, mark_scene_collision_ready, spawn_scene_common,
};

/// 玩家低于高度场编码下限后仍保留的物理容错距离，避免贴近最低河床时误判死亡。
const DYNAMIC_FALL_DEATH_MARGIN_METERS: f32 = 16.0;

/// 从高度场可表示的最低世界Y派生真正虚空边界，拒绝非有限的损坏清单。
fn dynamic_fall_death_y(base_height: f32) -> Option<f32> {
    let death_y = base_height - DYNAMIC_FALL_DEATH_MARGIN_METERS; // 完整高度量程都属于合法地形空间。
    (base_height.is_finite() && death_y.is_finite()).then_some(death_y) // 非有限边界不得进入玩家死亡系统。
}

/// 判断一个GLB primitive是否为动态场景唯一正式地形源。
fn dynamic_terrain_source_identity(mesh_name: &str, material_name: &str) -> bool {
    mesh_name == super::DYNAMIC_TERRAIN_MESH_NAME // 网格身份必须匹配Rust白模生成器的稳定名称。
        && material_name == super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME // 材质身份排除同网格预热和其他primitive。
}

/// GLB场景子实体出现后只登记一次公共地形身份，供可选F12插件安全接管。
pub(super) fn mark_dynamic_scene_terrain_primitive(
    mut commands: Commands, // 仅给匹配的正式primitive插入零大小标记。
    scope: feathrise_scene_core::residency::EntitySceneScope,
    frame: feathrise_scene_core::residency::InstanceFrame,
    runtime: InstanceRes<DynamicSceneRuntimeState>,
    parents: Query<&ChildOf>,
    candidates: InstanceQuery<
        (Entity, &GltfMeshName, &GltfMaterialName), // 同时核对网格和原始白模材质名称。
        (
            Without<DynamicSceneTerrainSourcePrimitive>, // 已登记隐藏输入源不再进入候选。
            Without<crate::DynamicSceneTerrainPrimitive>, // 正式派生显示会复制GLB名称供地表接管，绝不能被反向认作隐藏输入源。
            Without<TerrainSurfaceCarrier>, // 离屏材质预热副本同样复制双名称，必须保持独立可见性直至GPU就绪。
            Or<(Added<GltfMeshName>, Added<GltfMaterialName>, Added<feathrise_scene_core::residency::SceneInstanceOwner>)>, // GLB后挂到子场景时，归属补齐也触发一次检查。
        ),
    >,
) {
    for (entity, mesh_name, material_name) in &candidates {
        if scope.instance(entity) != frame.instance() { continue; } // 后代归属分帧补齐期间，父图不得先拿走子图的GLB输入。
        // 名称只证明资产类型；缩略图、资产预览也会实例化同名GLB，必须属于本次正式地形根。
        if !runtime.root.is_some_and(|root| {
            let mut cursor = entity;
            for _ in 0..128 {
                let Ok(parent) = parents.get(cursor) else { return false; };
                cursor = parent.parent();
                if cursor == root { return true; }
            }
            false // 异常深层级失败关闭，不无界回溯。
        }) { continue; }
        if !dynamic_terrain_source_identity(&mesh_name.0, &material_name.0) {
            continue; // 水面、预热网格和场景装饰均不得获得地形编辑所有权。
        }
        commands.entity(entity).insert((
            DynamicSceneTerrainSourcePrimitive, // GLB primitive只作为派生输入，不直接承担正式显示或碰撞。
            Visibility::Hidden,               // 首载及热更计算期间禁止半套新网格提前出现在画面。
        )); // 正式显示网格只在完整派生结果采用时由地形域创建或替换。
    }
}

/// 构造新场景进入时使用的中性白模环境。
fn dynamic_scene_environment() -> SceneEnvironment {
    SceneEnvironment {
        clear_color: Color::srgb(0.56, 0.62, 0.68),
        light_illuminance: 9_000.0,
        light_color: Color::srgb(1.0, 0.93, 0.82),
        light_rotation: (-1.35, -0.42),
    }
}

/// 旧地域标识已在读取清单时规范化；运行时只采用中性配置，不再恢复退役场景气候。
fn dynamic_scene_weather_profile(weather: SceneWeatherDescriptor) -> WeatherClimateProfile {
    match (weather.region, weather.climate) {
        (SceneWeatherRegion::Global, SceneWeatherClimate::Uniform) => UNIFORM_CLIMATE_PROFILE,
    }
}

/// 进入统一动态场景时接收传送目标SceneId并建立相机、光照与启动门等待环境。
pub(super) fn begin_dynamic_scene(
    mut commands: Commands,
    mut clear_color: ResMut<ClearColor>,
    view_layers: Res<crate::SceneViewLayers>,
    portal_destination: Res<PortalDestination>,
    mut active_scene: InstanceResMut<ActiveDynamicScene>,
    mut weather_climate: InstanceResMut<DynamicSceneWeatherClimate>, // 新会话先撤销上一张动态场景的局部气候。
    mut fall_boundary: InstanceResMut<SceneFallBoundary>, // 进入新动态场景时先撤销上一张图的高度所有权。
    mut state: InstanceResMut<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
) {
    weather_climate.clear(); // 清单完成验证前自动天气暂不推进，不能沿用上一张图的抽取倍率。
    fall_boundary.clear_for_scene(ScenePhase::Active); // 清单完成校验前不允许沿用旧动态场景边界。
    if let Some(scene_id) = portal_destination.dynamic_scene_id.as_deref() {
        match feathrise_scene_content::SceneId::parse(scene_id) {
            Ok(scene_id) => active_scene.select(scene_id),
            Err(error) => {
                status.phase = DynamicSceneRuntimePhase::Failed;
                status.detail = format!("传送目标动态场景ID无效：{error}");
            }
        }
    }
    let environment = dynamic_scene_environment();
    let (camera, celestial_sun_light, celestial_moon_light) = spawn_scene_common(
        &mut commands,
        &mut clear_color,
        &environment,
        ScenePhase::Active,
        &view_layers,
    );
    commands.entity(camera).insert((
        GpuBladeView::screen_space_lod_with(dynamic_scene_blade_view_lod()),
        VegetationMeshBatchView,
    )); // 所有Blade与Mesh植被后端共享唯一玩法观察位置，小场景不会重复生成LOD实例流。
    state.dirty = true;
    state.root = None;
    state.camera = Some(camera); // 只保存当前场景唯一玩法相机，不扫描世界寻找接管目标。
    state.celestial_sun_light = Some(celestial_sun_light); // 全局与场景覆盖通过明确实体更新真实太阳灯。
    state.celestial_moon_light = celestial_moon_light; // 动态场景公共生成器保证提供独立月亮灯。
    state.active_scene_id = None;
    state.glb_path = None;
    state.sparse_height_path = None;
    state.collision_ready = false;
    state.sparse_clipmap_ready = false; // 新场景必须安装自己的固定显示网格。
    state.sparse_height_pages_ready = false; // 新场景必须采用自己的首组高度页。
    state.surface_required = false;
    state.surface_ready = false;
    state.boundary_required = false; // 清单尚未验证时不能假定存在安全边界。
    state.boundary_ready = false; // 边界系统在同一进入链后独立发布完成证据。
    state.ready_released = false;
    state.environment_revision = state.environment_revision.wrapping_add(1); // 新场景会话使上一相机天空结果过期。
    status.phase = DynamicSceneRuntimePhase::Waiting;
    status.scene_id = active_scene.scene_id.clone();
    status.detail = "正在读取动态场景清单".to_owned();
}

/// 清单、目录或GLB失败事件只更新有限事务状态，稳定帧不扫描世界。
pub(super) fn observe_dynamic_scene_assets(
    active_scene: InstanceRes<ActiveDynamicScene>,
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,
    mut manifest_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneManifestAsset>,
    mut glb_failures: MessageReader<AssetLoadFailedEvent<WorldAsset>>,
    mut manifest_failures: MessageReader<AssetLoadFailedEvent<SceneManifestAsset>>,
    mut state: InstanceResMut<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
) {
    let registry_changed = registry
        .as_deref()
        .is_some_and(|registry| registry.revision != state.catalog_revision);
    let manifest_id = active_scene.scene_id.as_ref()
        .and_then(|scene_id| registry.as_deref()?.handles(scene_id))
        .map(|handles| handles.manifest.id()); // 容器与子实例使用运行域自己的合法投影，不能按同SceneId接受其他清单。
    let mut manifest_changed = false;
    for event in manifest_events.read() {
        let id = match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } | AssetEvent::Removed { id }
            | AssetEvent::Unused { id } | AssetEvent::LoadedWithDependencies { id } => *id,
        };
        manifest_changed |= Some(id) == manifest_id; // 全部消费原通知及驻留回放，匹配后也不能遗留下一帧重复刷新。
    }
    if registry_changed || manifest_changed {
        state.dirty = true;
        state.environment_revision = state.environment_revision.wrapping_add(1); // 清单或目录换代强制天空请求取得新身份。
    }
    for failure in manifest_failures.read() {
        if Some(failure.id) != manifest_id { continue; } // 其他场景或同图私有投影失败不污染当前运行域。
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = format!("动态场景清单加载失败：{} ({})", failure.path, failure.error);
        error!("{}", status.detail);
    }
    for failure in glb_failures.read() {
        if state
            .glb_path
            .as_deref()
            .is_some_and(|path| failure.path.to_string().contains(path))
        {
            status.phase = DynamicSceneRuntimePhase::Failed;
            status.detail = format!(
                "动态场景白模GLB加载失败：{} ({})",
                failure.path, failure.error
            );
            error!("{}", status.detail);
        }
    }
}

#[cfg(test)]
#[path = "systems/manifest_events_tests.rs"]
mod manifest_events_tests;

/// 目录已经完成采用但不再包含当前SceneId时，一次性终止整张动态场景首载并返回主菜单。
pub(super) fn return_to_main_menu_if_dynamic_scene_missing(
    active_scene: InstanceRes<ActiveDynamicScene>, // 读取读档、传送或编辑器切换场景提前冻结的真实SceneId。
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>, // None或零修订都表示目录尚未完成首次异步采用。
    mut state: InstanceResMut<DynamicSceneRuntimeState>, // 关闭本代次地形准备，避免同帧继续生成半套世界。
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>, // F3和日志保留导致回退的精确场景身份。
    mut progress: Option<ResMut<LoadingProgress>>, // 加载遮罩在状态切换前显示一次真实失败原因。
    mut next_scene: ResMut<NextState<ScenePhase>>, // 目录身份确定缺失后直接请求正式主菜单状态。
) {
    let (Some(scene_id), Some(registry)) = (active_scene.scene_id.as_ref(), registry.as_deref()) else {
        return; // 尚未选择场景沿用既有作者错误页；目录未就绪则继续异步等待。
    };
    if registry.revision == 0 || registry.handles(scene_id).is_some() {
        return; // 首次目录未采用或仍持有当前场景时，继续等待或保留可修复的原地失败设计。
    }

    let detail = format!("场景目录不存在{scene_id}"); // 先冻结诊断，后续状态与日志共用同一权威文本。
    state.dirty = false; // 本帧后续准备链由运行条件跳过，下一状态帧再执行完整OnExit清理。
    status.phase = DynamicSceneRuntimePhase::Failed; // 离场前仍明确发布失败，不能冒充正常加载完成。
    status.scene_id = Some(scene_id.clone()); // 保留导致回退的稳定身份，避免只看到共用DynamicSceneRoot。
    status.detail.clone_from(&detail); // F3或其他观察者在OnExit前可读取完整原因。
    if let Some(progress) = progress.as_deref_mut() {
        progress.stage = LoadingStage::Failed; // 加载遮罩不得继续显示永远无法完成的场景初始化。
        progress.completed = 0; // 失败不是部分完成证据。
        progress.total = 1; // 保持进度分母合法，避免UI计算零分母。
        progress.label = format!("{detail}，正在返回主菜单"); // 状态切换前给用户可读反馈。
    }
    error!("{detail}，已请求返回主菜单"); // 同一缺失身份只在离场前记录一次，不让水域等子系统重复报错。
    next_scene.set(ScenePhase::Idle); // 下一次状态过渡进入正式主菜单并触发动态场景OnExit。
}

/// 只有目录未就绪或仍包含当前SceneId时，才允许地形、地表、植被、水域和边界子加载器推进。
pub(super) fn dynamic_scene_target_is_registered(
    active_scene: InstanceRes<ActiveDynamicScene>, // 读取当前动态场景稳定身份。
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>, // 目录未完成首次采用时仍应让各系统发布Loading状态。
) -> bool {
    let Some(scene_id) = active_scene.scene_id.as_ref() else {
        return true; // 无SceneId继续交给既有作者错误页处理，不擅自改变其恢复契约。
    };
    registry
        .as_deref()
        .is_none_or(|registry| registry.revision == 0 || registry.handles(scene_id).is_some()) // 完成目录明确缺失时，整条子加载链同帧关闭。
}

/// 旧 GLB 派生链只服务明确声明白模路径的场景；稀疏格点不能被整图捕获。
pub(super) fn dynamic_scene_uses_glb_terrain(state: InstanceRes<DynamicSceneRuntimeState>) -> bool {
    state.glb_path.is_some() && state.derive_glb_height
}

/// 从场景清单生成唯一白模根；同一路径GLB内容修改交给既有热重建插件。
pub(super) fn prepare_dynamic_scene_terrain(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    active_scene: InstanceRes<ActiveDynamicScene>,
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,
    manifests: Res<Assets<SceneManifestAsset>>,
    mut weather_climate: InstanceResMut<DynamicSceneWeatherClimate>, // 完整清单验证后原子采用区域和气候配置。
    author_objects: Option<InstanceRes<feathrise_scene_content::SceneInstanceAuthorPreview>>,
    mut fall_boundary: InstanceResMut<SceneFallBoundary>, // 只在清单完整有效后发布当前场景的虚空边界。
    mut state: InstanceResMut<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
) {
    if author_objects.as_ref().is_some_and(|preview| preview.is_changed()) { state.dirty = true; }
    if !state.dirty {
        return;
    }
    let Some(scene_id) = active_scene.scene_id.as_ref() else {
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = "尚未选择动态场景".to_owned();
        state.dirty = false;
        return;
    };
    status.scene_id = Some(scene_id.clone());
    let Some(registry) = registry.as_deref() else {
        status.phase = DynamicSceneRuntimePhase::Waiting;
        status.detail = "场景目录仍在加载".to_owned();
        return;
    };
    state.catalog_revision = registry.revision;
    let Some(handles) = registry.handles(scene_id) else {
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = format!("场景目录不存在{scene_id}");
        state.dirty = false;
        return;
    };
    if matches!(
        asset_server.load_state(handles.manifest.id()),
        LoadState::Failed(_)
    ) {
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = format!("场景{scene_id}清单加载失败");
        state.dirty = false;
        return;
    }
    let Some(manifest) = manifests.get(&handles.manifest) else {
        status.phase = DynamicSceneRuntimePhase::Waiting;
        status.detail = format!("场景{scene_id}清单仍在加载");
        return;
    };
    let author_container = author_objects.as_deref().filter(|preview| preview.scene_id.as_ref() == Some(scene_id))
        .and_then(|preview| preview.terrains.as_ref()).map(|terrains| {
            let mut container = manifest.clone();
            container.terrain = feathrise_scene_content::SceneTerrainDescriptor::empty();
            container.terrains = Some(terrains.clone());
            container
        });
    let manifest = author_container.as_ref().unwrap_or(manifest);
    if let Some(camera) = state.camera {
        let sky_mode = match manifest.environment.sky_mode {
            SceneSkyMode::Dynamic => AuthoredSkyMode::Dynamic, // 旧场景默认只加载 IBL 两件套。
            SceneSkyMode::Skybox => AuthoredSkyMode::Skybox,   // 显式模式加载并显示主 Cube 三件套。
        };
        commands
            .entity(camera)
            .insert(AuthoredSkyboxRequest::new_with_mode(
                sky_mode, // 运行时枚举隔离 atmosphere 与场景数据 crate 的依赖方向。
                manifest.environment.skybox_path.clone(), // 清单相对 assets 主路径由统一天空资源运行时按模式验证。
                manifest.environment.skybox_brightness, // 天空盒模式使用该日间基准，动态模式无损保留供切换。
                manifest.environment.environment_diffuse_intensity, // 人物与地形天空补光采用独立冻结基准。
                manifest.environment.environment_specular_intensity, // 水面与光滑材质反射采用独立冻结基准。
                manifest.environment.skybox_yaw_degrees, // 玩法与 F12 的可见 Cube、IBL 方向完全一致。
                state.environment_revision, // 热更新失败或迟到完成只能影响当前清单修订。
            )); // IBL 加载失败独立回退相机原环境贴图，不阻断动态天空、地形、碰撞和场景 Ready 门。
    }
    let sparse_path = manifest.terrain.height_path.as_deref(); // 新场景以独立稀疏清单作为作者源。
    let valid_terrain_source = match manifest.terrain.kind {
        SceneTerrainKind::Empty => manifest.terrains.is_some(), // 空容器必须显式声明对象集合。
        SceneTerrainKind::Voxel => manifest.terrain.glb_path.is_empty(), // 独立体素的作者身份由其对象上下文校验。
        SceneTerrainKind::Gltf => !manifest.terrain.glb_path.trim().is_empty(), // 原始GLB沿用完整Scene0。
        SceneTerrainKind::SparseClipmap => {
            manifest.terrain.glb_path.is_empty()
                && sparse_path.is_some_and(|path| feathrise_scene_content::sparse_height_manifest_terrain_id(scene_id, path).is_ok())
        } // 稀疏路径由 SceneId 推导，拒绝跨场景或非预期文件。
    };
    if manifest.id != *scene_id
        || !manifest.is_data_driven()
        || manifest.world_bounds().is_none()
        || !valid_terrain_source
        || !manifest.terrain.height_scale.is_finite()
        || manifest.terrain.height_scale <= 0.0
        || manifest.environment.validate().is_err()
    {
        status.phase = DynamicSceneRuntimePhase::Failed;
        status.detail = format!("场景{scene_id}不是有效的动态高度场场景清单");
        state.dirty = false;
        return;
    }
    let Some(fall_death_y) = dynamic_fall_death_y(manifest.terrain.base_height) else {
        status.phase = DynamicSceneRuntimePhase::Failed; // 损坏高度量程不能进入死亡规则。
        status.detail = format!("场景{scene_id}高度场最低高度无效"); // 明确指出清单失败字段。
        state.dirty = false; // 等待下一次清单热更新事件再重试。
        return; // 保留旧完整地形和边界，不采用半套新清单。
    };
    if !fall_boundary.set_for_scene(ScenePhase::Active, fall_death_y) {
        status.phase = DynamicSceneRuntimePhase::Failed; // 防御性拒绝状态资源未接受的边界。
        status.detail = format!("场景{scene_id}虚空死亡边界无效"); // F3与F12可直接定位失败项。
        state.dirty = false; // 不在稳定帧重复处理同一损坏输入。
        return; // 当前完整运行时保持不变。
    }
    weather_climate.set(dynamic_scene_weather_profile(manifest.environment.weather)); // 完整环境与安全高度均通过后才替换气候，失败热更保留上一套有效配置。
    if state.active_scene_id.as_ref() == Some(scene_id)
        && ((matches!(manifest.terrain.kind, SceneTerrainKind::Empty | SceneTerrainKind::Voxel) && state.glb_path.is_none() && state.sparse_height_path.is_none())
            || (manifest.terrain.kind == SceneTerrainKind::SparseClipmap
            && state.sparse_height_path.as_deref() == sparse_path)
            || (matches!(manifest.terrain.kind, SceneTerrainKind::Gltf)
                && state.glb_path.as_deref() == Some(manifest.terrain.glb_path.as_str())))
        && state.root.is_some()
    {
        state.dirty = false;
        return;
    }
    if let Some(old_root) = state.root.take() {
        commands.entity(old_root).try_despawn();
    }
    state.derive_glb_height = false; // 高度作者源只由Clipmap分页维护，模型不再派生整图高度。
    state.terrain_display = None;
    if matches!(manifest.terrain.kind, SceneTerrainKind::Empty | SceneTerrainKind::Voxel) {
        let root = commands.spawn((Name::new(format!("DynamicSceneContainer_{scene_id}")),
            DynamicSceneTerrainRoot, Transform::IDENTITY, Visibility::Inherited,
            DespawnOnExit(ScenePhase::Active))).id();
        state.root = Some(root);
        state.active_scene_id = Some(scene_id.clone());
        state.glb_path = None;
        state.sparse_height_path = None;
        state.collision_ready = manifest.terrain.kind == SceneTerrainKind::Empty; // 体素必须等真实三维几何和碰撞完整采用。
        state.sparse_clipmap_ready = false;
        state.sparse_height_pages_ready = false;
        state.surface_required = false;
        state.surface_ready = true;
        state.boundary_required = handles.boundaries.is_some();
        state.boundary_ready = !state.boundary_required;
        state.ready_released = false;
        state.dirty = false;
        status.phase = DynamicSceneRuntimePhase::Ready;
        status.detail = "场景容器已就绪；地表由独立地形对象提供".into();
        return;
    }
    if manifest.terrain.kind == SceneTerrainKind::SparseClipmap {
        let root = commands
            .spawn((
                Name::new(format!("DynamicSceneSparseClipmap_{scene_id}")),
                DynamicSceneTerrainRoot,
                Transform::IDENTITY,
                Visibility::Inherited, // 可见环带与隐藏光追源共用完整父级可见性链，避免 B0004。
                RigidBody::Static,
                DespawnOnExit(ScenePhase::Active),
            ))
            .id(); // 地形、局部碰撞与三维洞穴可分别挂在同一个场景根下。
        state.root = Some(root);
        state.active_scene_id = Some(scene_id.clone());
        state.glb_path = None;
        state.sparse_height_path = sparse_path.map(str::to_owned);
        state.collision_ready = false; // 稀疏碰撞系统采用当前场景块后单独放行。
        state.sparse_clipmap_ready = false; // 固定网格后台完成前不能结束首次加载。
        state.sparse_height_pages_ready = false; // 空图平页或非空正式页采用后再放行。
        state.surface_required = handles.surface.is_some();
        state.surface_ready = !state.surface_required;
        state.boundary_required = handles.boundaries.is_some();
        state.boundary_ready = !state.boundary_required;
        state.ready_released = false;
        state.dirty = false;
        status.phase = DynamicSceneRuntimePhase::BuildingCollision;
        status.detail = format!("场景{scene_id}稀疏高度根已建立，正在加载 Clipmap 与局部碰撞");
        return;
    }
    let scene_handle =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset(manifest.terrain.glb_path.clone()));
    let root = commands
        .spawn((
            Name::new(format!("DynamicSceneTerrain_{scene_id}")),
            DynamicSceneTerrainRoot,
            WorldAssetRoot(scene_handle.clone()),
            Transform::IDENTITY,
            RigidBody::Static,
            DespawnOnExit(ScenePhase::Active),
        ))
        .id();
    state.root = Some(root);
    state.active_scene_id = Some(scene_id.clone());
    state.glb_path = Some(manifest.terrain.glb_path.clone());
    state.sparse_height_path = None;
    state.collision_ready = false;
    state.sparse_clipmap_ready = false; // 旧 GLB 路径不消费该证据。
    state.sparse_height_pages_ready = false; // 旧 GLB 路径不消费该证据。
    state.surface_required = handles.surface.is_some();
    state.surface_ready = !state.surface_required;
    state.boundary_required = handles.boundaries.is_some(); // 新场景声明边界文件后加入首次启动门。
    state.boundary_ready = !state.boundary_required; // 旧场景未声明时保持向后兼容。
    state.ready_released = false;
    state.dirty = false;
    status.phase = DynamicSceneRuntimePhase::BuildingCollision;
    status.detail = format!("场景{scene_id}白模已生成，正在构建碰撞");
}

/// 把全局配置、正式场景覆盖或 F12 未保存预览解析到当前动态场景太阳灯与月亮灯。
pub(super) fn sync_dynamic_celestial_lighting(
    active_scene: InstanceRes<ActiveDynamicScene>, // 当前动态场景稳定身份决定可读取哪一份清单与预览。
    content: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>, // 目录和清单异步加载期间允许只使用全局安全默认值。
    manifests: Res<Assets<SceneManifestAsset>>, // 正式场景覆盖始终来自已完整加载资产。
    global: Option<Res<WorldEnvironmentRegistry>>, // 缺失全局插件时仍使用编译期配置默认值。
    preview: Option<Res<DynamicCelestialLightingPreview>>, // F12 只发布已经解析且匹配场景的完整草稿快照。
    state: InstanceRes<DynamicSceneRuntimeState>, // 保存进入场景时创建的明确太阳和月亮方向光实体。
    mut lights: Query<&mut WorldCelestialLight>, // 只改作者基准，最终时间与天气仍由氛围系统统一写渲染组件。
) {
    let (Some(scene_id), Some(sun_entity), Some(moon_entity)) = (
        active_scene.scene_id.as_ref(), // 当前场景身份决定正式清单与 F12 预览匹配。
        state.celestial_sun_light,  // 太阳实体只消费日间照度、日光色和太阳轨迹。
        state.celestial_moon_light, // 月亮实体只消费夜间峰值、月光色和月亮轨迹。
    ) else {
        return; // 进入场景前或离场后没有合法接管目标。
    };
    let global_profile = global
        .as_deref()
        .map(|registry| registry.celestial_lighting.clone())
        .unwrap_or_default(); // 全局文件缺失、无效或插件未安装都保持当前默认画面。
    let formal_profile = content
        .as_deref()
        .and_then(|registry| registry.handles(scene_id))
        .and_then(|handles| manifests.get(&handles.manifest))
        .map(|manifest| resolve(&global_profile, &manifest.environment.celestial_overrides))
        .filter(|profile| profile.validate().is_ok())
        .unwrap_or_else(|| global_profile.clone()); // 清单未就绪或覆盖损坏时不让方向光变黑。
    let resolved: &CelestialLightingProfile = preview
        .as_deref()
        .and_then(|preview| preview.profile_for(scene_id))
        .unwrap_or(&formal_profile); // 匹配场景的未保存草稿在关闭 F12 后仍优先；保存或撤销回到干净状态才归还正式值。
    let Ok(current_sun) = lights.get_mut(sun_entity) else {
        return; // 同帧离场或实体重建由下一次明确进入重新登记。
    };
    let day_clear_color = current_sun.day_clear_color; // 两盏灯共享场景天空底色，只从稳定太阳实体读取一次。
    drop(current_sun); // 结束首次可变借用后再逐项更新太阳与月亮实体。
    for (entity, kind) in [
        (sun_entity, WorldCelestialLightKind::Sun), // 第一项构造独立太阳灯配置。
        (moon_entity, WorldCelestialLightKind::Moon), // 第二项构造独立月亮灯配置。
    ] {
        let desired = WorldCelestialLight::new_authored(
            kind,                                         // 稳定身份决定最终系统消费日间还是夜间字段。
            resolved.day_base_illuminance_lux,            // 日间基础照度只进入太阳高度与季节曲线。
            resolved.day_color(),                         // 高角度太阳使用最终 sRGB 作者色。
            resolved.night_peak_illuminance_lux, // 夜间作者值恢复为月亮午夜峰值，旧 JSON 键名无需迁移。
            resolved.night_color(),              // 月亮灯使用独立夜间颜色。
            resolved.azimuth_offset_degrees.to_radians(), // 水平偏移同时作用于太阳和月亮真实轨迹。
            resolved.light_source_angular_diameter_degrees.to_radians(), // 作者角直径只写太阳，月亮使用固定零点五一八度。
            day_clear_color, // 场景日间基色供动态大气回退底色与天气云层共同使用。
        );
        let Ok(mut current) = lights.get_mut(entity) else {
            continue; // 同帧单个实体回收时保留另一盏灯，下一场景进入会重新登记完整双灯。
        };
        if *current != desired {
            *current = desired; // 只在完整解析结果改变时写组件，稳定帧保持变更检测安静。
        }
    }
}

/// 动态场景白模根标记；派生完成值只安装到当前活动根，并防止其他碰撞事件误放行启动门。
#[derive(Component)]
pub(crate) struct DynamicSceneTerrainRoot;

/// GLB场景内的隐藏输入primitive；正式画面、碰撞和F12编辑均不直接持有它。
#[derive(Component)]
pub(super) struct DynamicSceneTerrainSourcePrimitive;

/// 把动态场景首次构建失败发布到跨场景加载遮罩；已进入玩法后的GLB热更失败继续保留旧完整版本。
pub(super) fn publish_dynamic_scene_initial_failure(
    state: InstanceRes<DynamicSceneRuntimeState>, // ready_released区分首次加载和已有完整版本的热更。
    terrain_status: InstanceRes<DynamicTerrainHeightStatus>, // 捕获期输入错误也必须提升为场景级失败。
    display_status: InstanceRes<feathrise_scene_content::SceneTerrainDisplayLodStatus>,
    mut runtime_status: InstanceResMut<DynamicSceneRuntimeStatus>, // 统一形成F3、F12和加载页可读取的失败原因。
    mut progress: Option<ResMut<LoadingProgress>>,       // 只有真实加载事务存在时才更新跨场景遮罩。
) {
    if state.ready_released {
        return; // 热更失败不得重新盖住已经进入的玩法画面，也不得撤销上一完整地形。
    }
    if state.sparse_height_path.is_some()
        && (!state.sparse_clipmap_ready || !state.sparse_height_pages_ready)
        && display_status.phase == feathrise_scene_content::SceneTerrainDisplayLodPhase::Failed
    {
        runtime_status.phase = DynamicSceneRuntimePhase::Failed;
        runtime_status.scene_id.clone_from(&display_status.scene_id);
        runtime_status.detail.clone_from(&display_status.detail); // GPU失败/超时同样结束首载等待，不能只停留在F3诊断。
    }
    if runtime_status.phase != DynamicSceneRuntimePhase::Failed {
        if state.sparse_height_path.is_some()
            || terrain_status.phase != DynamicTerrainHeightPhase::Failed
        {
            return; // 当前仍在合法异步等待或构建阶段，继续保留SceneInit进度。
        }
        runtime_status.phase = DynamicSceneRuntimePhase::Failed; // 顶点/索引捕获期失败同样属于首载失败。
        runtime_status.scene_id.clone_from(&terrain_status.scene_id); // 失败身份与地形派生目标保持一致。
        runtime_status.detail = if let Some(scene_id) = terrain_status.scene_id.as_ref() {
            format!(
                "场景{scene_id}从GLB生成内存高度失败：{}",
                terrain_status.detail
            ) // 场景级状态补齐具体GLB派生原因。
        } else {
            format!("动态场景从GLB生成内存高度失败：{}", terrain_status.detail) // 缺少SceneId时仍保留可操作的底层错误。
        };
    }
    let Some(progress) = progress.as_deref_mut() else {
        return; // F12直接预览或独立测试没有加载遮罩时只保留结构化状态。
    };
    if progress.stage == LoadingStage::Failed {
        return; // 目录缺失等更早失败路径已经写入专用恢复提示，不被通用桥接覆盖。
    }
    let detail = if runtime_status.detail.trim().is_empty() {
        "动态场景初始化失败，请查看日志".to_owned() // 防御性保证失败页永远不是空文本。
    } else {
        runtime_status.detail.clone() // UI显示与运行时诊断完全相同的具体原因。
    };
    progress.stage = LoadingStage::Failed; // 结束永远停在70%的SceneInit视觉状态。
    progress.completed = 0; // 失败不冒充任何完成进度。
    progress.total = 1; // 维持加载UI合法分母。
    progress.label.clone_from(&detail); // 标题切为“加载失败”，正文显示可修复的资产错误。
    error!("动态场景首次加载失败：{detail}"); // 每个加载事务只在首次切入Failed时记录一次明确错误。
}

/// 碰撞与可选地表都完整就绪后才放行玩法；热更失败不重复撤销已生成角色。
pub(super) fn dynamic_scene_startup_inputs_ready(state: &DynamicSceneRuntimeState) -> bool {
    let sparse_visual_ready = state.sparse_height_path.is_none()
        || state.sparse_clipmap_ready && state.sparse_height_pages_ready; // 只有稀疏新图增加两项首屏证据。
    state.collision_ready && state.surface_ready && state.boundary_ready && sparse_visual_ready // 一个纯判定供启动系统和无窗口回归共享。
}

/// 碰撞、可选内容与稀疏首屏都完整就绪后才放行玩法；热更失败不重复撤销已生成角色。
pub(super) fn finalize_dynamic_scene_ready(
    mut commands: Commands,
    mut state: InstanceResMut<DynamicSceneRuntimeState>,
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>,
    subscenes: Option<Res<super::SubsceneRuntimeStatus>>,
    volumes: Option<Res<super::TerrainVolumeStreamStatus>>,
    registry: Option<Res<SceneContentRegistry>>,
    placements: Option<Res<crate::ScenePlacementRuntimeStatus>>,
) {
    if subscenes.as_deref().is_some_and(|subscenes| !subscenes.ready) { return; }
    if !dynamic_scene_startup_inputs_ready(&state) {
        return;
    }
    if state.sparse_height_path.is_some()
        && volumes
            .as_deref()
            .is_some_and(|v| v.scene_id == state.active_scene_id && !v.ready)
    {
        status.detail = volumes.as_ref().unwrap().detail.clone();
        return;
    }
    if !state.ready_released {
        // 空地形场景的可走地面可以全部来自 GLB 摆放，须先采用同一场景的网格与碰撞。
        if let Some(scene_id) = state.active_scene_id.as_ref()
            && registry.as_deref().and_then(|registry| registry.handles(scene_id))
                .and_then(|handles| handles.placements.path())
                .is_some_and(|path| path.path().extension().is_some_and(|ext| ext == "glb"))
        {
            use crate::ScenePlacementRuntimePhase;
            let Some(placement) = placements.as_deref()
                .filter(|placement| placement.scene_id.as_ref() == Some(scene_id))
            else { return; };
            if placement.phase == ScenePlacementRuntimePhase::Failed {
                status.phase = DynamicSceneRuntimePhase::Failed;
                status.detail.clone_from(&placement.detail); // 沿用动态场景首载失败页，保留实际资产诊断。
                return;
            }
            if placement.phase != ScenePlacementRuntimePhase::Ready {
                if status.detail != placement.detail {
                    status.detail.clone_from(&placement.detail);
                }
                return;
            }
        }
        mark_scene_collision_ready(&mut commands, ScenePhase::Active);
        state.ready_released = true;
        status.revision = status.revision.wrapping_add(1);
    }
    if status.phase != DynamicSceneRuntimePhase::Ready {
        status.phase = DynamicSceneRuntimePhase::Ready;
        let terrain = if state.sparse_height_path.is_some() {
            "稀疏Clipmap地形" // 新图没有GLB白模，诊断必须反映真实运行时。
        } else {
            "白模地形" // 已有动态场景继续沿用单GLB派生路径。
        };
        status.detail = if state.surface_required && state.boundary_required {
            format!("动态场景{terrain}、RGBA地表与区域边界均已就绪")
        } else if state.surface_required {
            format!("动态场景{terrain}碰撞与RGBA地表均已就绪")
        } else if state.boundary_required {
            format!("动态场景{terrain}碰撞与区域边界均已就绪")
        } else {
            format!("动态场景{terrain}与碰撞已就绪")
        };
    }
}

/// 离开动态场景时只清理轻量事务；实体由DespawnOnExit统一回收。
pub(super) fn clear_dynamic_scene(
    mut state: InstanceResMut<DynamicSceneRuntimeState>, // 清理动态场景轻量加载事务。
    mut status: InstanceResMut<DynamicSceneRuntimeStatus>, // 发布离图后的等待诊断。
    mut weather_climate: InstanceResMut<DynamicSceneWeatherClimate>, // 离图释放动态清单天气所有权。
    mut fall_boundary: InstanceResMut<SceneFallBoundary>, // 离图必须释放本场景死亡边界所有权。
) {
    weather_climate.clear(); // 固定场景随后由自己的显式静态气候接管，不继承动态局部倍率。
    fall_boundary.clear_for_scene(ScenePhase::Active); // 静态场景不会继承动态场景深度量程。
    state.dirty = true;
    state.active_scene_id = None;
    state.glb_path = None;
    state.sparse_height_path = None;
    state.root = None;
    state.camera = None; // 实体由DespawnOnExit回收，轻量所有权不得泄漏到下一场景。
    state.celestial_sun_light = None; // 离场后不允许全局热更写入已经回收的太阳实体。
    state.celestial_moon_light = None; // 离场后同样释放月亮灯轻量所有权。
    state.collision_ready = false;
    state.sparse_clipmap_ready = false; // 离图清除旧固定网格完成信用。
    state.sparse_height_pages_ready = false; // 离图清除旧高度页完成信用。
    state.surface_required = false;
    state.surface_ready = false;
    state.boundary_required = false; // 离图清除旧目录要求。
    state.boundary_ready = false; // 下一张图必须重新发布边界完成证据。
    state.ready_released = false;
    state.environment_revision = state.environment_revision.wrapping_add(1); // 离图使所有迟到天空结果过期。
    status.phase = DynamicSceneRuntimePhase::Waiting;
    status.detail = "已离开动态场景".to_owned();
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap; // 缺失场景回归构造一份已经完成采用但内容为空的目录。

    use bevy::state::app::StatesPlugin; // 最小测试应用显式安装正式状态迁移调度。
    use feathrise_scene_content::{SceneCatalogAsset, SceneId}; // 测试使用正式目录句柄和稳定SceneId类型。

    #[test]
    fn sparse_startup_waits_for_fixed_mesh_and_first_height_pages() {
        let mut state = DynamicSceneRuntimeState {
            collision_ready: true, // 近场物理已经完整采用。
            surface_ready: true,   // 地表未声明或已完成预热。
            boundary_ready: true,  // 边界未声明或已完整采用。
            sparse_height_path: Some("scenes/test/terrain/height.scene_height_manifest.json".into()), // 明确选择稀疏新图门。
            ..default()
        };
        assert!(!dynamic_scene_startup_inputs_ready(&state)); // 两项首屏都缺失时不能先进入平面画面。
        state.sparse_clipmap_ready = true; // 固定环带网格先完成。
        assert!(!dynamic_scene_startup_inputs_ready(&state)); // 高度页仍在后台时继续等待。
        state.sparse_height_pages_ready = true; // 正式页组或确定性全平回退完成。
        assert!(dynamic_scene_startup_inputs_ready(&state)); // 四类证据齐全后才允许放行。
        state.sparse_height_path = None; // 旧 GLB 场景不依赖稀疏渲染资源。
        state.sparse_clipmap_ready = false; // 旧图没有固定 Clipmap 实体。
        state.sparse_height_pages_ready = false; // 旧图也没有稀疏页组。
        assert!(dynamic_scene_startup_inputs_ready(&state)); // 旧路径启动语义保持不变。
    }

    use super::*; // 测试动态场景纯派生与目录失败回退，不启动窗口或物理世界。

    #[derive(Resource, Default)] // 子加载探针证明同帧运行条件已经关闭整条准备链。
    struct DynamicPreparationProbe(bool); // true代表错误地继续运行了地形、水域等后续准备。

    /// 模拟任一动态场景子加载器；目录缺失帧不得执行。
    fn mark_dynamic_preparation_ran(mut probe: ResMut<DynamicPreparationProbe>) {
        probe.0 = true; // 若运行条件失效，测试通过该标记精确失败。
    }

    #[test] // 已删除场景不能停在永远无法放行的DynamicSceneRoot启动门。
    fn missing_catalog_entry_requests_main_menu_and_blocks_subloaders() {
        let scene_id = SceneId::parse("MAP-missing-runtime").expect("测试SceneId必须合法"); // 模拟存档仍引用已删除场景。
        let mut loading_progress = LoadingProgress::default(); // 从公共构造入口创建含私有名称快照的加载进度。
        loading_progress.stage = LoadingStage::SceneInit; // 模拟从加载页进入动态场景后遮罩仍在等待。
        loading_progress.completed = 0; // 尚无成功场景初始化证据。
        loading_progress.total = 1; // 使用合法最小工作总量。
        loading_progress.label = "正在初始化动态场景".to_owned(); // 旧实现会永久停留在这条提示。
        let mut app = App::new(); // 最小ECS App只运行目录守卫和一个子加载探针。
        app.add_plugins(StatesPlugin) // 提供StateTransition调度和NextState正式采用边界。
            .init_state::<ScenePhase>() // 提供与正式游戏相同的当前状态与NextState资源。
            .insert_resource(ActiveDynamicScene {
                scene_id: Some(scene_id.clone()), // 读档已在LoadingScene前恢复目标身份。
            })
            .insert_resource(feathrise_scene_content::SceneContentRegistry {
                catalog: Handle::<SceneCatalogAsset>::default(), // 句柄内容与本次身份查找无关。
                revision: 0,           // 首帧先模拟目录句柄已建立但资产尚未完成首次采用。
                scenes: BTreeMap::new(), // 未采用目录暂时为空不能证明场景已经删除。
                last_error: None,      // 不是目录文件损坏，只是正常异步加载窗口。
            })
            .init_resource::<DynamicSceneRuntimeState>() // 守卫关闭当前准备代次。
            .init_resource::<DynamicSceneRuntimeStatus>() // 守卫发布可诊断失败状态。
            .init_resource::<DynamicPreparationProbe>() // 记录子加载器是否被错误执行。
            .insert_resource(loading_progress) // 使用公共构造的测试进度，名称快照保持封装。
            .add_systems(
                Update,
                (
                    return_to_main_menu_if_dynamic_scene_missing, // 生产守卫先发布失败并请求主菜单。
                    mark_dynamic_preparation_ran.run_if(dynamic_scene_target_is_registered), // 同帧模拟完整子加载链。
                )
                    .chain(), // 保证运行条件读取守卫相同帧的最终目录事实。
            );

        app.update(); // 首个Update验证零修订目录仍按异步加载处理。

        assert!(matches!(
            app.world().resource::<NextState<ScenePhase>>(),
            NextState::Unchanged
        )); // 首次目录尚未采用时不得误退主菜单。
        assert!(app.world().resource::<DynamicPreparationProbe>().0); // 子加载器仍需运行并发布正式Waiting诊断。
        app.world_mut()
            .resource_mut::<feathrise_scene_content::SceneContentRegistry>()
            .revision = 7; // 模拟热目录完成采用且确认目标入口已经删除。
        app.world_mut().resource_mut::<DynamicPreparationProbe>().0 = false; // 清空首帧探针以单独验证缺失帧运行门。

        app.update(); // 第二个Update设置NextState；状态转换将在下一次更新开始时采用。

        assert!(matches!(
            app.world().resource::<NextState<ScenePhase>>(),
            NextState::Pending(ScenePhase::Idle)
        )); // 缺失目录必须立即请求正式主菜单，而不是继续停在动态场景。
        assert!(!app.world().resource::<DynamicPreparationProbe>().0); // 地形、水域等子系统不得再各自失败或制造重复日志。
        let status = app.world().resource::<DynamicSceneRuntimeStatus>(); // 读取离场前的结构化失败证据。
        assert_eq!(status.phase, DynamicSceneRuntimePhase::Failed); // 回退不能伪装成Ready或普通Waiting。
        assert_eq!(status.scene_id.as_ref(), Some(&scene_id)); // 诊断仍精确指向已删除场景。
        assert_eq!(status.detail, format!("场景目录不存在{scene_id}")); // 错误文本与生产日志保持一致。
        let progress = app.world().resource::<LoadingProgress>(); // 加载遮罩先收到失败反馈再被主菜单入口清理。
        assert_eq!(progress.stage, LoadingStage::Failed); // 不再永久显示SceneInit阶段。
        assert!(progress.label.contains("正在返回主菜单")); // 用户可见文本明确说明自动恢复动作。
    }

    #[test] // 首次GLB地形无效时必须显示具体失败，不能永久停在70%的SceneInit。
    fn first_terrain_derivation_failure_reaches_loading_overlay() {
        let scene_id = SceneId::parse("MAP-invalid-terrain").expect("测试SceneId必须合法"); // 模拟当前首次进入的作者场景。
        let terrain_detail = "动态地形三角形12194三维面积退化"; // 使用当前真实校验类型验证正文不会丢失。
        let mut terrain_status = DynamicTerrainHeightStatus::default(); // 从公开默认状态建立最小失败快照。
        terrain_status.phase = DynamicTerrainHeightPhase::Failed; // 模拟捕获或后台派生拒绝当前GLB。
        terrain_status.scene_id = Some(scene_id.clone()); // 失败必须关联具体场景身份。
        terrain_status.detail = terrain_detail.to_owned(); // 保存作者可直接定位的底层原因。
        let mut runtime_status = DynamicSceneRuntimeStatus::default(); // 模拟尚未由具体失败分支提升的场景级状态。
        runtime_status.phase = DynamicSceneRuntimePhase::BuildingCollision; // 旧行为会一直停留在碰撞构建阶段。
        let mut progress = LoadingProgress::default(); // 模拟跨入玩法状态后仍保留的加载遮罩资源。
        progress.stage = LoadingStage::SceneInit; // 截图中的旧状态固定显示70%。
        progress.label = "正在初始化场景...".to_owned(); // 旧正文没有透露真实失败原因。
        let mut app = App::new(); // 最小ECS App只执行首次失败桥接系统。
        app.insert_resource(DynamicSceneRuntimeState::default()) // 默认ready_released=false代表从未有完整地形。
            .insert_resource(terrain_status) // 注入GLB派生失败证据。
            .insert_resource(runtime_status) // 注入仍未失败的场景级状态。
            .insert_resource(progress) // 注入仍停留SceneInit的加载页状态。
            .init_resource::<feathrise_scene_content::SceneTerrainDisplayLodStatus>()
            .add_systems(Update, publish_dynamic_scene_initial_failure); // 执行生产桥接逻辑。

        app.update(); // 同一Update应把场景状态和加载遮罩一起切为Failed。

        let runtime_status = app.world().resource::<DynamicSceneRuntimeStatus>(); // 核对结构化场景级失败。
        let expected_detail = format!("场景{scene_id}从GLB生成内存高度失败：{terrain_detail}"); // 构造生产预期文案。
        assert_eq!(runtime_status.phase, DynamicSceneRuntimePhase::Failed); // 首载门必须明确终止。
        assert_eq!(runtime_status.scene_id.as_ref(), Some(&scene_id)); // 失败身份不能丢失。
        assert_eq!(runtime_status.detail, expected_detail); // 场景级诊断保留具体坏三角形。
        let progress = app.world().resource::<LoadingProgress>(); // 核对玩家实际看到的跨场景遮罩。
        assert_eq!(progress.stage, LoadingStage::Failed); // 标题与百分比不再冒充初始化中。
        assert_eq!(progress.completed, 0); // 失败没有伪完成项。
        assert_eq!(progress.total, 1); // 进度分母保持合法。
        assert_eq!(progress.label, expected_detail); // 正文直接显示GLB修复原因。
    }

    #[test] // 已有完整版本时GLB热更失败必须继续显示旧地形，不重新打开加载失败遮罩。
    fn hot_reload_failure_keeps_running_version_and_loading_state() {
        let scene_id = SceneId::parse("MAP-hot-reload").expect("测试SceneId必须合法"); // 模拟已经进入玩法的动态场景。
        let mut state = DynamicSceneRuntimeState::default(); // 建立当前运行时完成证据。
        state.ready_released = true; // 表示碰撞、画面、水域和植被已经共同采用过。
        let mut terrain_status = DynamicTerrainHeightStatus::default(); // 新GLB版本仍可独立发布失败。
        terrain_status.phase = DynamicTerrainHeightPhase::Failed; // 模拟Blender保存了无效的新拓扑。
        terrain_status.scene_id = Some(scene_id); // 失败仍归属当前运行场景。
        terrain_status.detail = "新GLB存在折叠面".to_owned(); // 保存热更诊断但不撤销旧版本。
        let mut runtime_status = DynamicSceneRuntimeStatus::default(); // 当前场景级状态继续代表已采用版本。
        runtime_status.phase = DynamicSceneRuntimePhase::Ready; // 旧完整版本仍然可玩。
        runtime_status.detail = "上一完整动态场景仍在运行".to_owned(); // 热更失败不得覆盖该完成证据。
        let mut progress = LoadingProgress::default(); // 防御性保留一个不应被触碰的进度资源。
        progress.stage = LoadingStage::DynamicEntities; // 使用非Failed状态证明系统会完整早退。
        progress.label = "原加载状态".to_owned(); // 记录不可覆盖的正文。
        let mut app = App::new(); // 最小ECS App验证热更分支。
        app.insert_resource(state) // 注入已有完整版本证据。
            .insert_resource(terrain_status) // 注入新版本失败。
            .insert_resource(runtime_status) // 注入当前Ready状态。
            .insert_resource(progress) // 注入防御性加载资源。
            .init_resource::<feathrise_scene_content::SceneTerrainDisplayLodStatus>()
            .add_systems(Update, publish_dynamic_scene_initial_failure); // 执行同一生产桥接系统。

        app.update(); // ready_released=true应在任何写入前返回。

        let runtime_status = app.world().resource::<DynamicSceneRuntimeStatus>(); // 核对旧运行时状态未被撤销。
        assert_eq!(runtime_status.phase, DynamicSceneRuntimePhase::Ready); // 热更失败不使玩法场景整体失败。
        assert_eq!(runtime_status.detail, "上一完整动态场景仍在运行"); // 成功版本诊断保持不变。
        let progress = app.world().resource::<LoadingProgress>(); // 核对没有重新盖上失败页。
        assert_eq!(progress.stage, LoadingStage::DynamicEntities); // 进度阶段保持调用前状态。
        assert_eq!(progress.label, "原加载状态"); // 进度正文保持调用前状态。
    }

    #[test] // 默认新场景的-80米编码下限必须全部视为合法可雕刻空间。
    fn fall_death_boundary_stays_below_the_complete_height_range() {
        assert_eq!(dynamic_fall_death_y(-80.0), Some(-96.0)); // 最深河床以下再留16米才算掉出世界。
        assert_eq!(dynamic_fall_death_y(0.0), Some(-16.0)); // 不同高度原点仍使用相同物理余量。
        assert_eq!(dynamic_fall_death_y(f32::NAN), None); // 损坏清单不得关闭死亡检测。
        assert_eq!(dynamic_fall_death_y(f32::NEG_INFINITY), None); // 无穷高度必须拒绝。
    }

    #[test] // 双名称契约必须同时成立，防止作者工具误接管其他primitive。
    fn dynamic_terrain_source_requires_mesh_and_white_material_identity() {
        assert!(dynamic_terrain_source_identity(
            // 正式Rust白模名称组合应被接受。
            super::super::DYNAMIC_TERRAIN_MESH_NAME, // 使用动态场景唯一网格名。
            super::super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME, // 使用唯一白模材质名。
        ));
        assert!(!dynamic_terrain_source_identity(
            // 只有网格名相同仍不足以取得所有权。
            super::super::DYNAMIC_TERRAIN_MESH_NAME, // 保持网格名正确。
            "其他材质",                              // 错误材质必须拒绝。
        ));
    }

    #[test] // 正式显示复制GLB名称后仍必须保持可见，不能在下一帧被源发现系统重新隐藏。
    fn derived_terrain_display_and_surface_warmup_are_never_reclassified_as_hidden_glb_sources() {
        let mut app = App::new(); // 最小ECS应用足以执行Added过滤与延迟组件写入。
        let root = app.world_mut().spawn_empty().id();
        app.insert_resource(DynamicSceneRuntimeState { root: Some(root), ..default() });
        app.add_systems(Update, mark_dynamic_scene_terrain_primitive); // 使用生产源识别系统验证完整查询契约。
        let source = app
            .world_mut()
            .spawn((
                ChildOf(root),
                GltfMeshName(super::super::DYNAMIC_TERRAIN_MESH_NAME.to_owned()), // 原始GLB网格拥有稳定名称。
                GltfMaterialName(super::super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.to_owned()), // 原始白模材质完成双名称身份。
                Visibility::Inherited, // 场景实例化时尚未被源系统隐藏。
            ))
            .id(); // 保存原始输入实体供采用后核对。
        let display = app
            .world_mut()
            .spawn((
                ChildOf(root),
                crate::DynamicSceneTerrainPrimitive, // 后台派生完成后创建的唯一正式显示身份。
                GltfMeshName(super::super::DYNAMIC_TERRAIN_MESH_NAME.to_owned()), // 地表系统仍需沿用原网格名。
                GltfMaterialName(super::super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.to_owned()), // 地表系统仍需沿用原材质名。
                Visibility::Inherited, // 正式玩法地形必须保持可见。
            ))
            .id(); // 保存正式显示实体供误分类回归核对。
        let surface_warmup = app
            .world_mut()
            .spawn((
                ChildOf(root),
                TerrainSurfaceCarrier, // 离屏预热与活动地表共同使用的材质承接身份。
                GltfMeshName(super::super::DYNAMIC_TERRAIN_MESH_NAME.to_owned()), // 预热副本沿用原网格名完成同管线专用化。
                GltfMaterialName(super::super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.to_owned()), // 预热副本沿用原材质名但不是GLB源。
                Visibility::Inherited, // 离屏预热必须进入RenderApp才能取得GPU就绪证据。
            ))
            .id(); // 保存预热实体供二次误分类回归核对。

        let preview_root = app.world_mut().spawn_empty().id();
        let asset_preview = app.world_mut().spawn((
            ChildOf(preview_root),
            GltfMeshName(super::super::DYNAMIC_TERRAIN_MESH_NAME.to_owned()),
            GltfMaterialName(super::super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.to_owned()),
            Visibility::Inherited,
        )).id(); // 同一GLB的离屏预览有相同双名称，但不能取得场景地形身份或被隐藏。

        app.update(); // 应用Added查询及其延迟Commands。

        assert!(app.world().get::<DynamicSceneTerrainSourcePrimitive>(asset_preview).is_none());
        assert_eq!(app.world().get::<Visibility>(asset_preview), Some(&Visibility::Inherited));

        assert!(
            app.world()
                .get::<DynamicSceneTerrainSourcePrimitive>(source)
                .is_some()
        ); // 原始GLB primitive仍取得唯一隐藏输入身份。
        assert_eq!(
            app.world().get::<Visibility>(source).copied(),
            Some(Visibility::Hidden)
        ); // 原始输入继续禁止直接进入正式画面。
        assert!(
            app.world()
                .get::<DynamicSceneTerrainSourcePrimitive>(display)
                .is_none()
        ); // 正式显示即使名称相同也不能进入派生源集合。
        assert_eq!(
            app.world().get::<Visibility>(display).copied(),
            Some(Visibility::Inherited)
        ); // F12关闭后正式地形可由玩法相机正常渲染。
        assert!(
            app.world()
                .get::<DynamicSceneTerrainSourcePrimitive>(surface_warmup)
                .is_none()
        ); // 同名离屏预热不能取得原始GLB源身份。
        assert_eq!(
            app.world().get::<Visibility>(surface_warmup).copied(),
            Some(Visibility::Inherited)
        ); // 预热跨帧时仍可被RenderApp提取并完成材质准备。
    }

    #[test]
    fn legacy_weather_descriptor_adopts_only_neutral_climate() {
        assert_eq!(
            dynamic_scene_weather_profile(SceneWeatherDescriptor::default()),
            UNIFORM_CLIMATE_PROFILE,
        );
        // 同时覆盖旧清单读取与实际采用，避免只改了默认值，旧河滩身份仍能激活倍率。
        for climate in ["kaifeng_plain", "yellow_river_floodplain"] {
            let descriptor: SceneWeatherDescriptor = serde_json::from_value(serde_json::json!({
                "region": "kaifeng",
                "climate": climate,
            }))
            .expect("旧场景天气字段应能读取");
            assert_eq!(
                dynamic_scene_weather_profile(descriptor),
                UNIFORM_CLIMATE_PROFILE,
            );
        }
    }
}
