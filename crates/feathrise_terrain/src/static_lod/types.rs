//! 静态网格LOD完成值；误差使用源局部米制，实例缩放由运行时保守换算。
use bevy::prelude::{Mesh, Vec3};

pub struct StaticMeshLod {
    pub mesh: Mesh,
    pub max_error: f32,
}

pub struct EditableMeshLodChunk {
    pub center: Vec3,
    pub radius: f32,
    pub levels: Vec<EditableMeshLodIndices>,
}

pub struct EditableMeshLodIndices {
    pub indices: Vec<u32>,
    pub max_error: f32,
}

/// 只服务无贴图内置材质。作者线框UV和逐三角UV不是地表纹理，不能阻止共面点焊接。
/// 派生显示仅保存位置和法线，点边面、保存、碰撞仍读取调用方持有的完整作者网格。
pub struct EditableMeshChunkLods {
    pub(super) source: Mesh,
    pub chunks: Vec<EditableMeshLodChunk>,
}
