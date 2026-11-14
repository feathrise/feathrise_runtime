//! 通用静态网格LOD保留的原始作者来源；显示替换不改变作者资产身份。
use bevy::prelude::*;

/// 作者采样、GLTF子资产定位和真实碰撞始终读取这份原始句柄。
#[derive(Component, Clone)]
pub struct StaticMeshLodOriginal {
    pub mesh: Handle<Mesh>,
}

/// F12作者模式临时要求原始显示；不修改全局质量偏好或场景LOD策略。
#[derive(Resource, Default)]
pub struct MeshLodAuthoringState {
    pub active: bool,
}

/// 只反映已经采用的实际网格，供诊断和验收读取，不参与作者保存。
#[derive(Component, Clone, Debug)]
pub struct StaticMeshChunkLodStatus {
    pub levels: Vec<usize>,
    pub triangles: usize,
}
