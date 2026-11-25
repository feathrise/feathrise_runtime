//! 独立资产以秒计时，坐标属于绑定的场景实例；可选的游戏条件和收尾动作不参与画面取样。
use serde::{Deserialize, Serialize};

pub(crate) fn yes() -> bool {
    true
}
fn unit_scale() -> [f32; 3] {
    [1.0; 3]
}
fn identity() -> [f32; 4] {
    [0., 0., 0., 1.]
}
fn fov() -> f32 {
    45.0
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pose {
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default = "identity")]
    pub rotation: [f32; 4],
    #[serde(default = "unit_scale")]
    pub scale: [f32; 3],
}
impl Default for Pose {
    fn default() -> Self {
        Self {
            translation: [0.; 3],
            rotation: identity(),
            scale: unit_scale(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraKey {
    pub time: f32,
    pub pose: Pose,
    #[serde(default = "fov")]
    pub fov_y_degrees: f32,
    /// 从上个机位平滑到达此键；关闭时在此键直接切镜。
    #[serde(default)]
    pub smooth: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoseKey {
    pub time: f32,
    pub pose: Pose,
    #[serde(default = "yes")]
    pub smooth: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub time: f32,
    pub duration: f32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionTrack {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default)]
    pub keys: Vec<PoseKey>,
    #[serde(default)]
    pub clips: Vec<Clip>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueCue {
    pub id: String,
    pub time: f32,
    pub duration: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yarn: Option<String>,
    #[serde(default)]
    pub wait: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatPolicy {
    #[default]
    EveryTime,
    OncePerSave,
    OncePerQuest {
        quest: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlaybackCondition {
    Quest {
        quest: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        stage: Option<u32>,
    },
    HasItem {
        item: String,
    },
    Seen {
        cinematic: String,
        seen: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FinishAction {
    SetVariable {
        key: String,
        value: serde_json::Value,
    },
    QuestObjective {
        quest: String,
        from: u32,
        to: u32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cinematic {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub duration: f32,
    #[serde(default = "yes")]
    pub control_camera: bool,
    #[serde(default = "yes")]
    pub control_player: bool,
    #[serde(default)]
    pub camera: Vec<CameraKey>,
    #[serde(default)]
    pub actors: Vec<MotionTrack>,
    #[serde(default)]
    pub objects: Vec<MotionTrack>,
    #[serde(default)]
    pub dialogue: Vec<DialogueCue>,
    #[serde(default)]
    pub repeat: RepeatPolicy,
    #[serde(default)]
    pub conditions: Vec<PlaybackCondition>,
    #[serde(default)]
    pub on_finish: Vec<FinishAction>,
}

impl Cinematic {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            version: 1,
            id: id.into(),
            name: name.into(),
            duration: 5.,
            control_camera: true,
            control_player: true,
            camera: Vec::new(),
            actors: Vec::new(),
            objects: Vec::new(),
            dialogue: Vec::new(),
            repeat: RepeatPolicy::default(),
            conditions: Vec::new(),
            on_finish: Vec::new(),
        }
    }
    pub fn from_slice(bytes: &[u8]) -> Result<Self, String> {
        let src = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let document: Self = serde_json::from_slice(src).map_err(|e| e.to_string())?;
        document.validate()?;
        Ok(document)
    }
}
