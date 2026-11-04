//! 世界UI从普通透明场景独立分流、按原深度绘制；名牌/血条在输出分辨率合成，不修改水、粒子或PBR管线。
mod components; // 同帧独立透明颜色输出。
mod depth; // 将内部抖动深度映射至输出分辨率。
#[cfg(test)] // 离屏GPU回归单独显式执行，不启动游戏。
mod depth_gpu_tests; // 验证超性能尺寸下真实深度像素。
mod draw; // 原主视图深度只读绘制及普通颜色回退。
mod pipeline; // 实际独立pass与普通回退分别选择投影管线。
mod queue; // 精确标记提取、retained 项归还与 batching 前分流。
#[cfg(test)] // 真实Bevy渲染链路测试不启动游戏窗口。
mod render_tests; // 验证管线变体、回退及实际GPU绘制。
mod request; // 高清世界标签不再依赖SDK帧生成开关。
mod resources; // 固定数量、显存预算和 Prepare 完整性门。
mod temporal_guides; // 清理旧的字形引导暂停标记，头顶文字始终保留普通时间重建。
#[cfg(test)] // 不启动 GPU 的 retained 队列生命周期回归。
mod tests;
mod types; // 复用 Bevy DrawMaterial 的独立 sorted phase。 // 验证关闭恢复、旧实体移除和相机销毁不会丢项或复活过期项。

use bevy::{
    core_pipeline::{Core3dSystems, core_3d::main_transparent_pass_3d, schedule::Core3d},
    pbr::MeshPipeline,
    prelude::*,
    render::{
        ExtractSchedule, Render, RenderApp, RenderSystems,
        render_phase::{AddRenderCommand, SortedRenderPhasePlugin, sort_phase_system},
    },
}; // 公开 Bevy phase/batching 插件，不维护 PBR 私有结构副本。
use feathrise_billboard_text::BillboardTextSystems; // 暂停标记必须早于既有时间引导可见性更新。
use feathrise_settings::GraphicsRuntimeLeaseSystems; // 同帧读取最终 MSAA/HDR 等画质租约事实。
pub use components::WorldUiLayer; // UI 合成只读取本帧输出与完成事实。
pub use resources::WorldUiFrameGate; // Prepare 必须确认可见世界 UI 完整剥离。
use types::WorldUi3d; // 独立 phase 类型使 GPU 实例和间接参数缓冲正确隔离。

pub(crate) fn install(app: &mut App) {
    bevy::asset::load_internal_asset!(
        app,
        depth::SHADER,
        "depth.wgsl",
        bevy::shader::Shader::from_wgsl
    ); // 编译期内嵌深度映射着色器。
    // 由 FSR 主插件注册，未编译该候选时完全不改变普通渲染。
    app.add_plugins(SortedRenderPhasePlugin::<WorldUi3d, MeshPipeline>::new(
        default(),
    )); // 复用 CPU/GPU batching 及缓冲收集全部公开系统。
    app.add_systems(
        PostUpdate,
        temporal_guides::sync_suppression
            .after(GraphicsRuntimeLeaseSystems::ApplyActive)
            .after(crate::FrameGenerationUiRequestSystems::Publish)
            .before(BillboardTextSystems::SyncTemporalGuides),
    ); // 主世界保留正常引导，实际暂停由RenderWorld按本帧分流结果决定。
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    }; // 无 GPU 的纯逻辑测试安全跳过。
    render_app
        .add_systems(bevy::render::RenderStartup, depth::prepare) // 启动期创建固定管线。
        .add_systems(
            Render,
            depth::configure
                .in_set(RenderSystems::PrepareResources)
                .before(bevy::core_pipeline::core_3d::prepare_core_3d_depth_textures),
        ) // 在主深度分配前补齐采样用途。
        .init_resource::<resources::WorldUiEntities>() // 最多 4096 项的精确标记清单。
        .init_resource::<request::WorldUiRequest>() // 与SDK捕获请求分开保存相机资格。
        .add_systems(ExtractSchedule, request::extract) // DLSS仅超分时仍请求高清名牌。
        .init_resource::<bevy::render::render_phase::DrawFunctions<WorldUi3d>>() // SortedRenderPhasePlugin只安装批处理，必须先拥有绘制函数表再注册DrawMaterial。
        .init_resource::<resources::WorldUiFrameGate>() // 固定大小的当前帧输入完整性事实。
        .init_resource::<resources::WorldUiPipelines>() // 有界派生投影管线。
        .add_render_command::<WorldUi3d, pipeline::DrawWorldUi>() // 普通回退使用原管线，独立层使用无抖动变体。
        .add_systems(
            Render,
            temporal_guides::publish_gate.in_set(RenderSystems::PrepareResources),
        ) // Queue完成后发布实际引导资格，早于所有渲染pass。
        .add_systems(ExtractSchedule, queue::extract_entities) // 只从主世界读取显式 UI 标记，不克隆场景。
        .add_systems(
            Render,
            queue::restore_retained_items
                .in_set(RenderSystems::Queue)
                .before(RenderSystems::QueueMeshes),
        ) // Bevy retained 材质队列先获得上一帧被借走的项。
        .add_systems(
            Render,
            queue::split_world_ui
                .in_set(RenderSystems::Queue)
                .after(RenderSystems::QueueSweep),
        ) // 所有材质队列清理完成后、排序/batching 前完整分流。
        .add_systems(
            Render,
            sort_phase_system::<WorldUi3d>.in_set(RenderSystems::PhaseSort),
        ) // 保留现有距离和 depth_bias 的背向前排序。
        .add_systems(
            Core3d,
            depth::draw
                .in_set(Core3dSystems::MainPass)
                .after(main_transparent_pass_3d)
                .before(draw::draw_world_ui), // 主深度完成后映射，之后只读World绘制名牌。
        ) // 可写资源与DrawMaterial只读世界分属两个系统。
        .add_systems(
            Core3d,
            draw::draw_world_ui
                .in_set(Core3dSystems::MainPass)
                .after(main_transparent_pass_3d),
        ); // 主深度已就绪且时间超分未开始，独立颜色层使用输出分辨率无抖动投影并保持原遮挡。
} // world_ui 安装结束。
