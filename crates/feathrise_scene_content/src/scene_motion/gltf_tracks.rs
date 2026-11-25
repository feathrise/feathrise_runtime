//! 通过 glTF 读取器和 Bevy 曲线装配场景对象动画，固定步只采样已准备好的关键帧。

use bevy::{
    animation::gltf_curves::{CubicKeyframeCurve, CubicRotationCurve, SteppedKeyframeCurve},
    curve::{ConstantCurve, Curve, Interval, UnevenSampleAutoCurve},
    gltf::gltf::{self, animation::{Interpolation, Property, util::ReadOutputs}},
    prelude::*,
};
use serde::Deserialize;
use std::{collections::HashMap, fmt::Debug, sync::Arc};

use super::{Curves, SceneMotionClip, SceneMotionClips, SceneMotionCollider, SceneMotionPlayback as Playback, SceneMotionTrack};
use crate::{MAX_SCENE_PLACEMENT_SCALE, MIN_SCENE_PLACEMENT_SCALE};

trait MotionCurve<T>: Curve<T> + Debug + Send + Sync {}
impl<T, C: Curve<T> + Debug + Send + Sync> MotionCurve<T> for C {}
type Channel<T> = Box<dyn MotionCurve<T>>;

#[derive(Debug, Default)]
pub(super) struct ObjectCurves {
    translation: Option<Channel<Vec3>>,
    rotation: Option<Channel<Quat>>,
    scale: Option<Channel<Vec3>>,
    duration: f64,
    playback: Playback,
}

impl ObjectCurves {
    pub(super) fn sample(&self, elapsed: f64, origin: Transform) -> Transform {
        let time = match self.playback {
            Playback::PingPong => elapsed.min(2.0 * self.duration - elapsed),
            _ => elapsed,
        } as f32;
        let scale = self.scale.as_ref().map_or(origin.scale, |curve| {
            let value = curve.sample_clamped(time);
            // 三次曲线切线可越出有效范围。避免瞬时退化为零厚度或翻面碰撞。
            if value.is_finite() { value.clamp(scale_min(), scale_max()) } else { origin.scale }
        });
        Transform {
            translation: self.translation.as_ref().map_or(origin.translation, |curve| curve.sample_clamped(time)),
            rotation: self.rotation.as_ref().map_or(origin.rotation, |curve| curve.sample_clamped(time)),
            scale,
        }
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
    feathrise_clip_id: Option<String>,
    #[serde(default)]
    feathrise_duration: Option<f64>,
    #[serde(default)]
    feathrise_playback: Playback,
    #[serde(default)]
    feathrise_autoplay: bool,
    #[serde(default)]
    feathrise_motion_triggers: Vec<super::MotionRegion>,
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

type Bindings = HashMap<usize, (String, Option<SceneMotionCollider>)>;

pub(super) fn read(file: &gltf::Gltf) -> Result<(SceneMotionClips, bool), String> {
    let Some(scene) = file.default_scene().or_else(|| file.scenes().next()) else {
        return Ok((SceneMotionClips::default(), false));
    };
    let authoritative = scene.nodes().any(|node| {
        node.extras().as_ref().is_some_and(|raw| raw.get().contains("\"feathrise_scene_animation\""))
            && extras::<NodeExtras>(node.extras()).is_ok_and(|extra| {
                extra.feathrise_scene_id.is_some() && extra.feathrise_scene_animation
            })
    });
    if file.animations().next().is_none() {
        return Ok((SceneMotionClips::default(), authoritative));
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
        matches!(channel.target().property(), Property::Translation | Property::Rotation | Property::Scale)
            && bindings.contains_key(&channel.target().node().index())
    };

    let mut fallback = None;
    let mut clips = Vec::new();
    for animation in file.animations() {
        let extra = extras::<ClipExtras>(animation.extras())?;
        if extra.feathrise_scene_animation {
            clips.push(read_clip(file, animation, extra, &bindings)?);
        } else if !authoritative && fallback.is_none() && animation.channels().any(relevant) {
            fallback = Some((animation, extra));
        }
    }
    // 无场景作者标记的 Blender 文件继续选择第一个相容片段。旧显式删除标记仍压住旧旁档。
    if clips.is_empty() && let Some((animation, extra)) = fallback {
        clips.push(read_clip(file, animation, extra, &bindings)?);
    }
    let authoritative = authoritative || !clips.is_empty();
    Ok((SceneMotionClips::from_clips(clips)?, authoritative))
}

fn read_clip(
    file: &gltf::Gltf, animation: gltf::Animation<'_>, extra: ClipExtras, bindings: &Bindings,
) -> Result<SceneMotionClip, String> {
    // 兼容 ID 必须使用 glTF 的绝对片段索引，与显示顺序、名称及未标记外部片段无关。
    let id = extra.feathrise_clip_id.unwrap_or_else(|| format!("scene-clip-{}", animation.index()));
    if id.trim().is_empty() { return Err("场景 GLB 的动画片段 ID 为空".into()); }
    let name = animation.name().unwrap_or(&id).to_owned();
    let mut tracks = HashMap::<usize, ObjectCurves>::new();
    let mut duration = extra.feathrise_duration.unwrap_or(0.0);
    if !duration.is_finite() || duration < 0.0 || duration > f64::from(f32::MAX) {
        return Err("场景 GLB 的动画时长无效".into());
    }
    for channel in animation.channels().filter(|channel| {
        matches!(channel.target().property(), Property::Translation | Property::Rotation | Property::Scale)
            && bindings.contains_key(&channel.target().node().index())
    }) {
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
                track.translation = Some(vector_curve(times, values.map(Vec3::from).collect(), interpolation, "位置")?);
            }
            ReadOutputs::Rotations(values) if track.rotation.is_none() => {
                track.rotation = Some(rotation_curve(times, values.into_f32().map(Vec4::from).collect(), interpolation)?);
            }
            ReadOutputs::Scales(values) if track.scale.is_none() => {
                let values = values.map(Vec3::from).collect::<Vec<_>>();
                let width = if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
                if values.iter().skip(width / 2).step_by(width)
                    .any(|value| value.cmplt(scale_min()).any() || value.cmpgt(scale_max()).any())
                {
                    return Err("场景 GLB 缩放关键帧须在摆放缩放范围内".into());
                }
                track.scale = Some(vector_curve(times, values, interpolation, "缩放")?);
            }
            _ => return Err("场景 GLB 中同一个对象的动画属性重复或无效".into()),
        }
    }
    let mut compiled = HashMap::with_capacity(tracks.len());
    for (node, mut curves) in tracks {
        curves.duration = duration;
        curves.playback = extra.feathrise_playback; // 同一场景动作的对象共用时间长度和播放方向。
        let (placement_id, collider) = &bindings[&node];
        if collider.is_some_and(|shape| !shape.center.is_finite()
            || !(shape.half_extents * 2.0).is_finite() || shape.half_extents.min_element() <= 0.0)
        {
            return Err(format!("对象 {placement_id} 的动画碰撞盒无效"));
        }
        if compiled.insert(placement_id.clone(), Arc::new(SceneMotionTrack {
            curves: Curves::Gltf(curves), collider: *collider,
        })).is_some() {
            return Err(format!("场景动画片段 {id} 的摆放目标 {placement_id} 重复"));
        }
    }
    Ok(SceneMotionClip { id, name, duration, playback: extra.feathrise_playback,
        auto_play: extra.feathrise_autoplay, regions: extra.feathrise_motion_triggers, tracks: compiled })
}

fn scale_min() -> Vec3 { Vec3::splat(MIN_SCENE_PLACEMENT_SCALE) }
fn scale_max() -> Vec3 { Vec3::splat(MAX_SCENE_PLACEMENT_SCALE) }

fn vector_curve(times: Vec<f32>, values: Vec<Vec3>, interpolation: Interpolation, property: &str)
    -> Result<Channel<Vec3>, String>
{
    let width = if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
    if values.len() != times.len().saturating_mul(width) || values.iter().any(|value| !value.is_finite()) {
        return Err(format!("场景 GLB {property}关键帧值无效"));
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
