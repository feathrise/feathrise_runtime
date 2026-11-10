//! 作者视图的无限网格GPU绑定；只挂在已有渲染相机，不新增场景或网格实体。
use bevy::{prelude::*, render::render_resource::{BindGroup, BufferId}};

#[derive(Component)]
pub(super) struct GridBindGroup {
    pub(super) buffers: (BufferId, BufferId),
    pub(super) binding: BindGroup,
}
