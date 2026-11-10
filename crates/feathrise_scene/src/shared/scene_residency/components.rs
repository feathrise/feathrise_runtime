//! 驻留期间由本功能持有的禁用状态；恢复时不覆盖原有显隐、相机或物理禁用责任。

use bevy::prelude::*;

#[derive(Component)]
pub(super) struct ParkedSceneEntity {
    pub disabled_by_us: bool,
    pub visibility: Option<Visibility>,
    pub camera_active: Option<bool>,
    pub body_disabled_by_us: bool,
    pub collider_disabled_by_us: bool,
    pub audio_playing: bool,
    pub spatial_audio_playing: bool,
}

/// 音频仅在完整采用后恢复，避免分帧期间先听到尚未可见的场景。
#[derive(Component)]
pub(super) struct ResumeResidentAudio {
    pub audio: bool,
    pub spatial: bool,
}
