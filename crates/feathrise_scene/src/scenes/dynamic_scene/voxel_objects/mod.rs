//! 独立有限体素后端；密度、等值面和碰撞在后台准备，主世界有界上传后原子采用。
mod build;
mod cache; // 所有独立对象共用固定64MiB派生块缓存；换窗不重复生成未变的网格。
mod density;
mod resources;
mod surface;
mod systems;
mod window;
pub(super) use resources::VoxelObjectRuntime;
pub(super) use resources::VoxelWorkBudget;
pub(super) use systems::drive;

pub(super) fn install(app: &mut bevy::prelude::App) {
    use feathrise_scene_core::residency::SceneInstanceContextAppExt;
    app.init_instance_resource::<VoxelObjectRuntime>()
        .init_resource::<resources::VoxelWorkBudget>();
}
