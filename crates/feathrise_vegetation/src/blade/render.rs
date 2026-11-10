//! GPU 草地渲染功能域入口。
//! 核心与诊断由独立插件装配；地表材质属于环境模块而不再由叶片后端拥有。
//! 候选分类在相机绘制前只执行一次，AlphaMask 阶段仅消费紧凑存活实例并写入深度。

mod commands; // 注册 AlphaMask 与方向光阴影阶段使用的间接绘制命令。
mod interaction; // 管理全局风场、足迹和消费者综合色纹理绑定。
mod output_readiness; // Queue 只看输出分配，Draw 严格检查本帧有效性，消除恢复边沿额外断帧。
mod pipeline; // 创建并专用化复用 Bevy MeshPipeline 的草地管线。
mod prepass; // 按前向/Deferred路径补齐草叶深度、GBuffer与完整时间输入。
mod queue; // 把可见且已经准备好的草地窗口加入渲染阶段。
mod systems; // 发布渲染统计并在高度或植被密度输入未就绪时抑制草地。
use feathrise_scene_core::residency::SceneResidencyAppExt;

use bevy::pbr::{
    MeshPipelineSystems, Shadow, queue_material_meshes, queue_prepass_material_meshes,
    queue_shadows,
}; // 草地基础管线等待标准网格初始化，自定义阶段必须排在 Bevy 标准材质清理之后。
// Bevy 全局渲染诊断会在同一通道建立外层管线统计查询，草地诊断需要据此协调查询所有权。
#[cfg(feature = "diagnostics")]
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::{
    core_pipeline::{
        core_3d::AlphaMask3d,          // 植被抖动遮罩在无颜色混合的深度写入阶段执行。
        deferred::AlphaMask3dDeferred, // Solari视图写草叶GBuffer、深度和运动矢量。
        prepass::AlphaMask3dPrepass,   // DLSS RR 等时间重建从该阶段读取草叶深度和运动矢量。
    },
    prelude::*, // App、Plugin、Resource 与系统配置等 ECS 常用类型。
    render::{
        ExtractSchedule, // 在下一帧提取开始时把上一完整渲染帧指标回传主世界。
        Render,          // 注册准备绑定组与草地排队系统。
        RenderApp,       // 访问独立渲染子应用。
        RenderStartup,   // 渲染启动时建立草地基础管线资源。
        RenderSystems,   // 约束输入检查、绑定准备与排队的执行阶段。
        extract_component::{
            ExtractComponentPlugin, // 把草地组件的轻量副本提取到渲染世界。
            UniformComponentPlugin, // 把草窗参数写入共享动态 uniform。
        },
        extract_resource::ExtractResourcePlugin, // 提取全局风场、profile、资产配置和可选诊断配置。
        render_phase::AddRenderCommand,          // 向 AlphaMask 与 Shadow 阶段注册自定义绘制命令。
        render_resource::SpecializedMeshPipelines, // 缓存不同视图与原型拓扑组合的专用管线。
    },
};

use self::commands::{DrawGpuBlade, DrawGpuBladeShadow}; // 主画面和方向光阴影分别注册草地间接绘制命令。
use self::interaction::{
    init_gpu_grass_interaction_gpu, // 渲染启动时建立固定尺寸的全局交互 GPU 资源。
    prepare_gpu_grass_interaction_bind_group, // 每帧更新全局 uniform 并维护纹理绑定组。
};
use self::pipeline::{
    GpuBladePipeline,               // 主画面草叶专用化管线缓存模板。
    GpuBladeShadowPipeline,         // 近景草方向光深度管线缓存模板。
    init_gpu_grass_pipeline,        // 渲染启动时建立主画面管线布局。
    init_gpu_grass_shadow_pipeline, // Bevy 预通道就绪后建立只写深度的阴影模板。
};
use self::prepass::{
    DrawGpuBladeDeferred, DrawGpuBladePrepass, GpuBladePrepassPipeline,
    init_gpu_grass_prepass_pipeline, queue_gpu_grass_prepass,
}; // 只在目标相机启用运动矢量时复用紧凑实例，并按视图补齐前向或Deferred输入。
use self::queue::{queue_gpu_grass, queue_gpu_grass_shadows}; // 主画面排全部可见 LOD，阴影只排零号槽 LOD0。
use self::systems::{
    publish_gpu_grass_render_stats, // 把上一完整渲染帧计数发布回主世界资源。
    suppress_grass_until_gpu_inputs_are_ready, // 高度或植被密度纹理未上传前禁止错误草地闪现。
};
use crate::components::{
    GpuBladeClipmapHeight, // 提取已采用分页高度快照。
    GpuBladeDensityMap,    // 提取密度纹理强句柄供渲染世界就绪检查。
    GpuBladeHeightMap,     // 提取高度纹理强句柄供渲染世界就绪检查。
    GpuBladePatch,         // 提取每个动态草地窗口的候选与 LOD 参数。
    GpuBladeProfileBankId, // 为每个窗口选择兼容或数据驱动的四槽绑定组。
    GpuBladeView,          // 只让明确标记的玩法相机进入草地排队。
};
#[cfg(feature = "diagnostics")] // 诊断快照和查询兼容状态只由可选插件初始化和发布。
use crate::resources::{
    GpuBladeGpuReadbackStats,    // 主世界读取上一份完整 GPU 状态回读。
    GpuBladeGpuTimingStats,      // 主世界读取上一份完整 GPU 时间戳与统计样本。
    GpuBladeTimingCompatibility, // RenderApp 在启动设备查询前读取全局诊断独占标记。
};
use crate::resources::{
    GpuBladeInteraction,         // 全部窗口共享世界风场和固定容量玩家足迹源。
    GpuBladeProfileBanks,        // 任意数量数据驱动四槽批次只在目录变化时重建GPU绑定。
    GpuBladeProfiles,            // 固定四槽通用外观表每帧只提取一份小 uniform。
    GpuBladeRenderAssets,        // 消费者提供共享综合色图集，None 时使用 Bevy 白色回退。
    GpuBladeRenderFrameCounters, // RenderApp 各阶段共用的无锁本帧计数器。
    GpuBladeRenderStats,         // 主世界读取的上一完整 CPU 渲染流水线快照。
    GpuBladeStreamingStats,      // 消费者场景层可写入的预测与驻留生命周期快照。
    GpuBladeTimingQueries,       // 关闭 diagnostics 时仍提供不含查询集的空资源。
};

/// 通用 GPU 草地核心渲染插件。
pub struct GpuBladePlugin;

impl Plugin for GpuBladePlugin {
    fn build(&self, app: &mut App) {
        crate::shaders::register_core_shaders(app); // 核心 WGSL 随 crate 注册，不要求宿主项目提供 shader 文件。
        app.init_resource::<GpuBladeRenderStats>(); // 无 RenderApp 的逻辑测试也能读取稳定全零快照。
        app.init_resident_resource::<GpuBladeInteraction>(); // 公共默认提供微风和零足迹，消费者可按玩法覆盖。
        app.init_resident_resource::<GpuBladeProfiles>(); // 默认四槽使用同一中性 profile，内容层可显式替换。
        app.init_resident_resource::<GpuBladeProfileBanks>(); // 动态批次默认为空，目录可按场景实际预设原子替换。
        app.init_resident_resource::<GpuBladeRenderAssets>(); // 默认不要求外部纹理，消费者可运行时覆盖共享图集。
        app.init_resident_resource::<GpuBladeStreamingStats>(); // 无草地场景保持 Inactive 全零生命周期快照。
        app.add_plugins((
            ExtractComponentPlugin::<GpuBladePatch>::default(), // 每帧只提取每个动态窗口 208 字节参数。
            ExtractComponentPlugin::<GpuBladeProfileBankId>::default(), // 每个窗口额外提取四字节会话批次身份。
            UniformComponentPlugin::<GpuBladePatch>::default(), // 将草窗参数写入共享动态 uniform。
            ExtractComponentPlugin::<GpuBladeClipmapHeight>::default(), // 分页高度只提取句柄和固定参数。
            ExtractComponentPlugin::<GpuBladeHeightMap>::default(), // 高度纹理只提取强句柄，不复制像素。
            ExtractComponentPlugin::<GpuBladeDensityMap>::default(),
            ExtractComponentPlugin::<crate::components::GpuBladeSourceRevision>::default(), // 回读保留实际参与Compute的作者修订。
            ExtractComponentPlugin::<crate::components::GpuBladeVolumeSource>::default(), // 密度纹理只提取强句柄，不复制像素。
            ExtractComponentPlugin::<GpuBladeView>::default(), // 只让明确标记的玩法相机进入草地排队。
            ExtractResourcePlugin::<GpuBladeInteraction>::default(), // 每帧只提取固定176字节风场、连续相位与足迹。
            ExtractResourcePlugin::<GpuBladeProfiles>::default(), // 每帧提取固定四槽高度、颜色、图集和弯曲参数。
            ExtractResourcePlugin::<GpuBladeProfileBanks>::default(), // 目录只在内容变更时触发任意数量批次重建。
            ExtractResourcePlugin::<GpuBladeRenderAssets>::default(), // 只提取可选强句柄，纹理像素仍由 RenderAssets 持有。
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return; // 无渲染子应用的逻辑测试无需安装 GPU 管线。
        };
        render_app
            .add_render_command::<AlphaMask3d, DrawGpuBlade>() // 为主画面最终颜色阶段注册间接绘制命令；Deferred overlay只读既有深度。
            .add_render_command::<AlphaMask3dPrepass, DrawGpuBladePrepass>() // 普通前向时间重建写草叶深度和运动矢量。
            .add_render_command::<AlphaMask3dDeferred, DrawGpuBladeDeferred>() // Solari路径写完整草叶GBuffer，但不占用最终颜色统计与GPU计时。
            .add_render_command::<Shadow, DrawGpuBladeShadow>() // 为方向光级联注册近景草间接深度命令。
            .init_resource::<SpecializedMeshPipelines<GpuBladePipeline>>() // 缓存视图与网格拓扑组合的主画面管线。
            .init_resource::<SpecializedMeshPipelines<GpuBladePrepassPipeline>>() // 缓存单采样运动矢量与草原型拓扑组合。
            .init_resource::<SpecializedMeshPipelines<GpuBladeShadowPipeline>>() // 缓存方向光深度与 LOD0 拓扑管线。
            .init_resource::<GpuBladeRenderFrameCounters>() // RenderCommand 与准备系统共用无锁计数器。
            .init_resource::<GpuBladeTimingQueries>() // 无诊断时保留 None 查询集，绘制命令仍能执行。
            .add_systems(ExtractSchedule, publish_gpu_grass_render_stats) // 帧首发布并清零上一完整渲染帧指标。
            .add_systems(
                RenderStartup, // 渲染设备建立后初始化持久管线与交互缓冲。
                (
                    // Bevy 0.19 把 MeshPipeline 改为 RenderStartup 资源；显式排序避免首次提取时资源尚不存在。
                    init_gpu_grass_pipeline.after(MeshPipelineSystems),
                    init_gpu_grass_interaction_gpu,
                ),
            )
            .add_systems(
                Render, // 按 Bevy 0.19 渲染阶段准备并排队草窗。
                (
                    suppress_grass_until_gpu_inputs_are_ready
                        .in_set(RenderSystems::Prepare)
                        .after(RenderSystems::PrepareAssets)
                        .before(RenderSystems::PrepareResources), // 输入图片尚未上传时先清零渲染副本实例数。
                    prepare_gpu_grass_interaction_bind_group
                        .in_set(RenderSystems::PrepareBindGroups), // 上传全局 uniform 并维护消费者纹理绑定组。
                    init_gpu_grass_prepass_pipeline.in_set(RenderSystems::PrepareResources), // Bevy预通道布局就绪后只初始化一次时间重建模板。
                    init_gpu_grass_shadow_pipeline.in_set(RenderSystems::PrepareResources), // 预通道布局就绪后只初始化一次阴影模板。
                    queue_gpu_grass
                        .in_set(RenderSystems::QueueMeshes)
                        .after(queue_material_meshes), // 标准材质先清理旧分箱，随后补入没有材质组件的GPU草窗。
                    queue_gpu_grass_prepass
                        .in_set(RenderSystems::QueueMeshes)
                        .after(queue_prepass_material_meshes), // 标准预通道先处理专用化失效，避免把刚加入的草窗再次移除。
                    queue_gpu_grass_shadows
                        .in_set(RenderSystems::QueueMeshes)
                        .after(queue_shadows), // 官方阴影脏专用化先移除旧分箱，随后再补回近景方向光草影。
                ),
            );
        crate::gpu_compute::install_gpu_grass_compute(render_app); // 安装相机前分类压缩、状态收尾和作业准备链路。
    }
}

/// 可选 GPU 状态回读、时间戳和管线统计插件。
#[cfg(feature = "diagnostics")] // 未启用 feature 时不导出插件，也不创建任何回读缓冲。
pub struct GpuBladeDiagnosticsPlugin;

#[cfg(feature = "diagnostics")] // 插件实现与公开类型使用同一 Cargo feature 边界。
impl Plugin for GpuBladeDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<GpuBladePlugin>() {
            app.add_plugins(GpuBladePlugin); // 独立添加诊断插件时自动补齐核心管线。
        }
        app.init_resource::<GpuBladeGpuReadbackStats>(); // 首份异步 GPU 回读前提供稳定全零快照。
        app.init_resource::<GpuBladeGpuTimingStats>(); // 查询不支持或尚未回读时保留明确能力状态。
        app.init_resource::<crate::diagnostics::GpuBladeReadbackConfig>(); // 默认每四帧采样，严格烟雾可改为逐帧。
        app.add_plugins(ExtractResourcePlugin::<
            crate::diagnostics::GpuBladeReadbackConfig,
        >::default()); // 只向 RenderApp 提取固定非零采样间隔。
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return; // 无渲染子应用的逻辑测试不安装 GPU 回读和查询链路。
        };
        render_app.init_resource::<GpuBladeTimingCompatibility>(); // 默认允许草地管线统计，finish阶段再依据完整插件集合收紧。
        crate::gpu_compute::install_gpu_grass_compute_diagnostics(render_app); // 只拷贝紧凑状态，不阻塞主渲染通道。
        crate::gpu_timing::install_gpu_grass_timing(render_app); // 安装时间戳、管线统计和异步三缓冲回读。
    }

    fn finish(&self, app: &mut App) {
        // 所有插件完成 build 后才能可靠判断 Solari 使用的全局渲染诊断是否存在。
        let reserve_pipeline_statistics = app.is_plugin_added::<RenderDiagnosticsPlugin>(); // 全局诊断存在时由它独占每个通道唯一的统计查询。
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            // 无 RenderApp 的逻辑测试无需写入 GPU 兼容状态。
            return; // 保持无渲染测试可运行，不创建伪造的主世界 GPU 资源。
        }; // 完成可选 RenderApp 解包。
        render_app.insert_resource(GpuBladeTimingCompatibility {
            // 在 RenderStartup 创建设备查询集前发布最终兼容决策。
            reserve_pipeline_statistics_for_render_diagnostics: reserve_pipeline_statistics, // 保留草地时间戳，仅让出会嵌套冲突的统计查询。
        }); // 完成固定布尔兼容资源更新。
    } // 完成 GPU 草地与 Bevy 全局渲染诊断的插件级协调。
}
