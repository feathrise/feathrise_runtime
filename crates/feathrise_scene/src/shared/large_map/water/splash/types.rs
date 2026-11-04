//! 近景水花局部时钟与切图身份。
use bevy::prelude::*; // 世界向量与实体身份。
#[derive(Default)] // 不累计暂停/卡帧期间的补发债务。
pub(in super::super) struct SplashClock {
    pub(super) previous: Option<f32>,
    pub(super) accumulator: f32,
    pub(super) sequence: u32,
    pub(super) previous_player: Option<Vec3>,
    pub(super) previous_entity: Option<Entity>,
    pub(super) previous_target: Option<Entity>,
    pub(super) previous_bounds: Vec4,
    pub(super) previous_mask: Option<bevy::asset::AssetId<Image>>,
    pub(super) previous_scene: Option<feathrise_scene_core::state::ScenePhase>,
    pub(super) previous_map: Option<feathrise_map_content::MapId>,
} // 所有时钟来自共享暂停感知视觉秒。
