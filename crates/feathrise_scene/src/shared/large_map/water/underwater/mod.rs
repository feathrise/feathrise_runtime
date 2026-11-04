//! 水下视角只接管当前主视图的色温与距离雾，复用已采用水面足迹，不新增全屏通道或水面扫描。
mod components;
mod resources;
mod systems;
mod types;
mod volume; // 仅相机介质范围读取正式地形；静水新建、绘制和外观不依赖此查询。
#[cfg(test)]
mod tests;
#[cfg(test)]
mod integration_tests;

pub(super) use resources::UnderwaterViewState;
pub(super) use systems::{apply_view, prepare_view};
pub(super) use volume::sample_column;

pub(super) fn install(app: &mut bevy::prelude::App) {
    app.init_resource::<UnderwaterViewState>();
}
