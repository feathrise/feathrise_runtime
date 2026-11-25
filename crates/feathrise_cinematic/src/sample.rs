//! 只按演出游标决定画面。向后拖动、跳到终点、固定帧率渲染都走与正向播放相同的插值。
use crate::{Cinematic, DialogueCue, MotionTrack, Pose};
use bevy_math::{Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub pose: Pose,
    pub fov_y_degrees: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipSample<'a> {
    pub name: &'a str,
    pub time: f32,
}
#[derive(Clone, Debug)]
pub struct TargetSample<'a> {
    pub target: &'a str,
    pub instance: Option<&'a str>,
    pub pose: Option<Pose>,
    pub clip: Option<ClipSample<'a>>,
}
#[derive(Debug)]
pub struct Sample<'a> {
    pub camera: Option<CameraPose>,
    pub actors: Vec<TargetSample<'a>>,
    pub objects: Vec<TargetSample<'a>>,
    pub dialogue: Vec<&'a DialogueCue>,
}

fn interpolate(from: Pose, to: Pose, t: f32) -> Pose {
    let blend = |a, b| Vec3::from_array(a).lerp(Vec3::from_array(b), t).to_array();
    let q = |a| Quat::from_array(a).normalize();
    Pose {
        translation: blend(from.translation, to.translation),
        scale: blend(from.scale, to.scale),
        rotation: q(from.rotation).slerp(q(to.rotation), t).to_array(),
    }
}

fn interval<K>(keys: &[K], time: f32, key: impl Fn(&K) -> (f32, bool)) -> Option<(&K, &K, f32)> {
    let next = keys.partition_point(|k| key(k).0 <= time);
    let left = keys.get(next.checked_sub(1)?)?;
    let Some(right) = keys.get(next) else {
        return Some((left, left, 0.));
    };
    let (end, smooth) = key(right);
    if !smooth {
        return Some((left, left, 0.));
    }
    let start = key(left).0;
    let t = (time - start) / (end - start);
    Some((left, right, t * t * (3. - 2. * t)))
}

impl MotionTrack {
    pub fn sample(&self, time: f32) -> TargetSample<'_> {
        let pose = interval(&self.keys, time, |k| (k.time, k.smooth))
            .map(|(a, b, t)| interpolate(a.pose, b.pose, t));
        let index = self.clips.partition_point(|clip| clip.time <= time);
        // 已结束片段停在末姿，下一片段以自己的起点开始。终态重建也采用这个结果。
        let clip = index.checked_sub(1).map(|i| {
            let c = &self.clips[i];
            ClipSample {
                name: &c.name,
                time: (time - c.time).min(c.duration),
            }
        });
        TargetSample {
            target: &self.target,
            instance: self.instance.as_deref(),
            pose,
            clip,
        }
    }
}

impl Cinematic {
    /// 对白或字幕系统只读该时刻的台词，避免为取一行文字再次采样所有角色和物件。
    pub fn dialogue_at(&self, time: f32) -> impl Iterator<Item = &DialogueCue> {
        self.dialogue
            .iter()
            .take_while(move |cue| cue.time <= time)
            .filter(move |cue| time < cue.time + cue.duration)
    }

    pub fn sample(&self, time: f32) -> Sample<'_> {
        let time = if time.is_nan() {
            0.
        } else {
            time.clamp(0., self.duration)
        };
        let camera =
            interval(&self.camera, time, |k| (k.time, k.smooth)).map(|(a, b, t)| CameraPose {
                pose: interpolate(a.pose, b.pose, t),
                fov_y_degrees: a.fov_y_degrees + (b.fov_y_degrees - a.fov_y_degrees) * t,
            });
        Sample {
            camera,
            actors: self.actors.iter().map(|t| t.sample(time)).collect(),
            objects: self.objects.iter().map(|t| t.sample(time)).collect(),
            dialogue: self.dialogue_at(time).collect(),
        }
    }
}
