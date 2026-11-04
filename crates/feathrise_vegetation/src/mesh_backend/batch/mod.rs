//! 分块共享实例后端：逐株数据不再扩张 ECS、可见性查询和渲染分箱。

mod components;
mod render;
mod resources;
mod selection;
mod spawn;

use feathrise_scene_core::residency::SceneResidencyAppExt;

use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_component::ExtractComponentPlugin;

pub use components::{
    VegetationMeshBatch, VegetationMeshBatchNode, VegetationMeshBatchPlacement,
    VegetationMeshBatchView,
};
pub use resources::VegetationMeshBatchQuality;
pub use spawn::spawn_vegetation_mesh_batch;
pub use spawn::spawn_volume_mesh_batch;
pub use components::VolumeMeshCells;

use components::VegetationMeshBatchInstances;
use resources::VegetationMeshBatchSelectionState;

pub(super) fn install_batch_backend(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<VegetationMeshBatchNode>::default());
    app.init_resource::<VegetationMeshBatchQuality>()
        .init_resident_resource::<VegetationMeshBatchSelectionState>()
        .init_resource::<selection::Workers>() // 工作席位全局共享，休眠地图不另外创建工作池。
        .add_observer(selection::inserted).add_observer(selection::removed)
        .add_plugins((
            ExtractComponentPlugin::<VegetationMeshBatchInstances>::default(),
            ExtractComponentPlugin::<VegetationMeshBatchView>::default(),
        ))
        .add_resident_systems(
            PostUpdate,
            selection::select_vegetation_mesh_batch_lods
                .after(bevy::transform::TransformSystems::Propagate),
        );
    if app.get_sub_app_mut(RenderApp).is_none() {
        return; // 无窗口ECS夹具仍可验证批次层级与LOD选择，不要求Shader资产资源存在。
    }
    let standard_prepass_shader = render::register_batch_shaders(app);
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        unreachable!("上方已经确认RenderApp存在");
    };
    render::install_batch_render_app(render_app, standard_prepass_shader);
}
