//! 通过 glTF 读取器和 Bevy 曲线装配场景对象动画，固定步只采样已准备好的关键帧。

use bevy::{
    animation::gltf_curves::{CubicKeyframeCurve, CubicRotationCurve, SteppedKeyframeCurve},
    curve::{ConstantCurve, Curve, Interval, UnevenSampleAutoCurve},
    gltf::gltf::{self, animation::{Interpolation, Property, util::ReadOutputs}},
    prelude::*,
};
use serde::Deserialize;
use std::{collections::HashMap, fmt::Debug, sync::Arc};

use super::{Curves, SceneMotionCollider, SceneMotionTrack};

trait MotionCurve<T>: Curve<T> + Debug + Send + Sync {}
impl<T, C: Curve<T> + Debug + Send + Sync> MotionCurve<T> for C {}
type Channel<T> = Box<dyn MotionCurve<T>>;

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum Playback {
    #[default]
    Once,
    Loop,
    PingPong,
}

#[derive(Debug, Default)]
pub(super) struct ObjectCurves {
    translation: Option<Channel<Vec3>>,
    rotation: Option<Channel<Quat>>,
    duration: f64,
    playback: Playback,
}

impl ObjectCurves {
    pub(super) fn sample(&self, elapsed: f64, origin: Transform) -> (Vec3, Quat) {
        let time = match self.playback {
            Playback::PingPong => elapsed.min(2.0 * self.duration - elapsed),
            _ => elapsed,
        } as f32;
        (
            self.translation.as_ref().map_or(origin.translation, |curve| curve.sample_clamped(time)),
            self.rotation.as_ref().map_or(origin.rotation, |curve| curve.sample_clamped(time)),
        )
    }

    pub(super) fn advance(&self, elapsed: f64, delta: f64) -> (f64, bool) {
        let next = elapsed + delta;
        if self.duration <= 0.0 {
            return (0.0, false); // 只有第 0 帧关键帧的片段按静置姿势显示。
        }
        match self.playback {
            Playback::Once => (next.min(self.duration), false),
            Playback::Loop => (next.rem_euclid(self.duration), next >= self.duration),
            Playback::PingPong => (next.rem_euclid(2.0 * self.duration), false),
        }
    }
}

#[derive(Deserialize, Default)]
struct ClipExtras {
    #[serde(default)]
    feathrise_scene_animation: bool,
    #[serde(default)]
    feathrise_duration: Option<f64>,
    #[serde(default)]
    feathrise_playback: Playback,
    #[serde(default)]
    feathrise_autoplay: bool,
}

#[derive(Deserialize, Default)]
struct NodeExtras {
    feathrise_id: Option<String>,
    feathrise_motion_collider: Option<SceneMotionCollider>,
    feathrise_scene_id: Option<String>,
    #[serde(default)]
    feathrise_scene_animation: bool,
}

fn extras<T: Default + for<'de> Deserialize<'de>>(value: &gltf::json::Extras) -> Result<T, String> {
    let Some(raw) = value.as_ref() else { return Ok(T::default()); };
    if !raw.get().trim_start().starts_with('{') { return Ok(T::default()); }
    serde_json::from_str(raw.get()).map_err(|error| error.to_string())
}

pub(super) fn read(file: &gltf::Gltf) -> Result<(HashMap<String, Arc<SceneMotionTrack>>, bool), String> {
    let Some(scene) = file.default_scene().or_else(|| file.scenes().next()) else {
        return Ok((HashMap::new(), false));
    };
    let authoritative = scene.nodes().any(|node| {
        node.extras().as_ref().is_some_and(|raw| raw.get().contains("\"feathrise_scene_animation\""))
            && extras::<NodeExtras>(node.extras()).is_ok_and(|extra| {
                extra.feathrise_scene_id.is_some() && extra.feathrise_scene_animation
            })
    });
    if file.animations().next().is_none() {
        return Ok((HashMap::new(), authoritative));
    }
    let mut bindings = HashMap::new();
    let mut visit: Vec<_> = scene.nodes().collect();
    let mut visited = std::collections::HashSet::new();
    while let Some(node) = visit.pop() {
        if !visited.insert(node.index()) { continue; }
        visit.extend(node.children());
        let extra = extras::<NodeExtras>(node.extras())?;
        if let Some(id) = crate::placement_glb::placement_node_id(
            extra.feathrise_id.as_deref(), node.name(), node.mesh().is_some(),
        ) {
            bindings.insert(node.index(), (id, extra.feathrise_motion_collider));
        }
    }
    let relevant = |channel: gltf::animation::Channel<'_>| {
        matches!(channel.target().property(), Property::Translation | Property::Rotation)
            && bindings.contains_key(&channel.target().node().index())
    };

    let mut fallback = None;
    let mut selected = None;
    for animation in file.animations() {
        let extra = extras::<ClipExtras>(animation.extras())?;
        if extra.feathrise_scene_animation {
            selected = Some((animation, extra));
            break;
        }
        if fallback.is_none() && animation.channels().any(relevant) {
            fallback = Some((animation, extra));
        }
    }
    let chosen = if authoritative { selected } else { selected.or(fallback) };
    let Some((animation, extra)) = chosen else { return Ok((HashMap::new(), authoritative)); };
    if !extra.feathrise_autoplay {
        // 已匹配的主 GLB 片段优先，包括 Blender 原生片段。手动沿用普通摆放的初始变换和碰撞。
        return Ok((HashMap::new(), true));
    }
    let mut tracks = HashMap::<usize, ObjectCurves>::new();
    let mut duration = extra.feathrise_duration.unwrap_or(0.0);
    if !duration.is_finite() || duration < 0.0 || duration > f64::from(f32::MAX) {
        return Err("场景 GLB 的动画时长无效".into());
    }
    for channel in animation.channels().filter(|channel| relevant(channel.clone())) {
        let node = channel.target().node().index();
        let reader = channel.reader(|buffer| (buffer.index() == 0).then_some(file.blob.as_deref()).flatten());
        let times = reader.read_inputs().ok_or("场景动画关键帧时间未内嵌在 GLB 中")?.collect::<Vec<_>>();
        if times.first().is_none_or(|&time| time < 0.0)
            || times.iter().any(|time| !time.is_finite())
            || times.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("场景 GLB 关键帧时间必须非负有限且递增".into());
        }
        duration = duration.max(f64::from(*times.last().expect("已校验关键帧非空")));
        let interpolation = channel.sampler().interpolation();
        let track = tracks.entry(node).or_default();
        match reader.read_outputs().ok_or("场景动画关键帧值未内嵌在 GLB 中")? {
            ReadOutputs::Translations(values) if track.translation.is_none() => {
                track.translation = Some(translation_curve(times, values.map(Vec3::from).collect(), interpolation)?);
            }
            ReadOutputs::Rotations(values) if track.rotation.is_none() => {
                track.rotation = Some(rotation_curve(times, values.into_f32().map(Vec4::from).collect(), interpolation)?);
            }
            _ => return Err("场景 GLB 中同一个对象的动画属性重复或无效".into()),
        }
    }
    let tracks = tracks.into_iter().map(|(node, mut curves)| {
        curves.duration = duration;
        curves.playback = extra.feathrise_playback; // 同一场景动作的对象共用时间长度和播放方向。
        let (id, collider) = bindings.remove(&node).expect("已校验动画节点的稳定摆放身份");
        if collider.is_some_and(|shape| !shape.center.is_finite()
            || !(shape.half_extents * 2.0).is_finite() || shape.half_extents.min_element() <= 0.0)
        {
            return Err(format!("对象 {id} 的动画碰撞盒无效"));
        }
        Ok((id, Arc::new(SceneMotionTrack { curves: Curves::Gltf(curves), collider })))
    }).collect::<Result<HashMap<_, _>, _>>()?;
    Ok((tracks, authoritative))
}

fn translation_curve(times: Vec<f32>, values: Vec<Vec3>, interpolation: Interpolation)
    -> Result<Channel<Vec3>, String>
{
    let width = if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
    if values.len() != times.len().saturating_mul(width) || values.iter().any(|value| !value.is_finite()) {
        return Err("场景 GLB 位置关键帧值无效".into());
    }
    if times.len() == 1 {
        return Ok(Box::new(ConstantCurve::new(Interval::EVERYWHERE, values[width / 2])));
    }
    Ok(match interpolation {
        Interpolation::Linear => Box::new(UnevenSampleAutoCurve::new(times.into_iter().zip(values))
            .map_err(|error| error.to_string())?),
        Interpolation::Step => Box::new(SteppedKeyframeCurve::new(times.into_iter().zip(values))
            .map_err(|error| error.to_string())?),
        Interpolation::CubicSpline => Box::new(CubicKeyframeCurve::new(times, values)
            .map_err(|error| error.to_string())?),
    })
}

fn rotation_curve(times: Vec<f32>, mut values: Vec<Vec4>, interpolation: Interpolation)
    -> Result<Channel<Quat>, String>
{
    let width = if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
    if values.len() != times.len().saturating_mul(width) || values.iter().any(|value| !value.is_finite()) {
        return Err("场景 GLB 旋转关键帧值无效".into());
    }
    for value in values.iter_mut().skip(width / 2).step_by(width) {
        *value = value.try_normalize().ok_or("场景 GLB 旋转关键帧不是有效的四元数")?;
    }
    if times.len() == 1 {
        return Ok(Box::new(ConstantCurve::new(Interval::EVERYWHERE, Quat::from_vec4(values[width / 2]))));
    }
    Ok(match interpolation {
        Interpolation::Linear => Box::new(UnevenSampleAutoCurve::new(times.into_iter().zip(values.into_iter().map(Quat::from_vec4)))
            .map_err(|error| error.to_string())?),
        Interpolation::Step => Box::new(SteppedKeyframeCurve::new(times.into_iter().zip(values.into_iter().map(Quat::from_vec4)))
            .map_err(|error| error.to_string())?),
        Interpolation::CubicSpline => Box::new(CubicRotationCurve::new(times, values)
            .map_err(|error| error.to_string())?),
    })
}
