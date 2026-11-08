//! 活动遮雨批次采用明确32体作者预算，超限由宿主拒绝整批。

use super::types::RainShelterVolume;
use bevy::prelude::*; // 资源与调度标签。
use bevy::render::render_resource::ShaderType; // 生产GPU布局编码。 // 单个固定96字节OBB。

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType, bytemuck::Pod, bytemuck::Zeroable)] // 与WGSL的3088字节布局完全一致。
pub struct RainShelterGpu {
    // 只有完整有效作者批次才能发布。
    pub header: Vec4,                      // X有效体数，Y所有体世界最高Y，ZW预留。
    pub shelters: [RainShelterVolume; 32], // 明确预算，禁止静默截断超限作者数据。
} // 完成共享缓冲契约。

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)] // CPU和GPU都观察同次完整发布。
pub struct RainShelterState {
    // 地图宿主负责来源生命周期。
    pub gpu: RainShelterGpu, // 稳定帧保持相等，避免重新上传。
} // 完成活动资源。

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)] // 宿主在该阶段前发布变换完成的批次。
pub struct RainShelterUpload; // 材质提取前只有一个固定缓冲写侧。
