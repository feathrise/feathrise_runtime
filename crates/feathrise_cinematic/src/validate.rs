//! 在资产进入播放器之前诊断冲突，保留作者输入的顺序和稳定身份。
use crate::*;
use std::collections::HashSet;

fn named(value: &str) -> bool {
    !value.trim().is_empty()
}

fn pose_ok(p: Pose) -> bool {
    let q = bevy_math::Quat::from_array(p.rotation);
    p.translation.iter().chain(&p.scale).all(|v| v.is_finite())
        && q.length_squared().is_finite()
        && q.length_squared() > 0.
}

impl Cinematic {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("不支持剧情演出版本 {}", self.version));
        }
        if !named(&self.id)
            || !named(&self.name)
            || !self.duration.is_finite()
            || self.duration <= 0.
        {
            return Err("剧情演出需要稳定 ID、名称和大于零的有限时长".into());
        }
        let time_ok = |t: f32| t.is_finite() && (0. ..=self.duration).contains(&t);
        let span_ok = |t: f32, d: f32| time_ok(t) && d.is_finite() && d > 0. && time_ok(t + d);
        if self.camera.iter().any(|k| {
            !time_ok(k.time)
                || !pose_ok(k.pose)
                || !(k.fov_y_degrees > 0. && k.fov_y_degrees < 180.)
        }) || self.camera.windows(2).any(|w| w[0].time >= w[1].time)
        {
            return Err("镜头键须按时间递增，使用有效姿态及 0 至 180 度之间的视场角".into());
        }
        for tracks in [&self.actors, &self.objects] {
            let mut bindings = HashSet::new();
            for t in tracks {
                if !named(&t.target)
                    || t.instance.as_ref().is_some_and(|i| !named(i))
                    || !bindings.insert((&t.instance, &t.target))
                {
                    return Err(format!("演出目标 {:?} 名称为空或重复绑定", t.target));
                }
                if t.keys.iter().any(|k| !time_ok(k.time) || !pose_ok(k.pose))
                    || t.keys.windows(2).any(|w| w[0].time >= w[1].time)
                {
                    return Err(format!("{} 的变换键无效或时间未递增", t.target));
                }
                if t.clips
                    .iter()
                    .any(|c| !named(&c.name) || !span_ok(c.time, c.duration))
                    || t.clips
                        .windows(2)
                        .any(|w| w[0].time + w[0].duration > w[1].time)
                {
                    return Err(format!("{} 的动作片段无效、越界或重叠", t.target));
                }
            }
        }
        let mut ids = HashSet::new();
        for cue in &self.dialogue {
            if !named(&cue.id)
                || !ids.insert(&cue.id)
                || !span_ok(cue.time, cue.duration)
                || !matches!((&cue.text,&cue.yarn),(Some(s),None)|(None,Some(s)) if named(s))
            {
                return Err(format!(
                    "对白 {} 需要唯一 ID、有效时段，以及字幕或 Yarn 节点之一",
                    cue.id
                ));
            }
        }
        if self.dialogue.windows(2).any(|w| w[0].time > w[1].time) {
            return Err("对白需按起始时间排序".into());
        }
        if let RepeatPolicy::OncePerQuest { quest } = &self.repeat
            && !named(quest)
        {
            return Err("每次任务一次需要任务 ID".into());
        }
        for condition in &self.conditions {
            let ok = match condition {
                PlaybackCondition::Quest {
                    quest,
                    status,
                    stage,
                } => named(quest) && (stage.is_some() || status.as_deref().is_some_and(named)),
                PlaybackCondition::HasItem { item } => named(item),
                PlaybackCondition::Seen { cinematic, .. } => named(cinematic),
            };
            if !ok {
                return Err(format!("无效的播放条件 {condition:?}"));
            }
        }
        for action in &self.on_finish {
            let ok = match action {
                FinishAction::SetVariable { key, value } => {
                    named(key)
                        && (value.is_boolean()
                            || value.is_string()
                            || value.as_f64().is_some_and(|n| (n as f32).is_finite()))
                }
                FinishAction::QuestObjective { quest, .. } => named(quest),
            };
            if !ok {
                return Err(format!("无效的收尾动作 {action:?}"));
            }
        }
        Ok(())
    }
}
