//! 场景局部光源使用 glTF 的线性 RGB、坎德拉、米和弧度；相同参数用于摆放草稿、主 GLB 和 Bevy。

use crate::ScenePlacementAssetTarget;
use serde::{Deserialize, Serialize};

pub type LocalLightParameters = ([f32; 3], f32, Option<f32>, Option<[f32; 2]>);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LightDefinition", into = "LightDefinition")]
pub struct SceneLocalLight(LightDefinition);

// 只有下方的校验构造可以产生此类型，NaN 和无穷均不会进入需要 Eq 的摆放目标。
impl Eq for SceneLocalLight {}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind { Point, Spot }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Spot {
    #[serde(default)]
    inner_cone_angle: f32,
    #[serde(default = "default_cone")]
    outer_cone_angle: f32,
}

const fn white() -> [f32; 3] { [1.0; 3] }
const fn one() -> f32 { 1.0 }
const fn default_cone() -> f32 { std::f32::consts::FRAC_PI_4 }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct LightDefinition {
    #[serde(rename = "type")]
    kind: Kind,
    #[serde(default = "white")]
    color: [f32; 3],
    #[serde(default = "one")]
    intensity: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spot: Option<Spot>,
}

impl TryFrom<LightDefinition> for SceneLocalLight {
    type Error = String;
    fn try_from(mut light: LightDefinition) -> Result<Self, String> {
        if !light.color.iter().all(|v| (0.0..=1.0).contains(v))
            || !(light.intensity >= 0.0 && (light.intensity * (4.0 * std::f32::consts::PI)).is_finite())
            || light.range.is_some_and(|r| !r.is_finite() || r <= 0.0) {
            return Err("灯光颜色必须在 0～1 内，光强须非负，照射范围须为正的有限米数".into());
        }
        match light.kind {
            Kind::Point => light.spot = None,
            Kind::Spot => {
                let cone = light.spot.get_or_insert(Spot { inner_cone_angle: 0.0, outer_cone_angle: default_cone() });
                if !(cone.inner_cone_angle >= 0.0 && cone.inner_cone_angle < cone.outer_cone_angle
                    && cone.outer_cone_angle <= std::f32::consts::FRAC_PI_2) {
                    return Err("聚光灯须满足 0° ≤ 内锥半角 < 外锥半角 ≤ 90°".into());
                }
            }
        }
        Ok(Self(light))
    }
}

impl From<SceneLocalLight> for LightDefinition {
    fn from(value: SceneLocalLight) -> Self { value.0 }
}

impl ScenePlacementAssetTarget {
    /// 已发行旧 SDK 可通过作者侧兼容入口隐藏此项；同一标准参数也适用于外部 GLB 工具。
    pub const SUPPORTS_LOCAL_LIGHTS: bool = true;

    pub fn from_local_light(color: [f32; 3], intensity: f32, range: Option<f32>, cone: Option<[f32; 2]>) -> Result<Self, String> {
        let light = LightDefinition {
            kind: if cone.is_some() { Kind::Spot } else { Kind::Point }, color, intensity, range,
            spot: cone.map(|[inner_cone_angle, outer_cone_angle]| Spot { inner_cone_angle, outer_cone_angle }),
        };
        Ok(Self::Light { light: light.try_into()? })
    }

    pub fn local_light_parameters(&self) -> Option<LocalLightParameters> {
        let Self::Light { light: SceneLocalLight(value) } = self else { return None; };
        Some((value.color, value.intensity, value.range, value.spot.map(|s| [s.inner_cone_angle, s.outer_cone_angle])))
    }
}
