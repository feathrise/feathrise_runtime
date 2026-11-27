//! 工程环境套装、场景来源和编辑器与运行时共用的有效环境解析。

use std::{collections::BTreeMap, sync::Arc};

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

use crate::{
    CelestialLightingProfile, SceneCelestialLightingOverrides, SceneEnvironmentDescriptor, SceneId,
    SceneSkyMode, SceneWeatherDescriptor, resolve,
};

/// 旧工程未建立库时可引用的隐式默认套装身份。
pub const DEFAULT_ENVIRONMENT_PROFILE_ID: &str = "default";

/// 场景省略来源时按旧方式解析，明确启用新来源后才改变选择语义。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "profile_id", rename_all = "snake_case")]
pub enum SceneEnvironmentSource {
    #[default]
    Legacy,
    InheritDefault,
    Shared(String),
    Local,
}

impl SceneEnvironmentSource {
    pub fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }
}

/// 方案的初始日历与天气控制。时钟在采用后正常推进，保存的方案值不跟随运行时间变化。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvironmentSimulation {
    pub hour: f32,
    pub month: u8,
    pub day: u8,
    pub paused: bool,
    /// 零为自动，一至五依次对应晴、阴、小雨、大雨和薄雾。
    pub weather_mode_index: u8,
    pub weather_transition: f32,
    /// 晴天草尖基准摆幅，单位为米。
    pub grass_wind_tip_strength: f32,
}

impl Default for EnvironmentSimulation {
    fn default() -> Self {
        Self {
            hour: 12.0, month: 6, day: 15, paused: false,
            weather_mode_index: 0, weather_transition: 1.0, grass_wind_tip_strength: 0.16,
        }
    }
}

impl EnvironmentSimulation {
    pub fn validate(&self) -> Result<(), String> {
        let days = match self.month { 2 => 28, 4 | 6 | 9 | 11 => 30, _ => 31 };
        if !self.hour.is_finite() || !(0.0..=24.0).contains(&self.hour)
            || !(1..=12).contains(&self.month) || !(1..=days).contains(&self.day)
        {
            return Err("环境时间必须使用有效月份、日期和零至二十四点".into());
        }
        if self.weather_mode_index > 5 || !self.weather_transition.is_finite()
            || !(0.0..=1.0).contains(&self.weather_transition)
        {
            return Err("环境天气模式或过渡进度无效".into());
        }
        // 与现有草顶点风路径共用三十五厘米安全摆幅。
        if !self.grass_wind_tip_strength.is_finite()
            || !(0.0..=0.35).contains(&self.grass_wind_tip_strength)
        {
            return Err("环境草尖摆幅必须在零至三十五厘米之间".into());
        }
        Ok(())
    }
}

/// 当前主场景已解析的模拟参数；编辑器偏好与方案数据在这里汇合后供正式消费者只读。
#[derive(Resource, Clone, Debug, Default)]
pub struct ActiveEnvironmentSimulation {
    pub configuration: Option<EnvironmentSimulation>,
    owner: Option<(SceneId, SceneEnvironmentSource)>,
    identity_revision: u64,
}

impl ActiveEnvironmentSimulation {
    pub fn publish(
        &mut self,
        scene_id: SceneId,
        source: SceneEnvironmentSource,
        configuration: Option<EnvironmentSimulation>,
    ) {
        let owner = (scene_id, source);
        if self.owner.as_ref() != Some(&owner) {
            self.owner = Some(owner);
            self.identity_revision = self.identity_revision.wrapping_add(1);
        }
        self.configuration = configuration;
    }

    pub fn clear(&mut self) {
        if self.owner.take().is_some() {
            self.identity_revision = self.identity_revision.wrapping_add(1);
        }
        self.configuration = None;
    }

    pub fn clear_for_scene(&mut self, scene_id: &SceneId) {
        if self.owner.as_ref().is_some_and(|(owner, _)| owner == scene_id) { self.clear(); }
    }

    pub const fn identity_revision(&self) -> u64 { self.identity_revision }
}

/// 可复用于多个场景的天空、日月、时间、天气与植被风摆参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentAppearance {
    pub sky_mode: SceneSkyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skybox_path: Option<String>,
    pub skybox_brightness: f32,
    pub environment_diffuse_intensity: f32,
    pub environment_specular_intensity: f32,
    pub skybox_yaw_degrees: f32,
    pub celestial_lighting: CelestialLightingProfile,
    #[serde(default)]
    pub simulation: EnvironmentSimulation,
    #[serde(default)]
    pub weather: SceneWeatherDescriptor,
}

impl Default for EnvironmentAppearance {
    fn default() -> Self {
        Self::from_environment(
            &SceneEnvironmentDescriptor::default(),
            CelestialLightingProfile::default(),
        )
    }
}

impl EnvironmentAppearance {
    fn from_environment(
        scene: &SceneEnvironmentDescriptor,
        celestial_lighting: CelestialLightingProfile,
    ) -> Self {
        Self {
            sky_mode: scene.sky_mode,
            skybox_path: scene.skybox_path.clone(),
            skybox_brightness: scene.skybox_brightness,
            environment_diffuse_intensity: scene.environment_diffuse_intensity,
            environment_specular_intensity: scene.environment_specular_intensity,
            skybox_yaw_degrees: scene.skybox_yaw_degrees,
            celestial_lighting,
            simulation: scene.simulation.unwrap_or_default(),
            weather: scene.weather,
        }
    }

    /// 将当前实际采用的视觉环境复制为可独立编辑的完整套装。
    pub fn from_resolved(resolved: &ResolvedSceneEnvironment) -> Self {
        Self::from_environment(&resolved.environment, resolved.celestial_lighting.clone())
    }

    /// 将整套环境写入场景，来源身份继续由场景选择管理。
    pub fn apply_to(&self, scene: &mut SceneEnvironmentDescriptor) {
        self.apply_sky(scene);
        scene.simulation = Some(self.simulation);
        scene.weather = self.weather;
        let celestial = &self.celestial_lighting;
        // 独立环境固定完整日月值，未来项目默认或共享套装变化不会渗入。
        scene.celestial_overrides = SceneCelestialLightingOverrides {
            day_base_illuminance_lux: Some(celestial.day_base_illuminance_lux),
            day_base_color_srgb: Some(celestial.day_base_color_srgb),
            night_peak_illuminance_lux: Some(celestial.night_peak_illuminance_lux),
            night_base_color_srgb: Some(celestial.night_base_color_srgb),
            azimuth_offset_degrees: Some(celestial.azimuth_offset_degrees),
            light_source_angular_diameter_degrees: Some(
                celestial.light_source_angular_diameter_degrees,
            ),
        };
    }

    fn apply_sky(&self, scene: &mut SceneEnvironmentDescriptor) {
        scene.sky_mode = self.sky_mode;
        scene.skybox_path.clone_from(&self.skybox_path);
        scene.skybox_brightness = self.skybox_brightness;
        scene.environment_diffuse_intensity = self.environment_diffuse_intensity;
        scene.environment_specular_intensity = self.environment_specular_intensity;
        scene.skybox_yaw_degrees = self.skybox_yaw_degrees;
    }

    /// 天空资源和日月参数沿用场景所用的校验规则。
    pub fn validate(&self) -> Result<(), String> {
        let mut scene = SceneEnvironmentDescriptor::default();
        self.apply_sky(&mut scene);
        scene.validate()?;
        self.simulation.validate()?;
        self.weather.validate()?;
        self.celestial_lighting.validate()
    }
}

/// 稳定身份由库键承载，套装改名不改变场景引用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldEnvironmentProfile {
    pub name: String,
    pub appearance: EnvironmentAppearance,
}

/// 完整环境库随 AssetServer 发布。共享索引避免热更采用时深拷贝所有套装。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldEnvironmentLibrary {
    pub default_profile_id: String,
    pub profiles: Arc<BTreeMap<String, WorldEnvironmentProfile>>,
}

impl PartialEq for WorldEnvironmentLibrary {
    fn eq(&self, other: &Self) -> bool {
        self.default_profile_id == other.default_profile_id
            && (Arc::ptr_eq(&self.profiles, &other.profiles) || self.profiles == other.profiles)
    }
}

impl WorldEnvironmentLibrary {
    pub fn is_empty(&self) -> bool {
        self.default_profile_id.is_empty() && self.profiles.is_empty()
    }

    /// 作者修改才分离索引，稳定帧和历史快照可共用未编辑版本。
    pub fn profiles_mut(&mut self) -> &mut BTreeMap<String, WorldEnvironmentProfile> {
        Arc::make_mut(&mut self.profiles)
    }

    /// 旧工程首次进入环境库时可编辑隐式默认，并沿用项目原有日月基准。
    pub fn with_legacy_default(mut self, legacy: &CelestialLightingProfile) -> Self {
        if self.is_empty() {
            let id = DEFAULT_ENVIRONMENT_PROFILE_ID.to_owned();
            self.default_profile_id.clone_from(&id);
            self.profiles_mut().insert(
                id,
                WorldEnvironmentProfile {
                    name: "默认环境".to_owned(),
                    appearance: EnvironmentAppearance {
                        celestial_lighting: legacy.clone(),
                        ..EnvironmentAppearance::default()
                    },
                },
            );
        }
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.is_empty() {
            return Ok(()); // 旧工程没有库时继续解析隐式默认。
        }
        validate_profile_id(&self.default_profile_id)?;
        if !self.profiles.contains_key(&self.default_profile_id) {
            return Err(format!("默认环境套装 {} 不存在", self.default_profile_id));
        }
        for (id, profile) in self.profiles.iter() {
            validate_profile_id(id)?;
            if profile.name.trim().is_empty() {
                return Err(format!("环境套装 {id} 的名称不能为空"));
            }
            profile
                .appearance
                .validate()
                .map_err(|error| format!("环境套装 {}：{error}", profile.name))?;
        }
        Ok(())
    }
}

pub(crate) fn validate_profile_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() || id.trim() != id || id.chars().any(char::is_control) {
        return Err("环境套装身份必须非空，且不能包含首尾空白或控制字符".to_owned());
    }
    Ok(())
}

/// 已解析的视觉环境、天体基准及引用信息。解析从不改写场景或全局资产。
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSceneEnvironment {
    pub environment: SceneEnvironmentDescriptor,
    pub celestial_lighting: CelestialLightingProfile,
    pub profile_id: Option<String>,
    /// 找不到明确引用时保留场景视觉托底，编辑器可报告缺失来源。
    pub missing_profile: Option<String>,
}

/// 离屏预览和正式运行时共用整套环境来源解析。
pub fn resolve_scene_environment(
    scene: &SceneEnvironmentDescriptor,
    legacy: &CelestialLightingProfile,
    library: &WorldEnvironmentLibrary,
) -> ResolvedSceneEnvironment {
    let mut environment = scene.clone();
    let selected = match &scene.source {
        SceneEnvironmentSource::InheritDefault => Some(if library.is_empty() {
            DEFAULT_ENVIRONMENT_PROFILE_ID
        } else {
            &library.default_profile_id
        }),
        SceneEnvironmentSource::Shared(id) => Some(id.as_str()),
        SceneEnvironmentSource::Legacy | SceneEnvironmentSource::Local => None,
    };
    if let Some(id) = selected {
        if let Some(profile) = library.profiles.get(id) {
            profile.appearance.apply_to(&mut environment);
            return ResolvedSceneEnvironment {
                environment,
                celestial_lighting: profile.appearance.celestial_lighting.clone().normalized(),
                profile_id: Some(id.to_owned()),
                missing_profile: None,
            };
        }
        if library.is_empty() && id == DEFAULT_ENVIRONMENT_PROFILE_ID {
            let appearance = EnvironmentAppearance {
                celestial_lighting: legacy.clone(),
                ..EnvironmentAppearance::default()
            };
            appearance.apply_to(&mut environment);
            return ResolvedSceneEnvironment {
                environment,
                celestial_lighting: legacy.clone().normalized(),
                profile_id: Some(id.to_owned()),
                missing_profile: None,
            };
        }
    }
    let celestial_lighting = if scene.source == SceneEnvironmentSource::Local {
        // 即使手写清单只填写部分日月值，独立环境也不再读取项目基准。
        resolve(
            &CelestialLightingProfile::default(),
            &scene.celestial_overrides,
        )
    } else {
        // 旧清单和失效引用使用原场景数据，在诊断中显式区分缺失身份。
        resolve(legacy, &scene.celestial_overrides)
    };
    ResolvedSceneEnvironment {
        environment,
        celestial_lighting,
        profile_id: None,
        missing_profile: selected.map(str::to_owned),
    }
}

/// 当前场景未保存的天空草稿。正式运行时按同一场景身份临时接管。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicSceneEnvironmentPreview {
    scene_id: Option<SceneId>,
    environment: Option<SceneEnvironmentDescriptor>,
    revision: u64,
}

impl DynamicSceneEnvironmentPreview {
    pub fn publish(&mut self, scene_id: SceneId, environment: SceneEnvironmentDescriptor) {
        if self.scene_id.as_ref() == Some(&scene_id)
            && self.environment.as_ref() == Some(&environment)
        {
            return;
        }
        self.scene_id = Some(scene_id);
        self.environment = Some(environment);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn environment_for(&self, scene_id: &SceneId) -> Option<&SceneEnvironmentDescriptor> {
        if self.scene_id.as_ref() == Some(scene_id) {
            self.environment.as_ref()
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        if self.scene_id.take().is_some() {
            self.environment = None;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorldEnvironmentAsset;

    fn add_profile(library: &mut WorldEnvironmentLibrary, id: &str, brightness: f32) {
        library.profiles_mut().insert(
            id.to_owned(),
            WorldEnvironmentProfile {
                name: format!("环境 {id}"),
                appearance: EnvironmentAppearance {
                    sky_mode: SceneSkyMode::Skybox,
                    skybox_path: Some(format!("textures/{id}.ktx2")),
                    skybox_brightness: brightness,
                    celestial_lighting: CelestialLightingProfile {
                        day_base_illuminance_lux: brightness * 10.0,
                        ..CelestialLightingProfile::default()
                    },
                    ..EnvironmentAppearance::default()
                },
            },
        );
    }

    #[test]
    fn legacy_files_keep_scene_sky_and_per_field_celestial_inheritance() {
        let mut project = WorldEnvironmentAsset::default();
        project.celestial_lighting.night_peak_illuminance_lux = 625.0;
        let json = serde_json::to_value(&project).unwrap();
        assert!(json.get("library").is_none());
        let restored: WorldEnvironmentAsset = serde_json::from_value(json).unwrap();
        assert!(restored.library.is_empty());
        let scene: SceneEnvironmentDescriptor = serde_json::from_value(serde_json::json!({
            "skybox_path": "textures/old.ktx2",
            "environment_light_intensity": 380.0,
            "celestial_overrides": {"day_base_illuminance_lux": 1234.0}
        }))
        .unwrap();
        let current =
            resolve_scene_environment(&scene, &restored.celestial_lighting, &restored.library);
        assert_eq!(scene.source, SceneEnvironmentSource::Legacy);
        assert_eq!(scene.simulation, None);
        assert_eq!(
            current.environment.skybox_path.as_deref(),
            Some("textures/old.ktx2")
        );
        assert_eq!(current.environment.environment_specular_intensity, 380.0);
        assert_eq!(current.celestial_lighting.day_base_illuminance_lux, 1234.0);
        assert_eq!(current.celestial_lighting.night_peak_illuminance_lux, 625.0);
        let scene_json = serde_json::to_value(&scene).unwrap();
        assert!(scene_json.get("source").is_none());
        assert!(scene_json.get("local_environment_initialized").is_none());

        let new_scene = SceneEnvironmentDescriptor::inheriting_default();
        let implicit =
            resolve_scene_environment(&new_scene, &restored.celestial_lighting, &restored.library);
        assert_eq!(
            implicit.profile_id.as_deref(),
            Some(DEFAULT_ENVIRONMENT_PROFILE_ID)
        );
        assert_eq!(
            implicit.celestial_lighting.night_peak_illuminance_lux,
            625.0
        );
        assert_eq!(implicit.environment.sky_mode, SceneSkyMode::Dynamic);
    }

    #[test]
    fn default_switch_rename_and_local_copy_keep_distinct_sources() {
        let mut project = WorldEnvironmentAsset::default();
        let base = &project.celestial_lighting;
        add_profile(&mut project.library, "dawn", 350.0);
        add_profile(&mut project.library, "night", 95.0);
        project.library.profiles_mut().get_mut("dawn").unwrap().appearance.simulation =
            EnvironmentSimulation { hour: 6.0, month: 9, paused: true,
                weather_mode_index: 3, weather_transition: 0.5, grass_wind_tip_strength: 0.3, ..Default::default() };
        project.library.profiles_mut().get_mut("night").unwrap().appearance.simulation.hour = 22.0;
        project.library.default_profile_id = "dawn".to_owned();
        let inherited = SceneEnvironmentDescriptor::inheriting_default();
        let pinned = SceneEnvironmentDescriptor {
            source: SceneEnvironmentSource::Shared("dawn".into()),
            ..SceneEnvironmentDescriptor::default()
        };
        let from_pinned = resolve_scene_environment(&pinned, base, &project.library);
        let mut local = pinned.clone();
        EnvironmentAppearance::from_resolved(&from_pinned).apply_to(&mut local);
        local.source = SceneEnvironmentSource::Local;
        local.local_environment_initialized = true;

        let shared = project.library.clone();
        assert!(Arc::ptr_eq(&shared.profiles, &project.library.profiles));
        project.library.default_profile_id = "night".to_owned();
        let profile = project.library.profiles_mut().get_mut("dawn").unwrap();
        profile.name = "改名后的早晨".into();
        profile.appearance.skybox_brightness = 640.0;
        profile
            .appearance
            .celestial_lighting
            .day_base_illuminance_lux = 6400.0;
        assert_eq!(shared.profiles["dawn"].appearance.skybox_brightness, 350.0);
        let resolved_default = resolve_scene_environment(&inherited, base, &project.library);
        let resolved_pinned = resolve_scene_environment(&pinned, base, &project.library);
        let resolved_local = resolve_scene_environment(&local, base, &project.library);
        assert_eq!(resolved_default.profile_id.as_deref(), Some("night"));
        assert_eq!(resolved_default.environment.skybox_brightness, 95.0);
        assert_eq!(resolved_pinned.profile_id.as_deref(), Some("dawn"));
        assert_eq!(resolved_pinned.environment.skybox_brightness, 640.0);
        assert_eq!(
            resolved_pinned.celestial_lighting.day_base_illuminance_lux,
            6400.0
        );
        assert_eq!(resolved_local.environment.skybox_brightness, 350.0);
        assert_eq!(resolved_default.environment.simulation.unwrap().hour, 22.0);
        assert_eq!(resolved_pinned.environment.simulation.unwrap().hour, 6.0);
        assert_eq!(resolved_local.environment.simulation, from_pinned.environment.simulation);
        assert_eq!(
            resolved_local.celestial_lighting.day_base_illuminance_lux,
            3500.0
        );
        assert_eq!(resolved_default.environment.weather, inherited.weather);
        assert!(project.validate().is_ok());
        let encoded = serde_json::to_string(&project).unwrap();
        assert_eq!(
            serde_json::from_str::<WorldEnvironmentAsset>(&encoded).unwrap(),
            project
        );
        let binding = serde_json::to_value(&pinned).unwrap();
        assert_eq!(
            binding["source"],
            serde_json::json!({"mode":"shared","profile_id":"dawn"})
        );
        let encoded = serde_json::to_string(&local).unwrap();
        assert_eq!(
            serde_json::from_str::<SceneEnvironmentDescriptor>(&encoded).unwrap(),
            local
        );
    }

    #[test]
    fn missing_named_profile_uses_stored_scene_appearance_and_reports_id() {
        let legacy = CelestialLightingProfile::default();
        let mut library = WorldEnvironmentLibrary::default().with_legacy_default(&legacy);
        let scene = SceneEnvironmentDescriptor {
            source: SceneEnvironmentSource::Shared("deleted".into()),
            skybox_brightness: 160.0,
            ..SceneEnvironmentDescriptor::default()
        };
        let resolved = resolve_scene_environment(&scene, &legacy, &library);
        assert_eq!(resolved.profile_id, None);
        assert_eq!(resolved.missing_profile.as_deref(), Some("deleted"));
        assert_eq!(resolved.environment.skybox_brightness, 160.0);

        library.default_profile_id = "deleted".into();
        assert!(library.validate().is_err());
    }

    #[test]
    fn preview_revision_changes_only_when_a_scene_snapshot_changes() {
        let scene = SceneId::parse("SCN-a").unwrap();
        let other = SceneId::parse("SCN-b").unwrap();
        let descriptor = SceneEnvironmentDescriptor::inheriting_default();
        let mut preview = DynamicSceneEnvironmentPreview::default();
        preview.publish(scene.clone(), descriptor.clone());
        assert_eq!(preview.environment_for(&scene), Some(&descriptor));
        assert_eq!(preview.environment_for(&other), None);
        let revision = preview.revision();
        preview.publish(scene.clone(), descriptor);
        assert_eq!(preview.revision(), revision);
        preview.clear();
        assert_ne!(preview.revision(), revision);
        assert_eq!(preview.environment_for(&scene), None);
    }

    #[test]
    fn simulation_compatibility_and_validation_keep_saved_configuration_finite() {
        let mut value = serde_json::to_value(EnvironmentAppearance::default()).unwrap();
        value.as_object_mut().unwrap().remove("simulation");
        value.as_object_mut().unwrap().remove("weather");
        let restored: EnvironmentAppearance = serde_json::from_value(value).unwrap();
        assert_eq!(restored.simulation, EnvironmentSimulation::default());
        assert!(restored.validate().is_ok());
        for invalid in [EnvironmentSimulation { hour: f32::NAN, ..Default::default() },
            EnvironmentSimulation { month: 2, day: 31, ..Default::default() },
            EnvironmentSimulation { weather_mode_index: 6, ..Default::default() },
            EnvironmentSimulation { weather_transition: 1.1, ..Default::default() },
            EnvironmentSimulation { grass_wind_tip_strength: 0.36, ..Default::default() }]
        {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn simulation_owner_ignores_old_scene_cleanup_and_stable_publication() {
        let scene = SceneId::parse("SCN-current").unwrap();
        let other = SceneId::parse("SCN-old").unwrap();
        let mut active = ActiveEnvironmentSimulation::default();
        active.publish(scene.clone(), SceneEnvironmentSource::Shared("a".into()), Some(Default::default()));
        let revision = active.identity_revision();
        active.publish(scene.clone(), SceneEnvironmentSource::Shared("a".into()), Some(Default::default()));
        active.clear_for_scene(&other);
        assert_eq!(active.identity_revision(), revision);
        assert!(active.configuration.is_some());
        active.publish(scene.clone(), SceneEnvironmentSource::Shared("b".into()), Some(Default::default()));
        assert_ne!(active.identity_revision(), revision);
        active.clear_for_scene(&scene);
        assert_eq!(active.configuration, None);
    }
}
