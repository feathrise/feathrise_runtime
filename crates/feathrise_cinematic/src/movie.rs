//! 以整数帧号驱动影片。对白停留独立于角色时间，作者选定的分支只在本次导出进程使用。
use crate::Cinematic;
use bevy_ecs::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub const MOVIE_JOB_ENV: &str = "FEATHRISE_CINEMATIC_EXPORT_JOB";
pub const PROTOCOL: u32 = 1;

#[derive(Resource, Default)]
pub struct GameDirectorProvided;
#[derive(Resource, Default)]
pub struct MovieYarnProvider;
#[derive(Resource)]
pub struct MovieDialogue {
    pub document: Arc<Cinematic>,
    pub choices: BTreeMap<String, String>,
    pub lines: Option<Result<BTreeMap<String, Vec<String>>, String>>,
    /// 作者可以直接选择这次 VM 实际开放的选项，显示文字和稳定行 ID 分开传递。
    pub choice: Option<MovieChoicePrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovieChoicePrompt {
    pub key: String,
    pub options: Vec<MovieOption>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovieOption {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MovieSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub seconds_per_line: f64,
    pub instance: Option<String>,
    pub choices: BTreeMap<String, String>,
}
impl Default for MovieSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            seconds_per_line: 2.0,
            instance: None,
            choices: BTreeMap::new(),
        }
    }
}
impl MovieSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width % 2 != 0 || self.height % 2 != 0 {
            return Err("H.264 4:2:0 视频宽、高必须是正偶数".into());
        }
        if self.fps == 0 || !self.seconds_per_line.is_finite() || self.seconds_per_line <= 0. {
            return Err("视频帧率和每句对白停留秒数必须大于零".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovieJob {
    pub version: u32,
    pub project_id: String,
    pub asset: PathBuf,
    pub cinematic: String,
    pub scene: Option<String>,
    pub asset_sha256: String,
    pub settings: MovieSettings,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovieReceipt {
    pub version: u32,
    pub cinematic: String,
    pub frames: u64,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<MovieChoicePrompt>,
}

struct CueLines {
    start: f64,
    duration: f64,
    lines: Vec<String>,
    hold_from: Option<f64>,
}
pub struct MovieTimeline {
    cues: Vec<CueLines>,
    fps: u32,
    seconds_per_line: f64,
    pub frames: u64,
    duration: f64,
}
#[derive(Debug, PartialEq)]
pub struct MovieFrame<'a> {
    pub time: f32,
    pub subtitles: Vec<&'a str>,
}
impl MovieTimeline {
    /// 等待节点的每句占固定放映时长，镜头游标在此期间停住；普通节点在作者原有时段内均分。
    pub fn new(
        doc: &Cinematic,
        settings: &MovieSettings,
        yarn: &BTreeMap<String, Vec<String>>,
    ) -> Result<Self, String> {
        settings.validate()?;
        doc.validate()?;
        let mut extra = 0.;
        let mut cues = Vec::new();
        for cue in &doc.dialogue {
            let lines = if let Some(text) = &cue.text {
                vec![text.clone()]
            } else {
                yarn.get(&cue.id)
                    .ok_or_else(|| format!("对白 {} 缺少 Yarn 导出结果", cue.id))?
                    .clone()
            };
            let hold_from = cue.wait.then_some(cue.time as f64 + extra);
            if cue.wait {
                extra += lines.len() as f64 * settings.seconds_per_line;
            }
            cues.push(CueLines {
                start: cue.time as f64,
                duration: cue.duration as f64,
                lines,
                hold_from,
            });
        }
        let count = ((doc.duration as f64 + extra) * settings.fps as f64).ceil();
        if !count.is_finite() || count >= u64::MAX as f64 {
            return Err("视频帧数超出可表示范围".into());
        }
        Ok(Self {
            cues,
            fps: settings.fps,
            seconds_per_line: settings.seconds_per_line,
            frames: count as u64,
            duration: doc.duration as f64,
        })
    }
    pub fn frame(&self, index: u64) -> Option<MovieFrame<'_>> {
        if index >= self.frames {
            return None;
        }
        let elapsed = index as f64 / self.fps as f64;
        let mut extra = 0.;
        for cue in self.cues.iter().filter(|c| c.hold_from.is_some()) {
            let start = cue.hold_from.unwrap();
            if elapsed < start {
                break;
            }
            let length = cue.lines.len() as f64 * self.seconds_per_line;
            if elapsed < start + length {
                let line = ((elapsed - start) / self.seconds_per_line) as usize;
                return Some(MovieFrame {
                    time: cue.start as f32,
                    subtitles: vec![cue.lines[line].as_str()],
                });
            }
            extra += length;
        }
        let time = (elapsed - extra).clamp(0., self.duration);
        let subtitles = self
            .cues
            .iter()
            .filter(|c| c.hold_from.is_none() && time >= c.start && time < c.start + c.duration)
            .filter_map(|c| {
                let i = (((time - c.start) / c.duration) * c.lines.len() as f64) as usize;
                c.lines.get(i).map(String::as_str)
            })
            .collect();
        Some(MovieFrame {
            time: time as f32,
            subtitles,
        })
    }
}
