//! 通用地表材质插件与RenderApp就绪观察系统。
//! 【功能设计】沿用原草土地表“真实材质实例、实际专用管线、Forward/Deferred分箱”三重证据，
//! 地图层只有取得完整GPU结果后才可把离屏预热材质原子转交给白模地形。

use std::any::TypeId; // 类型擦除材质实例仍需精确匹配TerrainSurfaceMaterial。
use bevy::core_pipeline::core_3d::Opaque3d; // 通用地表固定进入不透明PBR阶段。
use bevy::core_pipeline::deferred::Opaque3dDeferred; // Deferred材质在预通道提交正式表面，不进入Forward分箱。
use bevy::pbr::{
    // 注册扩展材质并读取Bevy材质后端的真实实例与专用化缓存。
    MaterialPlugin,
    RenderMaterialInstances,
    SpecializedMaterialPipelineCache,
    SpecializedPrepassMaterialPipelineCache,
    queue_material_meshes,
    queue_prepass_material_meshes,
};
use bevy::prelude::{App, IntoScheduleConfigs, Plugin, Query, Res, ResMut, With, Without}; // 插件、系统参数与调度约束。
use bevy::render::extract_component::ExtractComponentPlugin; // 同步承接标记和完整纹理组探针。
use bevy::render::render_asset::{RenderAssets, prepare_assets}; // PrepareAssets后检查当前模式全部图片是否真实上传GPU。
use bevy::render::render_phase::ViewBinnedRenderPhases; // 检查地表专用管线是否进入Opaque3d分箱。
use bevy::render::render_resource::{
    // 区分异步等待、成功和明确WGSL或设备错误。
    CachedPipelineState,
    PipelineCache,
};
use bevy::render::sync_world::MainEntity; // 将渲染实体映射到材质实例缓存使用的主世界身份。
use bevy::render::texture::GpuImage; // RenderAssets中的GPU图片类型。
use bevy::render::view::{ExtractedView, RenderVisibleEntities}; // 缓存管线必须对应本帧同视图可见的真实承接实体。
use bevy::render::{
    // 注册渲染世界资源和上一完整帧发布系统。
    ExtractSchedule,
    MainWorld,
    Render,
    RenderApp,
    RenderSystems,
};

use super::components::{TerrainSurfaceCarrier, TerrainSurfaceTextureProbe, TerrainSurfaceWarmup}; // 只观察显式地表承接实体和唯一纹理探针。
use super::material::TerrainSurfaceMaterial; // 精确核对当前实体绑定的通用扩展材质类型。
use super::resources::{
    // 渲染帧内部计数、内部零权重图与主世界F3快照。
    TerrainSurfaceFallbackImages,
    TerrainSurfaceRenderFrameCounters,
    TerrainSurfaceRenderStats,
};
use super::shaders::register_terrain_surface_shader; // 插件启用时注册内置WGSL。

/// 安装通用地表材质、组件提取和RenderApp状态观察能力。
#[derive(Clone, Copy, Debug, Default)] // 插件无运行时配置，可由统一EnvironmentPlugin确定性安装一次。
pub struct TerrainSurfacePlugin; // 地图资产路径、层参数和白模接管仍由场景适配器负责。

impl Plugin for TerrainSurfacePlugin {
    // 将主世界材质资产与渲染世界证据链集中装配。
    fn build(&self, app: &mut App) {
        // Bevy插件构建只登记固定类型和系统，不加载地图资产。
        super::images::install_shape_normalization(app); // 异步KTX2首次GPU抽取前恢复本入口的明确二维/数组合同。
        register_terrain_surface_shader(app); // 在MaterialPlugin开始专用化前注册固定WGSL句柄。
        super::weather::install_surface_weather(app); // 所有地表和水面共用唯一64字节天气缓冲。
        app.init_resource::<TerrainSurfaceRenderStats>(); // 无RenderApp逻辑测试也能读取稳定全零快照。
        app.init_resource::<super::resources::TerrainSurfaceInstanceFeedback>();
        app.add_plugins(MaterialPlugin::<TerrainSurfaceMaterial>::default()); // 复用StandardMaterial完整PBR和Deferred路径。
        app.init_resource::<TerrainSurfaceFallbackImages>(); // 单权重图地图共用唯一内部零RGBA句柄。
        app.add_plugins((
            // 两个轻量组件使用官方同步提取路径。
            ExtractComponentPlugin::<TerrainSurfaceCarrier>::default(), // 同步正式与预热承接标记。
            ExtractComponentPlugin::<TerrainSurfaceTextureProbe>::default(), // 同步完整强句柄组和失败位。
            ExtractComponentPlugin::<TerrainSurfaceWarmup>::default(), // 根图只聚合本次候选，不能被隐藏旧活动源挡住。
            ExtractComponentPlugin::<super::components::TerrainSurfaceInstanceScope>::default(),
            ExtractComponentPlugin::<super::components::TerrainSurfaceInstanceWarmup>::default(),
        )); // 完成组件提取插件安装。
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            // 逻辑测试允许没有独立渲染子应用。
            return; // 主世界仍保留材质类型和默认诊断资源。
        }; // 完成RenderApp存在性检查。
        super::instance_render::install(render_app);
        render_app // 在渲染世界建立固定计数器和两个小系统。
            .init_resource::<TerrainSurfaceRenderFrameCounters>() // 所有字段为无锁原子且每帧边界归零。
            .add_systems(ExtractSchedule, publish_terrain_surface_render_stats) // 帧首把上一完整渲染帧证据写回主世界。
            .add_systems(
                // Bevy材质排队完成后观察真实管线与Opaque项。
                Render, // 使用标准渲染调度阶段。
                observe_terrain_surface_pipeline
                    .in_set(RenderSystems::QueueMeshes) // 与标准材质排队位于同一阶段读取最终分箱。
                    .after(queue_material_meshes) // 必须等Bevy清理旧分箱并写入当前材质结果。
                    .after(queue_prepass_material_meshes) // Deferred的真实排队与Forward同样属于完整预热门。
                    .after(prepare_assets::<GpuImage>), // 完整纹理组必须完成本帧GPU准备后才可报就绪。
            ); // 完成RenderApp观察系统注册。
    } // 完成插件构建。
} // 完成插件实现。

/// 把上一完整RenderApp帧的固定证据写回主世界，同时保留地图生命周期字段。
fn publish_terrain_surface_render_stats(
    // ExtractSchedule是一帧延迟但不会读取半帧计数。
    mut main_world: ResMut<MainWorld>, // Bevy官方RenderApp回写主世界资源入口。
    frame_counters: Res<TerrainSurfaceRenderFrameCounters>, // 取走上一完整帧内部原子计数。
) {
    // 发布过程没有分配、实体扫描或GPU回读。
    let mut snapshot = frame_counters.snapshot_and_reset(); // 一次取走全部RenderApp字段并开始新帧计数。
    if let Some(mut stats) = main_world.get_resource_mut::<TerrainSurfaceRenderStats>() {
        // 插件拆装测试允许资源临时不存在。
        snapshot.phase = stats.phase; // 地图生命周期由主世界负责，RenderApp不得覆盖。
        snapshot.failure = stats.failure; // 具体失败项同样由地图生命周期系统发布。
        snapshot.generation = stats.generation; // 当前地图代次同样由主世界维护。
        *stats = snapshot; // 原子覆盖其余上一完整渲染帧证据。
    } // 完成可选主世界资源发布。
} // 完成地表快照发布系统。

/// 在标准材质完成排队后观察完整纹理组、材质实例、专用管线和Opaque分箱。
#[allow(clippy::too_many_arguments)] // 各资源代表独立真实证据，聚合参数会隐藏缺失状态和调度借用。
fn observe_terrain_surface_pipeline(
    // 系统只遍历唯一探针、唯一承接实体和当前视图小型管线缓存。
    surface_carriers: Query<(&MainEntity, Option<&TerrainSurfaceWarmup>), (With<TerrainSurfaceCarrier>, Without<super::components::TerrainSurfaceInstanceScope>)>, // 父图证据不包含子实例。
    texture_probes: Query<&TerrainSurfaceTextureProbe, Without<super::components::TerrainSurfaceInstanceScope>>, // 无网格探针独立表达当前模式全部纹理的上传和失败状态。
    images: Res<RenderAssets<GpuImage>>,                // 图片存在于该表才证明已经可供GPU绑定。
    material_instances: Res<RenderMaterialInstances>,   // 核对承接标记没有脱离真实扩展材质实例。
    specialized_pipelines: Res<SpecializedMaterialPipelineCache>, // 取得每个承接实体在各视图使用的准确管线ID。
    pipeline_cache: Res<PipelineCache>,                           // 查询每个管线的异步创建状态。
    opaque_phases: Res<ViewBinnedRenderPhases<Opaque3d>>, // 确认至少一个匹配管线进入真实绘制分箱。
    prepass_pipelines: Res<SpecializedPrepassMaterialPipelineCache>,
    deferred_phases: Res<ViewBinnedRenderPhases<Opaque3dDeferred>>,
    views: Query<(&ExtractedView, &RenderVisibleEntities)>,
    frame_counters: Res<TerrainSurfaceRenderFrameCounters>, // 把当前证据保存到下一帧主世界快照。
) {
    // 完成全部只读RenderApp依赖声明。
    let mut expected_textures = 0_u32; // HN-01固定路径为三项，动态索引PBR路径为五项。
    let mut ready_textures = 0_u32; // 分别核对每个强句柄是否已经上传GPU。
    let mut texture_failed = false; // 任一探针失败都使整组材质不可采用。
    for probe in &texture_probes {
        // 热重建瞬间若短暂出现两个探针仍按全部资产聚合。
        expected_textures = expected_textures.saturating_add(probe.images.len() as u32); // 固定数组长度转换不会失败。
        ready_textures = ready_textures.saturating_add(
            // 只计算GpuImage表真实存在的句柄。
            probe
                .images
                .iter()
                .filter(|image| images.get(*image).is_some())
                .count() as u32, // 不读取或复制任何图片像素。
        ); // 完成当前探针上传数量累计。
        texture_failed |= probe.failed; // 失败优先于普通上传等待。
    } // 完成纹理探针聚合。
    frame_counters.record_textures(ready_textures, expected_textures, texture_failed); // 发布本帧完整纹理组证据。
    let warming = surface_carriers.iter().any(|(_, warmup)| warmup.is_some());
    // 候选存在时只验候选；F12隐藏的旧活动网格继续托底，但不能与当前预热版本混成一个统计集合。
    let carrier_instances = surface_carriers.iter().filter(|(_, warmup)| !warming || warmup.is_some()).count() as u32;
    if carrier_instances == 0 {
        // 白模、非HN-01或预热尚未生成时没有自定义材质。
        frame_counters.record_pipeline(0, false, false, false, false); // 无承接实体保持管线全零而非永久等待。
        return; // 跳过全局材质和视图缓存检查。
    } // 完成无承接实体快速路径。
    let material_type = TypeId::of::<TerrainSurfaceMaterial>(); // 类型擦除资产ID仍保留原始材质TypeId。
    let mut waiting = false;
    let mut error = false;
    let mut queued = true; // 每个承接实体都须有自身当前视图的实际排队证据。
    for (main_entity, warmup) in &surface_carriers {
        if warming && warmup.is_none() { continue; }
        // 承接实体数量固定很小且只在预热期间可能超过一。
        let has_surface_material = material_instances
            .instances
            .get(main_entity)
            .is_some_and(|instance| instance.asset_id.type_id() == material_type); // 标记和材质必须在同一RenderApp帧同时存在。
        if !has_surface_material {
            // 材质提取尚未跟上组件提取时不能误读其他类型管线。
            waiting = true;
            queued = false;
            continue; // 保持当前实体处于等待。
        } // 完成材质类型核对。
        let evidence = terrain_surface_pipeline_evidence(*main_entity, &views,
            &specialized_pipelines, &prepass_pipelines, &pipeline_cache, &opaque_phases, &deferred_phases);
        waiting |= evidence.waiting;
        error |= evidence.error;
        queued &= evidence.queued;
    } // 完成全部承接实体专用化检查。
    let ready = !waiting && !error; // 缺少真实实体管线时帮助函数始终保持waiting。
    frame_counters.record_pipeline(
        // 一次发布本帧唯一地表管线状态。
        carrier_instances, // 保存RenderApp承接实体数。
        ready,             // 保存完整就绪标志。
        waiting,           // 保存异步等待标志。
        error,             // 保存明确失败标志。
        queued,            // 保存Opaque排队证据。
    ); // 完成管线证据发布。
} // 完成地表RenderApp观察系统。

/// 父图与独立实例共用同一条Forward/Deferred证据链，不借同管线的其他地图或陈旧可见性。
#[derive(Default)]
pub(super) struct SurfacePipelineEvidence {
    pub waiting: bool,
    pub error: bool,
    pub queued: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn terrain_surface_pipeline_evidence(
    entity: MainEntity,
    views: &Query<(&ExtractedView, &RenderVisibleEntities)>,
    pipelines: &SpecializedMaterialPipelineCache,
    prepass: &SpecializedPrepassMaterialPipelineCache,
    cache: &PipelineCache,
    opaque: &ViewBinnedRenderPhases<Opaque3d>,
    deferred: &ViewBinnedRenderPhases<Opaque3dDeferred>,
) -> SurfacePipelineEvidence {
    let mut evidence = SurfacePipelineEvidence::default();
    let mut found = false;
    for (view, visible) in views {
        // Bevy按主世界实体排序CPU可见表；二分/哈希查询不扫描全图，隐藏实体的旧缓存不能续借同管线。
        if !visible.get::<bevy::prelude::Mesh3d>().is_some_and(|visible| {
            // Mesh提取可能保留真实渲染实体或PLACEHOLDER；证据与材质缓存都由完整MainEntity确定。
            visible.entities_cpu_culling.binary_search_by_key(&entity, |(_, main)| *main).is_ok()
                || visible.entities_gpu_culling.contains_key(&entity)
        }) { continue; }
        let view = view.retained_view_entity;
        if let Some(id) = pipelines.get(&view).and_then(|pipelines| pipelines.get(&entity)) {
            found = true;
            match cache.get_render_pipeline_state(*id) {
                CachedPipelineState::Ok(_) => {},
                CachedPipelineState::Err(_) => evidence.error = true,
                _ => evidence.waiting = true,
            }
            // 只允许同一视图的分箱确认该视图专用化结果，不能从另一张辅助视图借排队证据。
            evidence.queued |= opaque.get(&view).is_some_and(|phase| {
                phase.multidrawable_meshes.keys().any(|key| key.pipeline == *id)
                    || phase.batchable_meshes.keys().any(|(key, _)| key.pipeline == *id)
                    || phase.unbatchable_meshes.keys().any(|(key, _)| key.pipeline == *id)
                    || phase.non_mesh_items.keys().any(|(key, _)| key.pipeline == *id)
            });
        }
        if let Some((_, id, _)) = prepass.get(&view).and_then(|pipelines| pipelines.get(&entity)) {
            found = true;
            match cache.get_render_pipeline_state(*id) {
                CachedPipelineState::Ok(_) => {},
                CachedPipelineState::Err(_) => evidence.error = true,
                _ => evidence.waiting = true,
            }
            // 深度/法线预通道本身不能证明地表已排队；必须真实进入Deferred表面分箱。
            evidence.queued |= deferred.get(&view).is_some_and(|phase| {
                phase.multidrawable_meshes.keys().any(|key| key.pipeline == *id)
                    || phase.batchable_meshes.keys().any(|(key, _)| key.pipeline == *id)
                    || phase.unbatchable_meshes.keys().any(|(key, _)| key.pipeline == *id)
                    || phase.non_mesh_items.keys().any(|(key, _)| key.pipeline == *id)
            });
        }
    }
    evidence.waiting |= !found;
    evidence
}
