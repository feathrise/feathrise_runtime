//! 正式大地图共用的Blender原生水材质、GPU背景缓波与玩家触水波纹功能域。
//! 插件入口负责模块装配，CPU维护接触事件与固定近景水滴池，水面逐顶点位移、乘色与法线统一在GPU执行。

use feathrise_scene_core::residency::SceneResidencyAppExt;

// Scene消息epoch必须早于整网格水工作推进并显式落地observer命令。
use crate::shared::world_asset_lifecycle::{
    WorldAssetLifecycleSystems, install_world_asset_lifecycle,
};
use bevy::pbr::MaterialPlugin; // 为具体GPU水面扩展材质注册提取、准备和全部PBR管线。
use bevy::prelude::*; // App、Plugin、PostUpdate 与系统运行条件使用 Bevy 基础类型。
use bevy::transform::TransformSystems; // 水面动画必须在全局变换传播后读取玩家坐标。

mod components; // ECS组件保存每张GPU水面的岸线与轻量玩家接触状态。
mod constants; // 背景波、玩家波纹、深度和容量等固定参数。
mod footprint; // 水面两岸横截面提取与局部点内判断。
mod gpu_upload; // runtime与F12共享分帧Assets登记及RenderApp准备回执。
mod material; // 保留StandardMaterial PBR/Alpha契约，并扩展顶点波形与样条逐像素流动。
mod mesh_overrides; // placement整网格赋水复用现有GPU材质，支持原地恢复与热重施。
mod painted; // R8覆盖离线建网格与编辑器、动态地图共用的水面生成入口。
mod retirement;
pub use retirement::retire_water_cpu;
pub(crate) use retirement::retire_water_cpu_in_world;
#[cfg(test)]
pub(crate) use retirement::install as install_water_retirement;
mod rain_mask; // 真实GPU水面高度图阻止雨丝继续穿过湖泊和河流。
mod reflection; // 当前视图色深副本驱动透明水面的角色、建筑和植被反射。
mod ribbon; // 完整三维河流、急流与瀑布幕布纯构建器。
mod ripple; // 无 ECS 系统参数的纯波纹采样与玩家接触状态计算。
mod shaders; // 内置水面WGSL固定句柄和注册入口。
mod splash; // P3固定容量近景离水水滴，真实足迹与积水核心授权。
mod systems; // GLB水面初始化observer和逐帧固定材质参数同步系统。
mod types; // 玩家波源、参考采样和固定GPU ABI等无ECS绑定值类型。
mod underwater; // 当前相机按真实水域足迹显示水下色调与雾，出水归还天气和白平衡。

use systems::update_dynamic_water_surfaces; // 插件只依赖GPU水面初始化和固定参数同步两个ECS系统入口。

pub(crate) use components::{DynamicWaterCpuInteraction, DynamicWaterSurface};
pub use components::{DynamicWaterMeshOverrideNumericPatch, DynamicWaterMeshOverrides}; // placement runtime与F12共享整批结构快照和一次性O(1)数值补丁。 // Clipmap静水安装同一八槽玩家互动状态，但保持crate内部实现边界。

pub use gpu_upload::{
    MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME, MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME,
    WaterGpuUploadQueue, WaterGpuUploadReceipts, WaterGpuUploadTicket,
}; // 两个采用方共享同一每帧上传额度和不可伪造的RenderApp回执。
pub use material::DynamicWaterMaterial; // F12编辑器登记实际动态水材质资产时使用稳定公开类型。
pub use mesh_overrides::DynamicWaterMeshOverrideStatus; // placement候选与F12只读水体全量接管结果，不暴露恢复或写入实现。
pub(crate) use mesh_overrides::DynamicWaterMeshOverrideSystems; // placement联合结算显式等待每帧单根处理集合。
use mesh_overrides::{
    apply_dynamic_water_mesh_numeric_patch, cancel_removed_dynamic_water_mesh_world_asset,
    cleanup_removed_mesh_override_assets, initialize_dynamic_water_mesh_overrides,
    invalidate_discarded_dynamic_water_mesh_world_asset, invalidate_dynamic_water_mesh_asset_epoch,
    process_pending_dynamic_water_mesh_override, queue_inserted_dynamic_water_mesh_overrides,
    restore_removed_dynamic_water_mesh_overrides, retire_removed_mesh_override_materials,
}; // 整网格赋水由生命周期observer定向收集请求，独立系统每帧最多处理一个根。
pub use painted::{
    BuiltWaterSurface, PaintedWaterMeshData, PaintedWaterPurpose, PaintedWaterSurface,
    PaintedWaterSurfaceSpawn, PaintedWaterTerrainInput, StagedWaterSurface, WaterSurfaceDescriptor,
    WaterSurfaceKind, WaterSurfaceMeshData, advance_water_surface_material,
    build_painted_water_mesh, build_painted_water_mesh_with_depth, create_water_surface_material,
    spawn_painted_water_surface, spawn_staged_water_surface, spawn_staged_water_warmup,
    spawn_water_surface, stage_water_surface_assets, stage_water_surface_chunk, chunk_water_surfaces,
}; // 数据驱动运行时与F12预览共用同一建网格、描述和GPU水材质安装入口。
pub use ribbon::{
    MAX_MAP_SPLINE_WATER_TRIANGLES, MAX_RIVER_RIBBON_SECTIONS, MAX_WATERFALL_RIBBON_SECTIONS,
    build_river_water_surface, build_waterfall_water_surface, validate_spline_water_build_budget,
};
pub(crate) use types::DynamicWaterUniform; // Clipmap雨水高度材质复用相同640字节GPU ABI。 // 动态地图single-flight通过公开纯函数构建固定预算三维水网络。

/// 正式大地图水面插件：注册GLB水面GPU材质、初始化observer和固定波源参数同步系统。
pub(crate) struct LargeMapWaterPlugin; // 地图聚合入口全局注册一次，支持所有已登记水面白名单的地图。

impl Plugin for LargeMapWaterPlugin {
    // 实现独立水面功能域的 Bevy 插件接口。
    fn build(&self, app: &mut App) {
        // Shader先于材质插件登记，空场景查询不会产生水面参数更新工作。
        shaders::register_dynamic_water_shaders(app); // 主通道、预通道与透明片元入口共享同一GPU水面参数。
        reflection::install(app); // 只为当前玩法或F12透视视图准备一次场景颜色副本。
        rain_mask::install(app); // 创建固定局部高度图与真实水面代理缓存。
        underwater::install(app);
        app.add_plugins(MaterialPlugin::<DynamicWaterMaterial>::default()) // 安装主渲染、透明片元、阴影、预通道和延迟顶点资产接线。
            .init_resident_resource::<splash::SplashWaterTarget>() // 既有水遍历与近景水滴共享唯一脚下水体。
            .add_resident_systems(Startup, splash::initialize_rain_splashes) // 启动阶段构建一次共享几何和48槽固定池。
            .init_resident_resource::<mesh_overrides::DynamicWaterMeshOverrideQueue>() // 去重FIFO只存根实体，容量随内容增长，稳定帧不扫描placement。
            .init_resident_resource::<mesh_overrides::DynamicWaterMeshOverrideWorkState>() // FIFO之外最多一个活动根，所有重计算都以固定预算游标推进。
            .init_resource::<mesh_overrides::RetiredMeshWaterMaterials>() // restore/Remove只登记旧材质，统一以四份/帧从Assets退役。
            .add_observer(initialize_dynamic_water_mesh_overrides) // placement World ready后只入队；普通空覆盖直接Ready。
            .add_observer(invalidate_discarded_dynamic_water_mesh_world_asset) // 资产根替换当下撤销旧Ready和旧实例请求。
            .add_observer(cancel_removed_dynamic_water_mesh_world_asset) // 根真实移除时释放换代阻断与FIFO席位。
            .add_observer(queue_inserted_dynamic_water_mesh_overrides) // 覆盖整组件Add/替换只唤醒对应已ready根。
            .add_observer(apply_dynamic_water_mesh_numeric_patch) // 拖动每档只改稳定索引的一项f32并进入同一重建链。
            .add_observer(invalidate_dynamic_water_mesh_asset_epoch) // 原地GLB热更按资产epoch定向撤销同InstanceId旧Ready。
            .add_observer(restore_removed_dynamic_water_mesh_overrides) // F12移除覆盖组件也完整恢复原句柄与位姿。
            .add_observer(cleanup_removed_mesh_override_assets) // 热改、撤销和despawn均精确回收独占Mesh/水材质。
            .add_resident_systems(
                Update,
                process_pending_dynamic_water_mesh_override
                    .after(WorldAssetLifecycleSystems::InvalidateRoots)
                    .in_set(DynamicWaterMeshOverrideSystems::ProcessPending), // 全局每帧最多承担一个根的现有硬预算。
            )
            .add_resident_systems(
                Update,
                retire_removed_mesh_override_materials
                    .after(DynamicWaterMeshOverrideSystems::ProcessPending), // 提交、显式撤销和despawn合计最多删除四份旧材质。
            )
            .add_resident_systems(
                // 在世界变换传播后读取同帧玩家坐标，只上传时间与八槽统一长期波源。
                PostUpdate, // 渲染提取前推进本帧固定GPU水面参数。
                (underwater::prepare_view,
                    underwater::sample_column.run_if(bevy::ecs::schedule::common_conditions::resource_exists::<avian3d::collider_tree::ColliderTrees>),
                    update_dynamic_water_surfaces, underwater::apply_view, splash::update_rain_splashes)
                    .chain()
                    .after(TransformSystems::Propagate)
                    .after(feathrise_environment::terrain_surface::SurfaceWeatherUpload)
                    .after(feathrise_environment::rain_shelter::RainShelterUpload)
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                    .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility), // 同帧天气与变换已发布，水花在可见性传播和裁剪前写入世界位置。
            ); // 完成正式大地图GPU水面系统注册。
        gpu_upload::install_water_gpu_upload_monitor(app); // 材质插件登记后再核对真实PreparedMaterial。
        install_world_asset_lifecycle(app); // 首个上层插件安装唯一消息游标；重复调用保持幂等。
    } // 结束插件构建。
} // 结束LargeMapWaterPlugin实现。

#[cfg(test)] // 仅给同级测试模块暴露水面ECS组件。
use components::WaterJumpCycle; // 测试构造最小动态水面运行时状态。
#[cfg(test)] // 纯函数测试仅在测试构建启用，不进入游戏运行时代码。
use constants::*; // 测试复用运行时完全相同的水波参数。
#[cfg(test)] // 仅给同级测试模块暴露岸线纯函数。
use footprint::{
    PaintedWaterFootprint, WaterInteractionFootprint,
}; // 测试验证真实水面轮廓、统一互动策略和点内判断。
#[cfg(test)] // 仅给同级测试模块暴露波纹纯函数。
use ripple::{
    calculate_bank_fade,            // 测试验证两侧河岸的背景波与互动波都固定为零。
    calculate_player_ripple_height, // 测试直接读取玩家波纹几何高度。
    calculate_player_ripple_sample, // 测试同时验证主波峰高度和波谷颜色信号。
    calculate_wave_height,          // 测试锁定GPU Shader复现的背景缓波参考公式。
    update_player_ripples,          // 测试验证入水、移动、跳跃、传送与离水触发节奏。
}; // 完成水域纯逻辑测试接口导入。
#[cfg(test)] // 仅给同级测试模块暴露玩家波源值类型。
use types::PlayerWaterRipple; // 测试构造固定年龄和振幅的局部波源。

#[cfg(test)] // 纯函数测试仅在测试构建启用，不进入游戏运行时代码。
mod tests; // 波纹行为测试保持独立文件并通过父模块私有导入访问被测接口。

pub(crate) use components::WaterRenderChunk;
