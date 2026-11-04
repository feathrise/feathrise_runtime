//! 显式验收下的真实动画探针：冻结当前场景身份，预算扫描后只读取有界目标和正在播放的图。
use super::{AuthorRuntimePreviewScene, AuthorRuntimePreviewStatus};
use bevy::{
    animation::{AnimatedBy, AnimationTargetId, graph::AnimationNodeType},
    asset::AssetId,
    prelude::*,
};
use std::collections::VecDeque;

struct Tracked {
    entity: Entity,
    target: AnimationTargetId,
    player: Entity,
    baseline: Transform,
}
#[derive(Resource, Default)]
pub struct Probe {
    key: Option<(u64, String, Entity)>,
    queue: VecDeque<Entity>,
    tracked: Vec<Tracked>,
    player: Option<Entity>,
    nodes: usize,
    samples: u64,
    maximum_delta: f32,
    frame: u32,
    value: Option<serde_json::Value>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Probe>().add_systems(
        PostUpdate,
        sample
            .after(bevy::animation::animate_targets)
            .before(bevy::transform::TransformSystems::Propagate),
    );
}

pub fn snapshot(
    probe: &Probe,
    scene: &AuthorRuntimePreviewScene,
    frame: u32,
) -> Option<serde_json::Value> {
    (probe.frame == frame
        && probe.key.as_ref() == Some(&(scene.revision, scene.kind.clone(), scene.root)))
    .then(|| probe.value.clone())
    .flatten()
}

// 不携带含完整Rust类型名的Debug文本；技能执行与动画探针必须共同遵守同一个4KiB通道预算。
fn asset_identity<A: Asset>(id: AssetId<A>) -> serde_json::Value {
    match id {
        AssetId::Index { index, .. } => index.to_bits().into(),
        AssetId::Uuid { uuid } => uuid.to_string().into(),
    }
}

// 探针不驱动动画、不写Transform；只采样正式Bevy动画系统本帧已经产生的姿态。
fn sample(
    mut probe: ResMut<Probe>,
    scene: Option<Res<AuthorRuntimePreviewScene>>,
    status: Res<AuthorRuntimePreviewStatus>,
    children: Query<&Children>,
    targets: Query<(&AnimationTargetId, &AnimatedBy, &Transform)>,
    players: Query<(Option<&Name>, &AnimationPlayer, &AnimationGraphHandle)>,
    graphs: Res<Assets<AnimationGraph>>,
    clips: Res<Assets<AnimationClip>>,
    assets: Res<AssetServer>,
    frame: Res<bevy::diagnostic::FrameCount>,
) {
    let Some(scene) =
        scene.filter(|s| status.ready && status.revision == s.revision && status.kind == s.kind)
    else {
        *probe = Probe::default();
        return;
    };
    sample_root(&mut probe, &scene, &children, &targets, &players, &graphs, &clips, &assets, frame.0);
}

/// 编辑器本地RTT与工程预览共用同一真实骨骼探针；调用者只提供当前完整根身份。
/// 不安装工程运行Job、不驱动播放器，也不写入待验姿态。
pub fn sample_root(
    probe: &mut Probe,
    scene: &AuthorRuntimePreviewScene,
    children: &Query<&Children>,
    targets: &Query<(&AnimationTargetId, &AnimatedBy, &Transform)>,
    players: &Query<(Option<&Name>, &AnimationPlayer, &AnimationGraphHandle)>,
    graphs: &Assets<AnimationGraph>,
    clips: &Assets<AnimationClip>,
    assets: &AssetServer,
    frame: u32,
) {
    let key = (scene.revision, scene.kind.clone(), scene.root);
    if probe.key.as_ref() != Some(&key) {
        *probe = Probe {
            key: Some(key),
            queue: VecDeque::from([scene.root]),
            ..default()
        };
    }
    // 最多65536节点，128节点/帧；稳定帧只查询最多16个真实动画目标，绝不扫描全World。
    for _ in 0..128 {
        let Some(entity) = probe.queue.pop_front() else {
            break;
        };
        probe.nodes += 1;
        if probe.nodes > 65_536 {
            probe.queue.clear();
            probe.value = None;
            return;
        }
        if players.get(entity).is_ok() {
            probe.player.get_or_insert(entity);
        }
        if probe.tracked.len() < 16
            && let Ok((target, player, transform)) = targets.get(entity)
        {
            probe.tracked.push(Tracked {
                entity,
                target: *target,
                player: player.0,
                baseline: *transform,
            });
        }
        if let Ok(children) = children.get(entity) {
            probe.queue.extend(children.iter());
        }
    }
    if !probe.queue.is_empty() {
        return;
    }
    probe.value = None; // 本帧没有实际图/目标命中时，不沿用上一帧的动画证据。
    let Some(player_entity) = probe.player else {
        return;
    };
    let Ok((name, player, graph_handle)) = players.get(player_entity) else {
        return;
    };
    let Some(graph) = graphs.get(graph_handle) else {
        return;
    };
    for (node_index, active) in player.playing_animations().take(8) {
        let Some(node) = graph.get(*node_index) else {
            continue;
        };
        let AnimationNodeType::Clip(clip_handle) = &node.node_type else {
            continue;
        };
        let Some(clip) = clips.get(clip_handle) else {
            continue;
        };
        let mut matched = 0;
        let mut delta = 0.0_f32;
        let mut pose = serde_json::Value::Null;
        for tracked in &probe.tracked {
            if tracked.player != player_entity || !clip.curves().contains_key(&tracked.target) {
                continue;
            }
            let Ok((target, animated_by, transform)) = targets.get(tracked.entity) else {
                continue;
            };
            if *target != tracked.target || animated_by.0 != player_entity {
                continue;
            }
            matched += 1;
            let change = tracked
                .baseline
                .rotation
                .angle_between(transform.rotation)
                .abs()
                .max(tracked.baseline.translation.distance(transform.translation));
            if change >= delta {
                delta = change;
                pose = serde_json::json!({"entity":tracked.entity.to_bits(),"target":target.0.to_string(),
                    "baseline_rotation":tracked.baseline.rotation.to_array(),"rotation":transform.rotation.to_array(),
                    "baseline_translation":tracked.baseline.translation.to_array(),"translation":transform.translation.to_array()});
            }
        }
        if matched == 0 {
            continue;
        }
        probe.samples += 1;
        probe.frame = frame;
        probe.maximum_delta = probe.maximum_delta.max(delta);
        probe.value = Some(serde_json::json!({
            "source":"real-animation-player-graph-and-animated-targets",
            "revision":scene.revision,"kind":scene.kind,"root":scene.root.to_bits(),
            "capture_frame":frame,
            "scan_complete":true,"player":player_entity.to_bits(),"player_name":name.map(Name::as_str),
            "graph":asset_identity(graph_handle.0.id()),"clip":asset_identity(clip_handle.id()),
            "derived_clip":assets.get_path(clip_handle.id()).is_none(),"seek":active.seek_time(),
            "matched_targets":matched,"samples":probe.samples,"current_pose_delta":delta,
            "maximum_pose_delta":probe.maximum_delta,"pose_sample":pose,
        }));
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_probe_requires_same_root_revision_kind_and_render_frame() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let other = world.spawn_empty().id();
        let mut scene = AuthorRuntimePreviewScene {
            revision: 7,
            kind: "rpg.character".into(),
            root,
        };
        let probe = Probe {
            key: Some((7, scene.kind.clone(), root)),
            frame: 42,
            value: Some(serde_json::json!({"seek":0.3,"current_pose_delta":0.0})),
            ..default()
        };
        assert!(snapshot(&probe, &scene, 42).is_some());
        assert!(snapshot(&probe, &scene, 43).is_none());
        scene.root = other;
        assert!(snapshot(&probe, &scene, 42).is_none());
        scene.root = root;
        scene.revision = 8;
        assert!(snapshot(&probe, &scene, 42).is_none());
        scene.revision = 7;
        scene.kind = "rpg.skill.v1".into();
        assert!(snapshot(&probe, &scene, 42).is_none());
    }
}
