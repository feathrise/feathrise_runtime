//! 分块 Mesh 批次在主世界与 RenderApp 之间共享的轻量组件。
//! 【功能设计】逐株只保存稳定 ID 和变换值；ECS 实体数量只随空间块、LOD 与部件数量增长。

use std::sync::Arc;

use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::sync_component::SyncComponent;

use crate::{VegetationLayerId, VegetationPrototypeId};

use super::super::types::VegetationMeshBatchVertexMode;

/// 一个批次内仍与作者候选一一对应的稳定落点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegetationMeshBatchPlacement {
    pub instance_id: u64,     // 与密度候选、保存数据及近场玩法代理共享同一稳定身份。
    pub transform: Transform, // 世界或批次根局部空间中的完整平移、旋转与缩放。
}

impl VegetationMeshBatchPlacement {
    pub const fn new(instance_id: u64, transform: Transform) -> Self {
        Self {
            instance_id,
            transform,
        }
    }
}

/// 一个空间块或编辑器安装片段的通用批次根身份。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq)]
pub struct VegetationMeshBatch {
    pub layer: VegetationLayerId,
    pub prototype: VegetationPrototypeId,
    pub batch_id: u64,
    pub instance_count: usize,
}

/// 允许海量 Mesh 批次按该玩法视图的观察位置执行逐实例 LOD。
///
/// 同一时刻只应有一个该标记；F12 会把它与 GPU 叶片视图一起临时移交给编辑器相机。
#[derive(Clone, Copy, Debug, Component, Default, PartialEq, Eq)]
pub struct VegetationMeshBatchView;

impl ExtractComponent<bevy::render::RenderApp> for VegetationMeshBatchView {
    type QueryData = &'static VegetationMeshBatchView;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(_item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(Self)
    }
}

impl SyncComponent<bevy::render::RenderApp> for VegetationMeshBatchView {
    type Target = Self;
}

/// 批次根拥有的只读逐株落点表；全部 LOD/部件子节点共享同一份 Arc。
#[derive(Clone, Component)]
pub(super) struct VegetationMeshBatchPlacements(pub(super) Arc<[VegetationMeshBatchPlacement]>);

/// 一个批次 LOD/部件节点的不可变作者规格。
#[derive(Clone, Component)]
pub struct VegetationMeshBatchNode {
    pub(super) authored_range: bevy::camera::visibility::VisibilityRange,
    /// 由原型中的实际顺序决定最远档，避免批量后端写死“四档/LOD3”。
    pub(super) farthest_lod: bool,
    pub(super) local_transform: Transform,
    pub(super) distance_offset: Vec3,
    pub(super) vertex_mode: VegetationMeshBatchVertexMode,
}
impl ExtractComponent<bevy::render::RenderApp> for VegetationMeshBatchNode {
    type QueryData = &'static Self;
    type QueryFilter = ();
    type Out = Self;
    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        Some(item.clone())
    }
}
impl SyncComponent<bevy::render::RenderApp> for VegetationMeshBatchNode {
    type Target = Self;
}

/// 主世界已为当前玩法观察位置选中的一条紧凑实例。
#[derive(Clone, Copy, Debug)]
pub(super) struct VegetationMeshSelectedInstance {
    pub(super) world_from_local: Mat4,
    pub(super) lod_range: Vec4,
    pub(super) determinant_positive: bool,
}

/// 变化驱动的当前 LOD 选中结果；提取只克隆 Arc，不逐帧复制实例数组。
#[derive(Clone, Component)]
pub(super) struct VegetationMeshBatchInstances {
    pub(super) volume: Option<VolumeMeshCells>,
    pub(super) revision: u64,
    pub(super) vertex_mode: VegetationMeshBatchVertexMode,
    pub(super) instances: Arc<[VegetationMeshSelectedInstance]>,
}

impl VegetationMeshBatchInstances {
    pub(super) fn empty(vertex_mode: VegetationMeshBatchVertexMode) -> Self {
        Self {
            volume: None,
            revision: 0,
            vertex_mode,
            instances: Arc::from([]),
        }
    }
}


/// 批次只提取一份层级变换，源实例矩阵缓冲不随整体拖动重上传。
#[derive(Component, Clone, Copy)]
pub(super) struct VegetationMeshBatchFrame(pub bevy::math::Affine3);
/// 原始三维作者格；同一格的候选、旋转、缩放和法线基底由顶点GPU派生。
#[derive(Clone)]
pub struct VolumeMeshCells {
    /// 位置/密度、法线/格距、稳定作者中心/身份；与Blade三像素源共用。
    pub cells: Arc<[[[f32; 4]; 3]]>,
    pub seed: u32,
    pub spacing: f32,
    pub scale_range: [f32; 2],
    pub ground_embed_depth: f32,
    pub min: Vec3,
    pub max: Vec3,
}

impl VolumeMeshCells {
    /// 仅供有界近场物理使用，渲染仍以原始作者格在GPU中选择及生成姿态。
    pub fn physics_pose(&self, cell: &[[f32; 4]; 3]) -> Option<Transform> {
        let p = Vec3::from_array([cell[0][0], cell[0][1], cell[0][2]]);
        let n = Vec3::from_array([cell[1][0], cell[1][1], cell[1][2]]).try_normalize()?;
        let mut h = cell[2][3] as u32 ^ self.seed; // LOD重投射位置变化不能改变碰撞与GPU共同的随机选择。
        h = (h ^ (h >> 16)).wrapping_mul(2246822519);
        h = (h ^ (h >> 13)).wrapping_mul(3266489917);
        h ^= h >> 16;
        let probability =
            (cell[0][3] * cell[1][3] * cell[1][3] / self.spacing.max(0.01).powi(2)).clamp(0., 1.);
        if h as f32 / 4294967295.0 >= probability {
            return None;
        }
        let angle = (h & 65535) as f32 / 65535. * std::f32::consts::TAU;
        let scale = self.scale_range[0]
            + (self.scale_range[1] - self.scale_range[0]) * ((h >> 16) as f32 / 65535.);
        let tangent = (if n.y.abs() > 0.95 { Vec3::X } else { Vec3::Y })
            .cross(n)
            .normalize();
        let bitangent = tangent.cross(n);
        let (sin, cos) = angle.sin_cos();
        let x = tangent * cos + bitangent * sin;
        let z = bitangent * cos - tangent * sin;
        Some(
            Transform::from_translation(p - n * self.ground_embed_depth)
                .with_rotation(Quat::from_mat3(&Mat3::from_cols(x, n, z)))
                .with_scale(Vec3::splat(scale)),
        )
    }
}

impl ExtractComponent<bevy::render::RenderApp> for VegetationMeshBatchInstances {
    type QueryData = (&'static VegetationMeshBatchInstances, Option<&'static Transform>, Option<&'static GlobalTransform>);
    type QueryFilter = ();
    type Out = (Self, VegetationMeshBatchFrame);

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        let (instances, local, global) = item;
        let frame = match (local, global) {
            (Some(local), Some(global)) => (global.affine() * local.compute_affine().inverse()).into(),
            _ => bevy::math::Affine3::IDENTITY,
        };
        Some((instances.clone(), VegetationMeshBatchFrame(frame)))
    }
}

impl SyncComponent<bevy::render::RenderApp> for VegetationMeshBatchInstances {
    type Target = Self;
}
