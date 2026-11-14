//! 目录驱动动态场景场景。
//!
//! 所有编辑器新建场景共用一个GameScene状态，ActiveDynamicScene保存真正SceneId，
//! 原始GLB与稀疏Clipmap采用互斥地形路线；后者显示与固定分块物理同源、独立LOD。

use feathrise_scene_core::residency::SceneInstanceContextAppExt; // 父资源与子实例资源表共同参加场景驻留。
use feathrise_scene_core::residency::SceneResidencyAppExt;

mod boundary; // 场景私有边界JSON驱动静态空气墙、热更新与F12未保存覆盖。
mod clipmap; // 新建稀疏高度图采用固定环带网格，避免地形规模决定网格顶点数。
#[cfg(any(feature = "test_panel", feature = "author_view"))]
pub(crate) use clipmap::ClipmapHeightMaterial; // 编辑器渲染覆盖复用高度兜底类型，不开放Clipmap内部模块。
#[cfg(all(test, any(feature = "test_panel", feature = "author_view")))]
pub(crate) use clipmap::{ClipmapHeightExtension, ClipmapFlatHeightPage, ClipmapBounds, ClipmapHeightMaterialPlugin}; // 真实GPU回归复用正式位移材质与Shader。
pub use clipmap::overlay::{
    clipmap_brush_overlay_evidence, ClipmapBrushOverlayEvidence,
    CLIPMAP_VEGETATION_OVERLAY_LAYER, ClipmapVegetationBrush, ClipmapVegetationDensity, ClipmapOverlaySet,
    DynamicClipmapVegetationOverlay,
    DynamicClipmapSurfaceOverlay, ClipmapSurfaceCoverage, ClipmapSurfaceOverlayIdentity,
}; // 编辑器只发布贴地覆盖参数与共享图片。
mod layered_collision; // Clipmap唯一碰撞方案：玩家中心、可配置多层的区域共享碰撞。
mod mesh_profiles; // 树木、灌木和岩石按需复用正式四级LOD与风材质原型。
mod resources; // 动态场景加载事务和公开阶段。
mod instances; // 同一父世界内的完整引用场景实例。
pub use instances::{SubsceneActive, SubsceneInstance, SubsceneRuntimeStatus, TerrainObjectInstance, TerrainObjectTransformPreview};
#[cfg(feature = "runtime_test_support")]
pub use instances::runtime_test_support;
pub use instances::TerrainObjectAuthoringTarget;
pub use instances::TerrainObjectStitchStatus;
pub use instances::TerrainVegetationDrafts;
pub use instances::TerrainWaterDrafts;
pub use instances::TerrainObjectResource;
pub use instances::TerrainObjectEditorVisibility;
pub use instances::TerrainObjectClipmapGround;
pub use instances::TerrainAuthoringApply;
pub use instances::TerrainVolumeAuthorPreviewRoot;
mod voxel_objects; // 独立密度源共用正式等值面与接缝简化，显示和碰撞分别选择LOD。
mod terrain_queries; // 编辑器和玩法通过同一对象身份冻结真实表面查询。
pub use terrain_queries::{TerrainObjectQueries, TerrainObjectRaySnapshot, TerrainSurfaceVersion};
mod session_crater; // 技能浅坑仅作用于当前场景会话，不写F12作者高度。
mod sparse_runtime; // 新场景稀疏清单和固定预算 Clipmap 网格的异步采用。
mod sparse_lod; // 编辑器与玩法共用当前相机的投影格距和全局观察中心。
mod surface; // 数据驱动RGBA地表的白模预热、失败回退与恢复。
mod systems; // 进入场景、清单热更、白模生成与碰撞就绪。
pub(crate) use systems::DynamicSceneTerrainRoot; // 水下介质射线识别装在正式根上的完整GLB碰撞。
mod terrain_derived; // 从唯一GLB地形分帧捕获拓扑并后台派生内存高度、GPU单元纹理和Trimesh。
mod terrain_topology; // 静态三维地形与高度面洞口联合采用。
mod volume_stream;
mod glb_objects; // 原始多primitive GLB保留Scene0，只派生复合碰撞。
pub(crate) mod sources; // 正式场景目录与运行时容器投影分离，关联内容只由所属地形加载。
pub(crate) mod author_sources; // 旧图未保存作者版本仍由显式兼容主地形接管。
#[cfg(test)]
mod terrain_queries_tests;
pub use terrain_topology::{
    StaticTerrainRaytracingSources, TerrainTopologySystems, TerrainVolumePreviewRequest,
    TerrainVolumePreviewTarget,
};
pub use volume_stream::{
    StreamedVolumeResident, TerrainVolumeStreamControl, TerrainVolumeStreamStatus,
    TerrainVolumeStreamSystems,
    VolumeSurfaceFields, VolumeDisplayBuild, build_volume_display, volume_lod_signature,
};
mod vegetation; // 独立密度层驱动GPU叶片窗口与空间分块Mesh散布。
mod water; // 场景私有R8覆盖驱动正式GPU水面、波浪和踏水波纹。

pub use feathrise_terrain::{DYNAMIC_TERRAIN_MESH_NAME, DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME}; // 场景、F12和GLB编解码共用唯一名称契约。

pub use boundary::{DynamicSceneBoundaryOverride, DynamicSceneBoundaryPhase, DynamicSceneBoundaryStatus}; // F12作者工具发布未保存边界并读取正式碰撞阶段。
pub use clipmap::{
    ClipmapGroundHit, ClipmapGroundPublishSet, ClipmapGroundSnapshot, DynamicClipmapGroundSurface,
}; // Clipmap作者笔刷的只读地表查询。
#[cfg(test)]
pub(crate) use clipmap::test_ground_snapshot; // 水下集成回归复用正式坡面夹具，不扩大生产接口。
pub use clipmap::{
    ClipmapRaytracingAnchor, ClipmapRaytracingInstanceRoute, ClipmapRaytracingPhase,
    ClipmapRaytracingPublishSet, ClipmapRaytracingRequest, ClipmapRaytracingStatus,
    SparseClipmapSurfaceMaterial,
};
pub use layered_collision::LayeredCollisionDiagnosticsSystemSet; // F1/F5在完整采用和活动门之后读取新方案诊断。
pub use layered_collision::validate_layered_collision_settings; // 编辑器后台保存复用真实分块预算检查。
pub use mesh_profiles::{
    DynamicMeshProfileReadiness,
    DynamicMeshProfilePhase, DynamicMeshProfileRequests, DynamicMeshProfileStatus,
};
pub use resources::{DynamicSceneRuntimePhase, DynamicSceneRuntimeStatus}; // F12和F3读取结构化状态。
pub use sparse_runtime::DynamicClipmapAuthoringView; // F12可登记独立离屏观察相机。
pub use surface::DynamicSurfaceAuthorPreview; // 完整GPU作者地表的正式源输入。
pub use terrain_derived::{
    DynamicTerrainHeightCache, DynamicTerrainHeightData, DynamicTerrainHeightPhase,
    DynamicTerrainHeightStatus,
}; // F12、水域与植被共享同一GLB派生完成值，不再读取磁盘高度图。
pub use vegetation::{DynamicBladeDraft, DynamicBladeDraftSnapshot}; // 同图未保存Blade复用正式运行时。
pub use vegetation::DynamicModelVegetationDraft;
pub use vegetation::DynamicModelVegetationStatus;
pub use vegetation::{DynamicVegetationPhase, DynamicVegetationStatus}; // 公开动态植被加载、空间常驻和热更证据。
pub(crate) use water::ClipmapRainHeightMaterial; // 雨水高度捕获复用同一高度页、R8与水深裁切。
pub use water::{
    ClipmapSplineWaterSurface, ClipmapWaterMaterial, ClipmapWaterSurface, DynamicClipmapWaterDraft,
    DynamicClipmapWaterDraftSnapshot, DynamicWaterPhase, DynamicWaterRebuildRequest, TerrainWaterRebuildRequest,
    DynamicWaterStatus,
}; // F12/F3读取状态，Clipmap草稿以完整不可变快照交给正式水运行时。
#[cfg(test)]
pub(crate) use water::{test_clipmap_water_material, test_clipmap_water_surface};

use avian3d::schedule::{PhysicsSchedule, PhysicsStepSystems}; // 承托空窗必须在Tnua电机之后、Avian碰撞步之前再次冻结。
use bevy::prelude::*; // App、Plugin与场景调度。
use bevy::transform::TransformSystems; // 玩家跟随草窗必须在全局变换传播前同步AABB。
use bevy_tnua::TnuaSystems;
use feathrise_atmosphere::{DynamicSceneWeatherClimate, WorldTimeSystemSet}; // 动态清单发布气候，天体配置必须在最终光照前采用。
use feathrise_scene_core::state::{ScenePhase, SceneFallBoundary}; // 所有作者场景共用场景并发布场景专用虚空边界。 // 以Tnua公开总集合建立跨插件的确定性速度写入顺序。

/// 动态场景清单、白模GLB和碰撞运行时。
pub(super) struct DynamicScenePlugin;

impl Plugin for DynamicScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(clipmap::ClipmapHeightMaterialPlugin); // 先登记新图外层材质及所有内置位移Shader。
        volume_stream::install(app);
        vegetation::install_volume(app);
        vegetation::install_models(app);
        water::install_volume(app);
        app.add_plugins(clipmap::ClipmapRaytracingPlugin); // 关闭态不构建；开启后准备世界固定真实高度源。
        app.add_plugins(water::ClipmapWaterMaterialPlugin); // 复用已登记动态水外观，并增加Clipmap高度/R8专用外层。
        instances::install(app); // 所有子场景共用有界事务宿主，正式父图仍沿用原入口。
        author_sources::install(app); // 容器清理与对象作者覆盖分离，未保存结构也使用同一隔离合同。
        voxel_objects::install(app);
        glb_objects::install(app);
        sources::install(app);
        #[cfg(any(feature = "test_panel", feature = "author_view"))] // 发布构建不安装F12高度感知线框材质。
        app.add_plugins(clipmap::ClipmapTerrainGridPlugin); // 必须晚于正式高度Shader注册，复用同一内置句柄。
        // 目录是全局服务，冷图切入不能清空，返回旧图也不能回滚；仅实例私有句柄表参加驻留。
        app.init_resource::<feathrise_scene_content::SceneContentRegistry>()
            .register_instance_resource::<feathrise_scene_content::SceneContentRegistry>()
            .init_instance_resource::<resources::DynamicSceneRuntimeState>()
            .init_instance_resource::<terrain_topology::TerrainTopologyRuntime>()
            .init_instance_resource::<TerrainVolumePreviewRequest>()
            .init_instance_resource::<StaticTerrainRaytracingSources>()
            .init_instance_resource::<DynamicSceneRuntimeStatus>()
            .init_instance_resource::<DynamicSceneWeatherClimate>() // 场景独立测试未安装完整天气插件时仍具备安全空配置。
            .init_instance_resource::<SceneFallBoundary>() // 动态场景独立测试和完整游戏共用同一死亡边界契约。
            .init_instance_resource::<boundary::DynamicSceneBoundaryState>()
            .add_resident_systems(
                Update,
                (boundary::show_air_walls, boundary::show_air_wall_outlines)
                    .chain()
                    .after(boundary::prepare_dynamic_boundaries),
            ) // 相机层最后切换，失效旧轮廓同帧隐藏。
            .init_instance_resource::<DynamicSceneBoundaryStatus>()
            .init_instance_resource::<DynamicSceneBoundaryOverride>()
            .init_instance_resource::<surface::DynamicSurfaceState>()
            .init_instance_resource::<surface::DynamicSurfaceAuthorPreview>()
            .init_instance_resource::<terrain_topology::VolumeSurfaceState>()
            .init_instance_resource::<session_crater::CraterSurfaceSource>() // 稀疏浅坑独立CPU四槽源。
            .init_instance_resource::<terrain_derived::DynamicTerrainHeightCache>()
            .init_instance_resource::<terrain_derived::DynamicTerrainHeightStatus>()
            .init_instance_resource::<terrain_derived::DynamicTerrainDerivationState>()
            .init_instance_resource::<terrain_derived::DynamicTerrainCommitState>() // 地形、水域和植被只在同代候选全部完成后同帧切换。
            .init_instance_resource::<sparse_runtime::SparseClipmapRuntime>() // 新场景的清单句柄与后台网格单飞状态。
            .init_instance_resource::<feathrise_scene_content::SparseHeightSessionCrater>() // 独立于作者草稿的技能地形覆盖。
            .add_message::<feathrise_scene_content::SparseHeightCraterRequest>() // 玩家与怪物共用一次施放一次地形请求。
            .init_instance_resource::<layered_collision::LayeredCollisionRuntime>() // 新模式独立单飞状态，不复用角色身份双层需求。
            .init_instance_resource::<feathrise_scene_core::residency::TerrainCollisionRegion>() // 范围外实体和地面查询只读取当前已采用覆盖。
            .init_instance_resource::<feathrise_scene_content::SceneCollisionLodStatus>() // 已保存和已采用配置分别反馈给F12。
            .init_instance_resource::<feathrise_scene_content::SceneTerrainDisplayLodStatus>() // 显示LOD独立发布保存、构建、预热和采用状态。
            .add_resident_systems(
                PreUpdate,
                (
                    layered_collision::reap_layered_collision,
                    layered_collision::update_terrain_activity,
                )
                    .chain(),
            ) // 玩法驱动前先标记范围外暂停；切换场景后仍归还旧任务槽。
            .add_resident_systems(
                PhysicsSchedule,
                (
                    layered_collision::update_terrain_activity,
                    layered_collision::freeze_terrain_activity,
                )
                    .chain()
                    .after(TnuaSystems)
                    .before(PhysicsStepSystems::First),
            ) // 固定步再次核对传送与无地面，最后清零驱动。
            .init_instance_resource::<clipmap::SparseHeightPageRuntime>() // 新图近场 GPU 高度页和地表包装的单飞状态。
            .add_resident_systems(PreUpdate, clipmap::reap_retired_sparse_height_pages) // 菜单和其他场景也回收旧页任务，不保留旧图内存。
            .init_instance_resource::<clipmap::AuthoritativeHeightRuntime>() // 摆放与Mesh植被按需共享局部Mip 0作者页。
            .init_instance_resource::<clipmap::DynamicClipmapGroundSurface>() // 共享量化页的笔刷地表快照。
            .init_instance_resource::<vegetation::DynamicVegetationState>()
            .init_instance_resource::<DynamicBladeDraft>() // 草稿图片与定义跨F12关闭共同保留。
            .init_resource::<DynamicModelVegetationDraft>() // 场景根模型画布独立于地形检查器Owner。
            .init_instance_resource::<DynamicVegetationStatus>()
            .init_instance_resource::<water::DynamicWaterState>()
            .init_instance_resource::<water::ClipmapWaterRuntime>()
            .init_instance_resource::<DynamicClipmapWaterDraft>()
            .init_instance_resource::<DynamicWaterStatus>()
            .add_message::<DynamicWaterRebuildRequest>() // F12保存先建立事务边界，再发布JSON/R8事件。
            .add_message::<TerrainWaterRebuildRequest>() // 独立画布只推进本批自己的正式水代次。
            .init_instance_resource::<mesh_profiles::DynamicMeshProfileRequests>()
            .init_instance_resource::<mesh_profiles::DynamicMeshProfileState>()
            .init_instance_resource::<mesh_profiles::DynamicMeshProfileStatus>()
            .init_resident_resource::<mesh_profiles::DynamicMeshProfileCache>() // 所有地形共享正式原型和唯一加载槽。
            .add_resident_systems(
                OnEnter(ScenePhase::Active),
                (
                    vegetation::reset_dynamic_blade_interaction, // 新场景先清空固定八槽，玩家出现后再从其世界位置建立路径。
                    systems::begin_dynamic_scene,
                    boundary::begin_dynamic_boundaries,
                    surface::begin_dynamic_surface,
                    mesh_profiles::begin_dynamic_mesh_profiles,
                    vegetation::begin_dynamic_vegetation,
                    water::begin_dynamic_water,
                    water::begin_clipmap_water,
                )
                    .chain(),
            )
            .add_resident_systems(
                Update,
                systems::sync_dynamic_celestial_lighting
                    .before(WorldTimeSystemSet::ApplyLighting)
                    .run_if(in_state(ScenePhase::Active)), // 全局、场景或 F12 草稿变化同帧进入最终光照。
            )
            .add_resident_systems(
                Update,
                (
                    systems::observe_dynamic_scene_assets,
                    systems::return_to_main_menu_if_dynamic_scene_missing, // 目录已采用但目标消失时先终止整图加载并请求主菜单。
                    (
                        (
                            systems::prepare_dynamic_scene_terrain, // 总清单仍存在时才建立正式地形根。
                            session_crater::prepare_crater_surface, // AssetServer有界读取作者raw，匹配预设后只发布完整快照。
                            session_crater::receive_crater_requests, // 核对场景身份与真实地面后发布会话覆盖。
                            session_crater::apply_pending_craters, // 根只消费旧单地形队列；独立对象队列由各自实例调度推进。
                            sparse_runtime::drive_sparse_clipmap, // 新图清单验证后后台构建固定环带网格。
                            terrain_topology::collect_terrain_topology
                                .in_set(TerrainTopologySystems::Collect),
                            layered_collision::drive_layered_collision
                                .in_set(TerrainTopologySystems::Collision)
                                .in_set(LayeredCollisionDiagnosticsSystemSet), // 完整采用后可读取诊断；实体由下次PreUpdate或物理前活动门安全恢复。
                            terrain_topology::publish_terrain_topology
                                .in_set(TerrainTopologySystems::Publish),
                            clipmap::drive_sparse_height_pages, // 当前视图按层编码有界GPU高度页。
                        )
                            .chain(), // 新图根、显示与碰撞按同一场景身份依次推进，也压缩顶层系统元组。
                        (
                            systems::mark_dynamic_scene_terrain_primitive, // GLB子实体出现后登记F12草稿实时接管身份。
                            terrain_derived::observe_dynamic_terrain_source, // 只追踪当前根内唯一GLB地形及相关Mesh热更。
                            terrain_derived::capture_dynamic_terrain_source, // 主线程每帧按固定字节预算冻结位置和索引。
                            terrain_derived::poll_dynamic_terrain_derivation, // 后台完整结果匹配身份后原子登记高度Image与Trimesh。
                        )
                            .chain()
                            .run_if(systems::dynamic_scene_uses_glb_terrain), // 新稀疏图绝不进入整图GLB派生。
                        boundary::observe_dynamic_boundary_assets,
                        boundary::prepare_dynamic_boundaries,
                        (
                            surface::observe_dynamic_surface_assets,
                            surface::prepare_dynamic_surface_definition,
                            surface::install_dynamic_surface,
                            clipmap::sync_sparse_clipmap_surface, // 内层地表预热后同帧包装当前高度页。
                            clipmap::drive_sparse_display_gpu_transaction, // Mesh、页集与当前地表候选连续两帧预热后原子提交。
                            sparse_runtime::follow_sparse_clipmap, // 新Mesh和高度页采用后再移动到共享观察中心。
                            terrain_topology::sync_topology_materials,
                            terrain_topology::sync_volume_surface, // 只复用本帧已采用地表，近远体积面和旧雕刻GLB共同接管。
                        )
                            .chain(),
                        mesh_profiles::drive_dynamic_mesh_profiles,
                        (
                            vegetation::observe_dynamic_vegetation_assets,
                            vegetation::prepare_dynamic_vegetation_definition,
                            vegetation::start_dynamic_vegetation_build,
                            vegetation::install_dynamic_vegetation,
                            vegetation::sync_dynamic_blade_clipmap, // 草窗与本帧地形使用同一完整页和显示中心。
                            clipmap::drive_authoritative_height_pages, // 最多一个IO准备与相机无关的局部Mip 0页。
                            clipmap::sync_clipmap_ground_surface
                                .in_set(ClipmapGroundPublishSet)
                                .after(TerrainTopologySystems::Publish), // 同帧联合拓扑先采用，笔刷不能冻结上一个洞口/网格版本。
                            vegetation::drive_dynamic_blade_clearance, // 完整后台遮罩一次替换当前Blade四窗密度句柄。
                            vegetation::drive_dynamic_mesh_residency, // 当前修订采用后按唯一玩法/F12观察相机流式常驻完整Mesh块。
                        )
                            .chain(), // 植被内部保持观察、冻结、构建与隐藏候选顺序，同时避免超长系统元组。
                        (
                            water::observe_dynamic_water_assets,
                            water::prepare_dynamic_water,
                            water::install_dynamic_water,
                            water::drive_clipmap_water,
                            water::follow_clipmap_water, // 视图移动只同步最多八个变换和高度绑定。
                        )
                            .chain(), // GLB与Clipmap共享内容事务，按地形能力进入互斥后端。
                        (
                            terrain_derived::prepare_dynamic_terrain_commit, // 四域候选全部完整后才武装一次性提交令牌。
                            terrain_derived::commit_dynamic_terrain_derivation, // 同一Update先提交正式Mesh、Trimesh与高度缓存。
                            vegetation::commit_dynamic_vegetation_for_terrain, // 同令牌切换同代隐藏植被根。
                            water::commit_dynamic_water_for_terrain, // 同令牌切换同代水域批次。
                            terrain_derived::finish_dynamic_terrain_commit, // 防止令牌跨帧或被重复消费。
                        )
                            .chain()
                            .run_if(systems::dynamic_scene_uses_glb_terrain), // 旧联合提交只处理GLB完成值。
                        systems::finalize_dynamic_scene_ready,
                    )
                        .chain() // 保持现有地形、可选内容和最终启动门严格顺序。
                        .run_if(systems::dynamic_scene_target_is_registered), // 缺失目录帧不让任一子系统重复失败或创建半套实体。
                    systems::publish_dynamic_scene_initial_failure, // 首载任一地形失败都结束70%的SceneInit等待；热更失败不遮挡玩法。
                )
                    .chain()
                    .before(crate::SceneApplicationSet::PublishSource) // 新地形、边界与水面先完整提交，角色才核对同帧环境身份。
                    .run_if(in_state(ScenePhase::Active)),
            )
            .add_resident_systems(
                PostUpdate,
                (
                    vegetation::follow_dynamic_blade_windows, // 相机只负责四级窗口常驻与LOD观察中心。
                    vegetation::update_dynamic_blade_interaction, // 真实玩家世界X/Z驱动草、麦和杂粮的压弯与回弹。
                )
                    .before(TransformSystems::Propagate)
                    .run_if(in_state(ScenePhase::Active)),
            )
            .add_resident_systems(
                PostUpdate,
                (
                    mesh_profiles::collect_dynamic_mesh_profiles, // 返回菜单仍结清无人持有的真实加载。
                    mesh_profiles::update_dynamic_mesh_profile_wind.run_if(in_state(ScenePhase::Active)),
                ).chain().before(TransformSystems::Propagate),
            )
            .add_resident_systems(
                OnExit(ScenePhase::Active),
                (
                    vegetation::reset_dynamic_blade_interaction, // 离场先清GPU源与CPU轨迹，避免下一场景短暂读取旧压草状态。
                    vegetation::clear_dynamic_vegetation,
                    water::clear_clipmap_water,
                    water::clear_dynamic_water,
                    boundary::clear_dynamic_boundaries,
                    mesh_profiles::clear_dynamic_mesh_profiles,
                    surface::clear_dynamic_surface,
                    terrain_derived::clear_dynamic_terrain_derivation,
                    terrain_topology::clear_terrain_topology, // 回收已采用/候选体积根及三维查询，不留下离图物理和光追来源。
                    clipmap::clear_sparse_height_pages,
                    clipmap::clear_authoritative_height_pages, // 释放局部Mip 0缓存并拒绝旧图迟到请求。
                    clipmap::clear_clipmap_ground_surface, // 同名场景重新进入也不能使用旧笔刷高度。
                    session_crater::clear_session_craters, // 切换场景或重进绝不沿用上次技能破坏。
                    session_crater::clear_crater_surface,  // CPU源强句柄与未处理消息同图一起释放。
                    sparse_runtime::clear_sparse_clipmap,
                    layered_collision::clear_layered_collision, // 退出新模式释放本域覆盖，不阻塞在途CPU回收。
                    layered_collision::update_terrain_activity, // 离场同帧归还本域拥有的轴锁。
                    systems::clear_dynamic_scene,
                )
                    .chain(),
            );
    }
}
