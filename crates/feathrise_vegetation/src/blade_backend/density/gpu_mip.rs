//! 解析密度图集在GPU上生成完整Mip链时使用的稳定调度ABI。

use bevy::asset::{load_internal_asset, uuid_handle}; // 正式WGSL编译进crate并使用稳定内部句柄。
use bevy::core_pipeline::schedule::camera_driver; // 候选Compute与回读复制在相机驱动前录入根图。
use bevy::prelude::{App, Handle, IntoScheduleConfigs, Shader, UVec2}; // 插件安装、系统排序、内部Shader资产和uniform二维尺寸。
use bevy::render::RenderApp; // 只有真实渲染子应用存在时才安装GPU候选门。
use bevy::render::extract_resource::ExtractResourcePlugin; // 主世界每帧只提取当前一个轻量作业。
use bevy::render::render_resource::ShaderType;
use bevy::render::renderer::RenderGraph; // 完整Mip只作为一次全局根图作业执行。
use bevy::render::{ExtractSchedule, Render, RenderStartup, RenderSystems}; // 管线创建、资源准备和下一帧异步映射阶段。 // Rust尺寸uniform严格匹配WGSL两个vec2字段。

use super::gpu_compose::{
    GPU_COMPOSE_SHADER_HANDLE, pipeline::init_vegetation_gpu_compose_pipeline,
};
use super::resources::VegetationGpuMipRequest; // 提取资源仅包含当前票据、层和候选图片句柄。 // 正式基础级竞争在相邻Mip之前使用独立固定管线。
use crate::blade::VegetationRenderGraphSet; // 与草地Compute共享跨模块根图顺序。

mod node; // 根RenderGraph按相邻Mip顺序记录Compute与全链复制。
mod pipeline; // RenderStartup排队正式固定Compute管线。
pub(in crate::blade_backend::density) mod plan; // 独立WGPU测试与RenderApp复用同一对齐规划和去填充解码。
mod prepare; // RenderAsset就绪后创建每级非重叠视图和绑定组。
mod readback; // 下一帧发起map_async并向主世界发布版本化结果。
mod resources; // 渲染世界管线、准备作业和跨回调共享队列。

use node::run_vegetation_gpu_mip; // 安装一次无视图全局计算系统。
use pipeline::init_vegetation_gpu_mip_pipeline; // 设备建立后排队内部WGSL入口。
use prepare::prepare_vegetation_gpu_mip_job; // 只在新票据层出现且图片已上传时建立作业。
use readback::begin_maps_and_publish_vegetation_gpu_mips; // 绝不在运行时同步poll或等待GPU。
use resources::{VegetationGpuMipRenderState, VegetationGpuMipShared}; // 渲染世界持有当前作业和完成队列。

/// 单轴Compute工作组边长。
pub(super) const GPU_MIP_WORKGROUP_SIDE: u32 = 8; // 8×8兼顾小Mip浪费和桌面GPU占用率。

/// 正式GPU完整Mip生成着色器。
#[cfg(test)]
pub(super) const GPU_MIP_SHADER: &str = include_str!("shaders/vegetation_layout_mip.wgsl"); // 运行时接线与真实GPU测试必须使用同一份WGSL。

/// 正式GPU完整Mip内部Shader的稳定句柄。
pub(super) const GPU_MIP_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("74922881-84bc-423e-8e44-744fe4e94f5e"); // 独立UUID避免与宿主应用和叶片Compute资产冲突。

/// Rust与WGSL共享的每级尺寸uniform。
#[derive(Clone, Copy, Debug, ShaderType)]
pub(super) struct GpuMipUniform {
    pub(super) source_size: UVec2, // 当前只读父Mip的真实宽高。
    pub(super) target_size: UVec2, // 当前只写子Mip的真实宽高。
}

/// 相邻两个Mip之间的一次GPU调度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GpuMipDispatch {
    pub(super) source_level: u32, // 只读取上一层，禁止跨级平均导致CPU/GPU语义分叉。
    pub(super) target_level: u32, // 只写紧邻的下一层。
    pub(super) source_width: u32, // WGSL钳制单像素轴时需要真实源宽。
    pub(super) source_height: u32, // WGSL钳制单像素轴时需要真实源高。
    pub(super) target_width: u32, // 目标宽使用离线链相同的向下整除规则。
    pub(super) target_height: u32, // 目标高至少保持一个纹素。
}

impl GpuMipDispatch {
    /// 转换为Bevy按std140编码的正式uniform值。
    pub(super) fn uniform(self) -> GpuMipUniform {
        GpuMipUniform {
            source_size: UVec2::new(self.source_width, self.source_height), // 与WGSL `source_size`字段一致。
            target_size: UVec2::new(self.target_width, self.target_height), // 与WGSL `target_size`字段一致。
        }
    }

    /// 编码与WGSL `MipParams` 完全一致的四个小端 `u32`。
    #[cfg(test)]
    pub(super) fn uniform_bytes(self) -> [u8; 16] {
        let mut bytes = [0_u8; 16]; // 两个 `vec2<u32>` 共16字节且天然满足uniform对齐。
        bytes[0..4].copy_from_slice(&self.source_width.to_le_bytes()); // 源宽占第一个标量。
        bytes[4..8].copy_from_slice(&self.source_height.to_le_bytes()); // 源高占第二个标量。
        bytes[8..12].copy_from_slice(&self.target_width.to_le_bytes()); // 目标宽占第三个标量。
        bytes[12..16].copy_from_slice(&self.target_height.to_le_bytes()); // 目标高占第四个标量。
        bytes // 返回无填充歧义的固定ABI。
    }

    /// 计算覆盖目标Mip的二维工作组数量。
    pub(super) fn workgroups(self) -> (u32, u32) {
        (
            self.target_width.div_ceil(GPU_MIP_WORKGROUP_SIDE), // X轴尾部由WGSL边界判断裁掉。
            self.target_height.div_ceil(GPU_MIP_WORKGROUP_SIDE), // Y轴使用相同覆盖规则。
        )
    }
}

/// 从合法基础级尺寸构造直到1×1的全部相邻级调度。
pub(super) fn build_gpu_mip_dispatches(
    base_width: u32,
    base_height: u32,
) -> Result<Vec<GpuMipDispatch>, String> {
    if base_width == 0 || base_height == 0 {
        return Err(format!(
            "vegetation GPU mip base extent must be non-zero, got {base_width}x{base_height}"
        )); // WebGPU不允许零尺寸纹理，错误必须在资源创建前显式返回。
    }
    let mut dispatches = Vec::new(); // 1×1基础级合法地不产生任何Compute工作。
    let mut source_level = 0_u32; // 基础级固定为Mip 0。
    let mut source_width = base_width; // 每轮把刚生成的目标宽提升为下一轮源宽。
    let mut source_height = base_height; // 高度独立收敛以支持非正方形图集。
    while source_width > 1 || source_height > 1 {
        let target_width = (source_width / 2).max(1); // 与CPU和离线KTX2链相同：整除二且至少一。
        let target_height = (source_height / 2).max(1); // 单像素轴重复边缘样本而不降到零。
        dispatches.push(GpuMipDispatch {
            source_level,                   // 当前完整级是唯一输入。
            target_level: source_level + 1, // 输出永远是相邻下一层。
            source_width,                   // 上传给WGSL做源坐标钳制。
            source_height,                  // 上传给WGSL做源坐标钳制。
            target_width,                   // 决定X工作组和越界判断。
            target_height,                  // 决定Y工作组和越界判断。
        });
        source_level += 1; // 下一轮从刚写完的级继续递归。
        source_width = target_width; // 宽轴最终稳定为一。
        source_height = target_height; // 高轴最终稳定为一。
    }
    Ok(dispatches) // 顺序可直接在同一命令编码器中逐项提交。
}

/// 在已有RenderApp中安装正式候选Mip和版本化非阻塞回读闭环。
pub(super) fn install(app: &mut App) -> bool {
    if app.get_sub_app_mut(RenderApp).is_none() {
        return false; // 无渲染子应用的逻辑测试继续使用CPU完整Mip回退。
    }
    load_internal_asset!(
        // 使用与叶片核心Shader相同的Bevy内部资产机制。
        app,                                  // 向主应用Shader资产表注册源码。
        GPU_MIP_SHADER_HANDLE,                // 管线缓存使用稳定强类型句柄。
        "shaders/vegetation_layout_mip.wgsl", // 相对当前模块定位正式唯一WGSL。
        Shader::from_wgsl                     // 保留Bevy预处理和异步管线缓存行为。
    );
    load_internal_asset!(
        // 正式基础级竞争与Mip使用相同Bevy内部资产生命周期。
        app,                                      // 向主应用Shader资产表注册竞争源码。
        GPU_COMPOSE_SHADER_HANDLE,                // 独立稳定强类型句柄。
        "shaders/vegetation_layout_compose.wgsl", // 相对密度功能域定位唯一正式WGSL。
        Shader::from_wgsl                         // 保留Bevy预处理和异步管线缓存行为。
    );
    app.add_plugins(ExtractResourcePlugin::<VegetationGpuMipRequest>::default()); // 每帧只克隆当前轻量请求，不复制CPU参考字节。
    let render_app = app
        .get_sub_app_mut(RenderApp)
        .expect("vegetation GPU mip RenderApp existence was checked before installation"); // 同一build调用内子应用不会消失。
    render_app
        .init_resource::<VegetationGpuMipRenderState>() // 无请求时保持空作业且根图零开销快返。
        .init_resource::<VegetationGpuMipShared>() // 异步回调只向小队列追加完成结果。
        .add_systems(
            RenderStartup,
            (
                init_vegetation_gpu_compose_pipeline, // 先排队正式基础级竞争入口。
                init_vegetation_gpu_mip_pipeline,     // 同一启动阶段排队相邻Mip入口。
            ),
        )
        .add_systems(
            Render,
            prepare_vegetation_gpu_mip_job
                .in_set(RenderSystems::PrepareResources)
                .after(RenderSystems::PrepareAssets), // 候选Image已成为GpuImage后才创建逐Mip视图。
        )
        .add_systems(ExtractSchedule, begin_maps_and_publish_vegetation_gpu_mips) // 下一帧发起map_async并把已完成结果写回MainWorld。
        .add_systems(
            RenderGraph,
            run_vegetation_gpu_mip
                .in_set(VegetationRenderGraphSet::CandidateMip)
                .before(camera_driver),
        ); // 候选作业在根图每帧最多执行一次，不随相机倍增。
    true // 主世界状态机必须启用GPU候选门而不是CPU回退。
}

#[cfg(test)]
mod tests; // 调度尺寸和uniform ABI回归拆到子文件。

#[cfg(test)]
mod runtime_tests; // 显式门控的真实Bevy RenderApp完整生命周期双后端烟雾。
