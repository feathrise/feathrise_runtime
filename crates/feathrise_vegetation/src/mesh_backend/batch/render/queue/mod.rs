//! 用批次绘制项替换 Bevy 为承接节点生成的单实例 PBR 绘制项。

mod main;
mod prepass;
mod shadow;

pub(super) use main::queue_vegetation_mesh_batches;
pub(super) use prepass::queue_vegetation_mesh_batch_prepasses;
pub(super) use shadow::queue_vegetation_mesh_batch_shadows;
