//! 源 Mesh 的分帧捕获、有界后台转换与主线程单项采用。

mod capture; // 每帧最多2MiB复制源属性，并启动最多两个后台任务。
mod convert; // 不访问ECS或Assets的纯CPU兼容Mesh构建。
mod identity; // 后台完成值采用前的代次、实体、资产、修订与票据校验。

use bevy::{
    prelude::Mesh, // 后台结果携带尚未登记的派生Mesh。
    tasks::Task,   // AsyncComputeTaskPool返回的非阻塞任务句柄。
};

use crate::types::AdaptationIdentity; // 所有捕获、任务与结果都锁定完整场景身份。

pub(crate) use capture::{capture_mesh_inputs, poll_and_adopt_mesh_results};

/// 当前分帧复制到哪一种源数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapturePhase {
    Positions, // Float32x3位置。
    Normals,   // Float32x3法线。
    Uvs,       // 可选Float32x2原UV0。
    Indices,   // U16或U32三角索引统一复制成U32。
    Complete,  // 全部输入已捕获，可移交后台任务。
}

/// 一项源 Mesh 的跨帧CPU捕获缓冲。
pub(crate) struct MeshCaptureJob {
    identity: AdaptationIdentity, // 每帧复制前重新核对代次、根、源资产与修订。
    phase: CapturePhase,          // 当前属性阶段。
    offset: usize,                // 当前属性已经复制的元素数量。
    positions: Vec<[f32; 3]>,     // 后台切线和最终Mesh使用的位置快照。
    normals: Vec<[f32; 3]>,       // 后台切线和正交基使用的法线快照。
    uvs: Option<Vec<[f32; 2]>>,   // 地形保留原UV；无UV村落/村路保持None。
    indices: Vec<u32>,            // 捕获时即把U16提升为U32。
    expected_vertices: usize,     // 发现阶段冻结的完整顶点数。
    expected_indices: usize,      // 发现阶段冻结的完整索引数。
    requires_source_uv: bool,     // 地形缺UV必须失败，不能走零UV路径。
}

/// 已脱离ECS和Assets、可安全移动到后台线程的完整输入。
struct CapturedMeshInput {
    identity: AdaptationIdentity, // 完成结果原样返回的不可混用身份。
    positions: Vec<[f32; 3]>,     // 完整位置数组。
    normals: Vec<[f32; 3]>,       // 完整法线数组。
    uvs: Option<Vec<[f32; 2]>>,   // 原UV或None。
    indices: Vec<u32>,            // 完整U32索引数组。
}

/// 后台纯CPU转换完成但尚未登记到 `Assets<Mesh>` 的结果。
pub(crate) struct AdaptedMeshOutput {
    pub(crate) identity: AdaptationIdentity, // 主线程采用前再次核对全部身份字段。
    pub(crate) mesh: Mesh,                   // 只含四项属性且enable_raytracing的派生Mesh。
    pub(crate) derived_bytes: u64,           // 本结果单帧上传预算与工作集估算。
}

/// 一个正在运行或已经标记过期的有界后台任务。
pub(crate) struct PendingAdaptationTask {
    pub(crate) identity: AdaptationIdentity, // 便于取消和诊断，不依赖任务闭包内部状态。
    pub(crate) cancelled: bool,              // 过期任务完成后只回收结果，绝不登记资产。
    pub(crate) estimated_bytes: u64,         // 输入与派生输出并存阶段的保守CPU工作集估算。
    pub(crate) task: Task<Result<AdaptedMeshOutput, String>>, // 纯CPU转换结果或明确失败文本。
}

pub(crate) type CompletedAdaptation = Result<AdaptedMeshOutput, (AdaptationIdentity, String)>; // 完成队列保持失败身份。

use convert::adapt_captured_mesh; // 仅同一功能域的捕获子模块调用，不扩大到crate公共接口。
