//! GPU草地每候选分类、工作组紧凑化与间接绘制参数生成功能域。

mod clipmap; // 初始化分页高度的固定兼容绑定。
mod clipmap_resources; // 固定参数与资源类型。
#[cfg(any(test, feature = "diagnostics"))] // 普通运行时不开放逐点 CPU 镜像。
pub(crate) mod clipmap_validation; // CPU oracle 接口仅供无窗口回归，不在运行时调用。
mod culling; // 从唯一玩法相机提取并上传六平面GPU视锥。
mod helpers; // 容量、GPU瓦片派发和八十字节状态编码纯函数。
mod node; // 主相机绘制前编码分类与收尾计算通道的根RenderGraph系统。
mod pipeline; // 创建统一计算绑定组布局及瓦片、派发、分类、收尾四个异步计算管线。
#[cfg(feature = "diagnostics")]
pub(crate) mod pipeline_diagnostic; // 四个精确管线ID的只读等待原因，不扫描全局缓存。
mod prepare; // 跨帧复用输出缓冲并准备当前帧计算作业。
#[cfg(feature = "diagnostics")] // 公共核心默认不编译异步回读节点及其映射状态机。
mod readback; // 三缓冲异步回读80字节状态，向F3提供GPU瓦片、扫描与真实存活数。

use bevy::app::SubApp; // 在既有RenderApp中安装计算资源、系统与RenderGraph节点。
use bevy::core_pipeline::schedule::camera_driver; // 诊断拷贝录入后才允许任何玩法相机消费间接参数。
use bevy::prelude::{IntoScheduleConfigs, resource_exists}; // 为准备系统指定RenderSystems阶段。
use bevy::render::renderer::RenderGraph; // Bevy 0.19根RenderGraph调度标签保证系统每帧只运行一次。
use bevy::render::{Render, RenderStartup, RenderSystems}; // 计算管线、相机视锥和逐帧作业准备所需渲染调度标签。

use crate::blade::VegetationRenderGraphSet; // 跨Mip、分类与诊断功能域固定相机前GPU命令顺序。
use crate::resources::{GpuBladeComputeJobs, GpuBladeComputePipeline}; // 节点只消费PrepareBindGroups阶段构建好的短作业列表。
use culling::{init_gpu_grass_view_culling, prepare_gpu_grass_view_culling}; // 全部内容窗口共享唯一玩法视图的六平面uniform。
use node::run_gpu_blade_compute; // 根图Schedule中的无状态计算系统。
use pipeline::{init_gpu_grass_compute_pipeline, needs_gpu_grass_compute_pipeline}; // 首次提取到真实草窗才排队计算管线。
use prepare::prepare_gpu_grass_compute_jobs; // 每帧复用缓冲、刷新间接模板并创建绑定组。
#[cfg(feature = "diagnostics")] // 只有诊断插件需要把回读节点接入根渲染图。
use readback::install_gpu_grass_readback; // 计算完成后只拷贝每圈80字节诊断状态。

/// 把GPU草地计算链路安装到渲染子应用。
pub(super) fn install_gpu_grass_compute(render_app: &mut SubApp) {
    render_app
        .init_resource::<GpuBladeComputeJobs>() // 空场景保持零作业，RenderGraph节点可以无条件运行。
        .add_systems(RenderStartup, clipmap::init_clipmap_fallback) // 初始化一次数组与整数页表兼容绑定。
        .add_systems(RenderStartup, init_gpu_grass_view_culling) // 启动时创建一次共享304字节视锥、投影LOD、常驻回退与画质uniform。
        .add_systems(
            Render,
            (
                init_gpu_grass_compute_pipeline
                    .run_if(needs_gpu_grass_compute_pipeline)
                    .in_set(RenderSystems::PrepareResources)
                    .before(prepare_gpu_grass_compute_jobs), // 真实草窗首次出现才异步编译；依赖边保证Commands在作业准备前采用。
                prepare_gpu_grass_view_culling.in_set(RenderSystems::PrepareResources), // 相机提取完成后先上传本帧六平面。
                prepare_gpu_grass_compute_jobs
                    .run_if(resource_exists::<GpuBladeComputePipeline>)
                    .in_set(RenderSystems::PrepareBindGroups), // 空工程没有草地管线和作业，不能为首个地形排队无用的大型Shader编译。
            ),
        )
        .configure_sets(
            RenderGraph,
            (
                VegetationRenderGraphSet::CandidateMip,
                VegetationRenderGraphSet::BladeCompute,
                VegetationRenderGraphSet::DiagnosticReadback,
            )
                .chain()
                .before(camera_driver), // 三个相机前阶段即使某项为空也保持稳定拓扑和多相机单次执行。
        )
        .add_systems(
            RenderGraph,
            run_gpu_blade_compute
                .run_if(resource_exists::<GpuBladeComputePipeline>)
                .in_set(VegetationRenderGraphSet::BladeCompute),
        ); // 完成Bevy 0.19根RenderGraph调度注册。
}

/// 安装可选的 Compute 状态异步回读链路。
#[cfg(feature = "diagnostics")] // 公共核心默认不创建回读缓冲或映射任务。
pub(super) fn install_gpu_grass_compute_diagnostics(render_app: &mut SubApp) {
    install_gpu_grass_readback(render_app); // 安装Compute→Readback→CameraDriver强依赖并在诊断拷贝后启动所有相机。
}

#[cfg(test)] // 无窗口双后端验证只进入测试构建。
mod clipmap_tests; // 生产 WGSL 与真实拓扑 CPU 镜像的闭环回归。

#[cfg(test)]
mod lod_tests; // 直接调用生产选择函数，验证任意档数的互斥与最粗档托底。

#[cfg(test)]
mod volume_pose_tests; // 从正式绘制Shader读回草叶姿态，覆盖二维与三维接缝处的风向和踩踏。

#[cfg(test)]
mod volume_density_tests; // 两种地形运行完整生产Compute流程，逐株对比密度、LOD与外观。
