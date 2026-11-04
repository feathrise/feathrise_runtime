//! 网格显示与碰撞LOD的共享作者合同；策略只决定简化，关闭时保留原始几何和物理。

use serde::{Deserialize, Serialize};

/// 实例可分别继承全局、保持原始质量，或覆盖当前通道的简化精度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "settings", rename_all = "snake_case")]
pub enum MeshLodMode<T> {
    #[default]
    Inherit,
    Disabled,
    Override(T),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshDisplayLodSettings {
    /// 后端必须以真实几何误差投影到屏幕；不能用距离冒充像素误差。
    pub max_error_pixels: f32,
    pub hysteresis_ratio: f32,
}

impl Default for MeshDisplayLodSettings {
    fn default() -> Self {
        Self { max_error_pixels: 1.0, hysteresis_ratio: 0.15 }
    }
}

impl MeshDisplayLodSettings {
    pub fn validate(self) -> Result<(), String> {
        if !self.max_error_pixels.is_finite() || !(0.1..=16.0).contains(&self.max_error_pixels)
            || !self.hysteresis_ratio.is_finite() || !(0.0..=0.5).contains(&self.hysteresis_ratio)
        {
            return Err("网格显示LOD的像素误差须为0.1至16，迟滞比例须为0至0.5".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshCollisionLodSettings {
    /// 玩家及附近角色的安全区保持原始碰撞；渲染相机不参与物理精度选择。
    pub safe_near_meters: f32,
    pub max_error_meters: f32,
}

impl Default for MeshCollisionLodSettings {
    fn default() -> Self {
        Self { safe_near_meters: 32.0, max_error_meters: 0.02 }
    }
}

impl MeshCollisionLodSettings {
    pub fn validate(self) -> Result<(), String> {
        if !self.safe_near_meters.is_finite() || !(32.0..=4096.0).contains(&self.safe_near_meters)
            || !self.max_error_meters.is_finite() || !(0.0001..=0.25).contains(&self.max_error_meters)
        {
            return Err("网格碰撞LOD的安全近区须为32至4096米，世界误差须为0.0001至0.25米".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshLodPolicy {
    pub display: MeshLodMode<MeshDisplayLodSettings>,
    pub collision: MeshLodMode<MeshCollisionLodSettings>,
}

impl MeshLodPolicy {
    pub fn validate(self) -> Result<(), String> {
        if let MeshLodMode::Override(value) = self.display { value.validate()?; }
        if let MeshLodMode::Override(value) = self.collision { value.validate()?; }
        Ok(())
    }

    /// 全局总开关优先于实例覆盖；非法来源安全回退原始质量，不能开启坏简化。
    pub fn resolve_display(self, global: Option<MeshDisplayLodSettings>) -> Option<MeshDisplayLodSettings> {
        let global = global.filter(|value| value.validate().is_ok())?;
        match self.display {
            MeshLodMode::Inherit => Some(global),
            MeshLodMode::Disabled => None,
            MeshLodMode::Override(value) => value.validate().ok().map(|_| value),
        }
    }

    pub fn resolve_collision(self, global: Option<MeshCollisionLodSettings>) -> Option<MeshCollisionLodSettings> {
        let global = global.filter(|value| value.validate().is_ok())?;
        match self.collision {
            MeshLodMode::Inherit => Some(global),
            MeshLodMode::Disabled => None,
            MeshLodMode::Override(value) => value.validate().ok().map(|_| value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_collision_modes_resolve_independently_and_global_off_wins() {
        let display = MeshDisplayLodSettings { max_error_pixels: 2.0, ..Default::default() };
        let collision = MeshCollisionLodSettings { max_error_meters: 0.01, ..Default::default() };
        let mut policy = MeshLodPolicy::default();
        assert_eq!(policy.resolve_display(Some(display)), Some(display));
        assert_eq!(policy.resolve_collision(None), None);
        policy.display = MeshLodMode::Disabled;
        policy.collision = MeshLodMode::Override(collision);
        assert_eq!(policy.resolve_display(Some(display)), None);
        assert_eq!(policy.resolve_collision(Some(Default::default())), Some(collision));
        assert_eq!(policy.resolve_collision(None), None);
        policy.display = MeshLodMode::Override(display);
        assert_eq!(policy.resolve_display(None), None);
        policy.collision = MeshLodMode::Disabled;
        assert_eq!(policy.resolve_collision(Some(collision)), None);
    }

    #[test]
    fn malformed_policy_never_authorizes_simplification_and_strict_documents_reject_it() {
        for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0, 17.0] {
            let policy = MeshLodPolicy {
                display: MeshLodMode::Override(MeshDisplayLodSettings { max_error_pixels: bad, ..Default::default() }),
                ..Default::default()
            };
            assert!(policy.validate().is_err());
            assert_eq!(policy.resolve_display(Some(Default::default())), None);
        }
        let policy = MeshLodPolicy {
            collision: MeshLodMode::Override(MeshCollisionLodSettings { safe_near_meters: 0.0, ..Default::default() }),
            ..Default::default()
        };
        assert!(policy.validate().is_err());
        assert_eq!(policy.resolve_collision(Some(Default::default())), None);
        assert_eq!(MeshLodPolicy::default().resolve_display(Some(MeshDisplayLodSettings {
            hysteresis_ratio: f32::NAN, ..Default::default()
        })), None);
    }

    #[test]
    fn legacy_policy_defaults_and_explicit_overrides_round_trip_without_identity_changes() {
        assert_eq!(serde_json::from_str::<MeshLodPolicy>("{}").unwrap(), MeshLodPolicy::default());
        let policy = MeshLodPolicy {
            display: MeshLodMode::Override(MeshDisplayLodSettings { max_error_pixels: 0.75, hysteresis_ratio: 0.2 }),
            collision: MeshLodMode::Disabled,
        };
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(serde_json::from_str::<MeshLodPolicy>(&json).unwrap(), policy);
        assert!(serde_json::from_str::<MeshLodPolicy>(r#"{"display":{"mode":"unknown"}}"#).is_err());
        assert!(serde_json::from_str::<MeshLodPolicy>(r#"{"display":{"mode":"override","settings":{"max_error_pixels":1}}}"#).is_err());
    }
}
