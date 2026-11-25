//! 作者项目视口的普通字幕。游戏使用自己的对白 HUD，视频由逐帧采集器合成所选分支。
use super::CinematicPresentation;
use bevy::prelude::*;
use feathrise_cinematic::{CinematicFrame, FrameMode};

#[derive(Component)]
pub(super) struct Subtitle;

pub(super) fn update(
    mut commands: Commands,
    frame: Res<CinematicFrame>,
    receipt: Res<CinematicPresentation>,
    mut labels: Query<(Entity, &UiTargetCamera, &mut Text), With<Subtitle>>,
) {
    let active = frame
        .active
        .as_ref()
        .filter(|f| f.mode == FrameMode::Preview && receipt.ready(f.ticket, f.time));
    let label = active
        .map(|f| {
            f.document
                .dialogue
                .iter()
                .filter(|c| c.time <= f.time && f.time < c.time + c.duration)
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let camera = receipt.camera.filter(|_| !label.is_empty());
    if let Ok((entity, target, mut text)) = labels.single_mut() {
        if Some(target.0) == camera {
            if text.0 != label {
                text.0 = label;
            }
            return;
        }
        commands.entity(entity).despawn();
    }
    let Some(camera) = camera else {
        return;
    };
    commands.spawn((
        Subtitle,
        UiTargetCamera(camera),
        GlobalZIndex(100),
        Node {
            position_type: PositionType::Absolute,
            bottom: percent(7.),
            left: percent(10.),
            width: percent(80.),
            ..default()
        },
        BackgroundColor(Color::srgba(0.03, 0.03, 0.03, 0.8)),
        Text::new(label),
        TextFont::from_font_size(26.),
        TextColor(Color::WHITE),
        TextLayout::new(Justify::Center, LineBreak::WordBoundary),
    ));
}
