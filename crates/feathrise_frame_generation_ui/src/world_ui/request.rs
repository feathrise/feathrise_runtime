//! 世界标签高清绘制资格独立于SDK插帧资格；时间抖动主相机即使关闭FG也需要分流。
use bevy::{
    prelude::*,
    render::{Extract, camera::TemporalJitter},
}; // 从本帧最终主世界相机状态提取。
use feathrise_scene_core::view::PrimarySceneView; // 不选择烘焙、地图等辅助相机。

#[derive(Resource, Default)] // RenderWorld独立请求不授权SDK，也不触发屏幕UI捕获。
pub(super) struct WorldUiRequest {
    // 只保留唯一相机身份。
    pub camera: Option<Entity>, // None时保留普通透明绘制。
} // 高清绘制资格结束。

pub(super) fn extract(
    mut output: ResMut<WorldUiRequest>,
    fg: Extract<Res<crate::FrameGenerationUiRequest>>,
    cameras: Extract<Query<(Entity, &Camera, Has<TemporalJitter>), With<PrimarySceneView>>>,
) {
    // 在所有主世界画质租约完成后读取真实相机能力。
    let mut active = cameras.iter().filter(|(_, camera, _)| camera.is_active); // 只接受活跃玩法相机。
    let first = active.next(); // 不克隆实体或场景数据。
    output.camera = match (first, active.next()) {
        // 多主相机明确回退，不猜测共享引导信号的归属。
        (Some((entity, _, temporal)), None)
            if temporal || (fg.enabled && fg.backend_ready && fg.camera == Some(entity)) =>
        {
            Some(entity)
        } // DLSS/DLAA等时间重建独立于FG；无时间重建时保留原FG能力。
        _ => None, // 无相机、相机歧义或两种需求都关闭时不分配独立纹理。
    }; // 每帧覆盖身份，防止地图切换后复用旧相机。
} // 非阻塞资格提取结束。
