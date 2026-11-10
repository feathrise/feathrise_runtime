//! 不可变显示正文与有界姿态；宿主不解释领域预设、装备或动画语义。
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDomainVisuals {
    pub lines: DomainBlob,
    pub prefab: Option<DomainBlob>,
    pub runtime: Option<DomainBlob>,
    pub ghost: Option<DomainGhost>,
    pub spacing: f32,
    pub visible_cells: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainGhost {
    pub position: Option<[f32; 3]>,
    pub anchor: Option<String>,
    pub follow_pointer: bool,
    pub yaw_degrees: f32,
    pub elevation: f32,
}
impl SceneDomainVisuals {
    pub fn validate(&self) -> Result<(), String> {
        self.lines.validate()?;
        if let Some(blob) = &self.prefab {
            blob.validate()?;
        }
        if let Some(blob) = &self.runtime {
            blob.validate()?;
        }
        if !self.spacing.is_finite()
            || self.spacing <= 0.
            || self.visible_cells == 0
            || self.ghost.as_ref().is_some_and(|g| {
                !g.yaw_degrees.is_finite()
                    || !g.elevation.is_finite()
                    || g.position.iter().flatten().any(|v| !v.is_finite())
                    || g.anchor.as_ref().is_some_and(|s| !short(s))
            })
        {
            return Err("作者显示密度或ghost姿态无效".into());
        }
        Ok(())
    }
}
