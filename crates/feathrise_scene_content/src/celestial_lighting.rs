//! 全局天体光照基础值、动态场景逐字段覆盖与纯解析规则。

use bevy::prelude::*; // Asset、Resource、Handle 与 TypePath 构成热加载全局配置入口。
use bevy::reflect::TypePath; // 自定义世界环境资产需要稳定类型路径。
use serde::{Deserialize, Serialize}; // 全局文件与场景清单共享可读 JSON 格式。

/// 世界环境全局配置文件的当前格式版本。
pub const WORLD_ENVIRONMENT_SCHEMA_VERSION: u32 = 1; // 首版只包含动态场景天体光照基础设置。
/// 编译期日间方向光基准照度，单位为 lux。
pub const DEFAULT_DAY_BASE_ILLUMINANCE_LUX: f32 = 9_000.0; // 保持当前动态场景正午画面。
/// 编译期日间方向光基准 sRGB 颜色。
pub const DEFAULT_DAY_BASE_COLOR_SRGB: [f32; 3] = [1.0, 0.93, 0.82]; // 保持当前暖白日光。
/// 编译期动态场景月亮方向光午夜峰值，单位为 lux。
pub const DEFAULT_NIGHT_PEAK_ILLUMINANCE_LUX: f32 = 1_000.0; // 月亮从月升零值连续达到午夜一千 lux；JSON 字段保持兼容。
/// 编译期夜间方向光基准 sRGB 颜色。
pub const DEFAULT_NIGHT_BASE_COLOR_SRGB: [f32; 3] = [0.42, 0.52, 0.78]; // 保持既有冷蓝月光。
/// 编译期太阳与月亮共同使用的水平方位角偏移，单位为度。
pub const DEFAULT_CELESTIAL_AZIMUTH_OFFSET_DEGREES: f32 = 0.0; // 默认不旋转世界本地天体轨迹。
/// 编译期方向光光源角直径，单位为度。
pub const DEFAULT_LIGHT_SOURCE_ANGULAR_DIAMETER_DEGREES: f32 = 0.533; // 地球观察到的太阳平均角直径。
/// F12 允许编辑的最小光源角直径，单位为度。
pub const MIN_EDITABLE_LIGHT_SOURCE_ANGULAR_DIAMETER_DEGREES: f32 = 0.05; // 避免零尺寸硬阴影与数值退化。
/// F12 允许编辑的最大光源角直径，单位为度。
pub const MAX_EDITABLE_LIGHT_SOURCE_ANGULAR_DIAMETER_DEGREES: f32 = 5.0; // 给艺术软阴影留出范围并限制极端输入。

/// 动态场景使用的完整太阳与月光基础配置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CelestialLightingProfile {
    /// 日间太阳曲线达到一时的方向光照度，单位为 lux。
    pub day_base_illuminance_lux: f32,
    /// 高角度日光的三个零至一 sRGB 通道。
    pub day_base_color_srgb: [f32; 3],
    /// 动态场景月亮方向光在午夜达到的峰值照度，单位为 lux。
    pub night_peak_illuminance_lux: f32,
    /// 夜间月光的三个零至一 sRGB 通道。
    pub night_base_color_srgb: [f32; 3],
    /// 绕世界 Y 轴同时施加到太阳与月亮的水平角偏移。
    pub azimuth_offset_degrees: f32,
    /// 写入 Bevy `SunDisk` 并供 Solari 读取的光源角直径。
    pub light_source_angular_diameter_degrees: f32,
}

impl Default for CelestialLightingProfile {
    fn default() -> Self {
        Self {
            day_base_illuminance_lux: DEFAULT_DAY_BASE_ILLUMINANCE_LUX, // 缺失全局文件时保持当前动态场景亮度。
            day_base_color_srgb: DEFAULT_DAY_BASE_COLOR_SRGB, // 缺失全局文件时保持当前暖白色。
            night_peak_illuminance_lux: DEFAULT_NIGHT_PEAK_ILLUMINANCE_LUX, // 动态场景月亮午夜峰值默认采用一千 lux。
            night_base_color_srgb: DEFAULT_NIGHT_BASE_COLOR_SRGB, // 旧默认冷蓝月光继续可用。
            azimuth_offset_degrees: DEFAULT_CELESTIAL_AZIMUTH_OFFSET_DEGREES, // 不改变既有太阳路径方向。
            light_source_angular_diameter_degrees: DEFAULT_LIGHT_SOURCE_ANGULAR_DIAMETER_DEGREES, // 保留地球太阳软阴影基准。
        }
    }
}

impl CelestialLightingProfile {
    /// 校验所有数值可以安全进入方向光、颜色与 Solari 参数。
    pub fn validate(&self) -> Result<(), String> {
        validate_non_negative_finite(self.day_base_illuminance_lux, "日间基准照度")?;
        validate_srgb(self.day_base_color_srgb, "日间基准颜色")?;
        validate_non_negative_finite(self.night_peak_illuminance_lux, "月亮午夜峰值照度")?;
        validate_srgb(self.night_base_color_srgb, "夜间基准颜色")?;
        if !self.azimuth_offset_degrees.is_finite() {
            return Err("天体方位角偏移必须是有限数".to_owned()); // 非有限角会生成损坏四元数。
        }
        if !self.light_source_angular_diameter_degrees.is_finite()
            || self.light_source_angular_diameter_degrees <= 0.0
        {
            return Err("光源角直径必须是有限正数".to_owned()); // 零、负数或非有限值不能成为光源范围。
        }
        Ok(()) // 完整配置可以进入运行时解析。
    }

    /// 返回方位角已规范到零至三百六十度的等价配置。
    pub fn normalized(mut self) -> Self {
        self.azimuth_offset_degrees = self.azimuth_offset_degrees.rem_euclid(360.0); // 保存可接受负角，运行时只使用稳定等价角。
        self
    }

    /// 返回日间基准 Bevy sRGB 颜色。
    pub fn day_color(&self) -> Color {
        Color::srgb(
            self.day_base_color_srgb[0], // 红通道按作者零至一数值读取。
            self.day_base_color_srgb[1], // 绿通道按作者零至一数值读取。
            self.day_base_color_srgb[2], // 蓝通道按作者零至一数值读取。
        )
    }

    /// 返回夜间基准 Bevy sRGB 颜色。
    pub fn night_color(&self) -> Color {
        Color::srgb(
            self.night_base_color_srgb[0], // 红通道按作者零至一数值读取。
            self.night_base_color_srgb[1], // 绿通道按作者零至一数值读取。
            self.night_base_color_srgb[2], // 蓝通道按作者零至一数值读取。
        )
    }
}

/// 动态场景对全局天体光照的逐属性可选覆盖。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneCelestialLightingOverrides {
    /// `None` 表示跟随全局日间照度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_base_illuminance_lux: Option<f32>,
    /// `None` 表示跟随全局日间颜色；三个通道作为一个属性共同继承。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_base_color_srgb: Option<[f32; 3]>,
    /// `None` 表示跟随全局月亮午夜峰值照度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub night_peak_illuminance_lux: Option<f32>,
    /// `None` 表示跟随全局夜间颜色；三个通道作为一个属性共同继承。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub night_base_color_srgb: Option<[f32; 3]>,
    /// `None` 表示跟随全局天体方位角偏移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_offset_degrees: Option<f32>,
    /// `None` 表示跟随全局光源角直径。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_source_angular_diameter_degrees: Option<f32>,
}

impl SceneCelestialLightingOverrides {
    /// 返回当前场景是否完全跟随全局配置。
    pub fn is_empty(&self) -> bool {
        self.day_base_illuminance_lux.is_none()
            && self.day_base_color_srgb.is_none()
            && self.night_peak_illuminance_lux.is_none()
            && self.night_base_color_srgb.is_none()
            && self.azimuth_offset_degrees.is_none()
            && self.light_source_angular_diameter_degrees.is_none()
    }

    /// 校验所有已经声明的覆盖值；未声明字段由全局配置负责校验。
    pub fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.day_base_illuminance_lux {
            validate_non_negative_finite(value, "场景日间基准照度")?;
        }
        if let Some(value) = self.day_base_color_srgb {
            validate_srgb(value, "场景日间基准颜色")?;
        }
        if let Some(value) = self.night_peak_illuminance_lux {
            validate_non_negative_finite(value, "场景月亮午夜峰值照度")?;
        }
        if let Some(value) = self.night_base_color_srgb {
            validate_srgb(value, "场景夜间基准颜色")?;
        }
        if self
            .azimuth_offset_degrees
            .is_some_and(|value| !value.is_finite())
        {
            return Err("场景天体方位角偏移必须是有限数".to_owned()); // 损坏覆盖不得污染最终变换。
        }
        if self
            .light_source_angular_diameter_degrees
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            return Err("场景光源角直径必须是有限正数".to_owned()); // 覆盖值遵循与全局相同的运行时约束。
        }
        Ok(()) // 所有存在的字段均可安全覆盖全局值。
    }
}

/// 按“全局配置 → 当前场景逐字段覆盖”生成动态场景最终天体光照配置。
pub fn resolve(
    global: &CelestialLightingProfile,
    overrides: &SceneCelestialLightingOverrides,
) -> CelestialLightingProfile {
    CelestialLightingProfile {
        day_base_illuminance_lux: overrides
            .day_base_illuminance_lux
            .unwrap_or(global.day_base_illuminance_lux), // 单字段覆盖不冻结其他全局属性。
        day_base_color_srgb: overrides
            .day_base_color_srgb
            .unwrap_or(global.day_base_color_srgb), // 日间 RGB 三通道共同继承或共同覆盖。
        night_peak_illuminance_lux: overrides
            .night_peak_illuminance_lux
            .unwrap_or(global.night_peak_illuminance_lux), // 夜间照度不再从日间百分比派生。
        night_base_color_srgb: overrides
            .night_base_color_srgb
            .unwrap_or(global.night_base_color_srgb), // 月光颜色具有独立作者基准。
        azimuth_offset_degrees: overrides
            .azimuth_offset_degrees
            .unwrap_or(global.azimuth_offset_degrees), // 太阳与月亮共享同一水平偏移。
        light_source_angular_diameter_degrees: overrides
            .light_source_angular_diameter_degrees
            .unwrap_or(global.light_source_angular_diameter_degrees), // 光栅 SunDisk 与 Solari 共用最终值。
    }
    .normalized() // 只有方位角需要规范化，其他作者值保持精确。
}

/// `assets/config` 中唯一的世界环境作者资产。
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldEnvironmentAsset {
    /// 全局文件格式版本。
    pub schema_version: u32,
    /// 动态场景默认使用的完整天体光照配置。
    pub celestial_lighting: CelestialLightingProfile,
}

impl Default for WorldEnvironmentAsset {
    fn default() -> Self {
        Self {
            schema_version: WORLD_ENVIRONMENT_SCHEMA_VERSION, // 编译期回退也使用当前可保存版本。
            celestial_lighting: CelestialLightingProfile::default(), // 完整默认值保证缺失文件不会使场景变黑。
        }
    }
}

impl WorldEnvironmentAsset {
    /// 校验格式版本与完整天体配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != WORLD_ENVIRONMENT_SCHEMA_VERSION {
            return Err(format!(
                "世界环境配置版本{}与运行时版本{}不一致",
                self.schema_version, WORLD_ENVIRONMENT_SCHEMA_VERSION
            )); // 不兼容版本失败关闭并使用编译期默认值。
        }
        self.celestial_lighting.validate() // 数值合法后才能原子采用完整配置。
    }
}

/// 世界环境资产当前有效配置、热加载修订与可见错误。
#[derive(Resource, Clone, Debug)]
pub struct WorldEnvironmentRegistry {
    /// 启动时固定持有的全局资产强句柄。
    pub handle: Handle<WorldEnvironmentAsset>,
    /// 当前安全有效的动态场景天体配置；失败时始终为编译期默认值。
    pub celestial_lighting: CelestialLightingProfile,
    /// 每次采用成功配置或失败回退时递增，消费者据此执行增量同步。
    pub revision: u64,
    /// 最近一次全局文件加载或校验失败原因，供 F12 直接显示。
    pub last_error: Option<String>,
}

/// F12 作者草稿对当前动态场景天体光照的纯内存预览；关闭界面后脏草稿仍可继续接管。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicCelestialLightingPreview {
    scene_id: Option<crate::SceneId>, // 预览只能命中同一稳定动态场景，切换场景不得串用旧草稿。
    profile: Option<CelestialLightingProfile>, // 已按全局草稿与场景覆盖解析的完整有效快照。
    revision: u64,                // 发布或清除时递增，运行时可按身份增量采用。
}

impl DynamicCelestialLightingPreview {
    /// 发布一个已经校验并解析的当前场景预览快照。
    pub fn publish(&mut self, scene_id: crate::SceneId, profile: CelestialLightingProfile) {
        let profile = profile.normalized(); // 进入共享资源前统一方位角表示。
        if self.scene_id.as_ref() == Some(&scene_id) && self.profile.as_ref() == Some(&profile) {
            return; // 稳定 Presentation 帧不重复触发动态光照写入。
        }
        self.scene_id = Some(scene_id); // 冻结当前草稿场景身份。
        self.profile = Some(profile); // 只发布完整快照，不暴露半组 RGB 通道。
        self.revision = self.revision.wrapping_add(1); // 消费者可拒收旧帧结果。
    }

    /// 没有适用的未保存作者权威、无草稿或切到固定场景时归还正式资产配置。
    pub fn clear(&mut self) {
        if self.scene_id.is_none() && self.profile.is_none() {
            return; // 已清除时保持资源变更检测安静。
        }
        self.scene_id = None; // 释放旧场景身份。
        self.profile = None; // 下一帧由全局资产与正式清单重新解析。
        self.revision = self.revision.wrapping_add(1); // 清除本身也要求运行时重新采用正式配置。
    }

    /// 返回只属于指定动态场景的完整预览配置。
    pub fn profile_for(&self, scene_id: &crate::SceneId) -> Option<&CelestialLightingProfile> {
        (self.scene_id.as_ref() == Some(scene_id))
            .then_some(self.profile.as_ref())
            .flatten() // 场景身份不一致时绝不泄漏未保存草稿。
    }

    /// 返回当前预览身份修订。
    pub const fn revision(&self) -> u64 {
        self.revision // 只读诊断不暴露内部可变字段。
    }
}

impl WorldEnvironmentRegistry {
    /// 使用已经请求的强句柄建立安全默认注册表。
    pub(crate) fn with_handle(handle: Handle<WorldEnvironmentAsset>) -> Self {
        Self {
            handle,                                                  // 强句柄保持 AssetServer 热更新订阅。
            celestial_lighting: CelestialLightingProfile::default(), // 首次资产完成前立即提供安全画面。
            revision: 0,      // 首次成功或失败事件后再发布正式修订。
            last_error: None, // 尚未收到失败事件时不显示误导诊断。
        }
    }
}

fn validate_non_negative_finite(value: f32, label: &str) -> Result<(), String> {
    if value.is_finite() && value >= 0.0 {
        Ok(()) // 零照度是合法艺术选择，负数和非有限值才拒绝。
    } else {
        Err(format!("{label}必须是有限非负数")) // 错误直接带属性名供 F12 显示。
    }
}

fn validate_srgb(value: [f32; 3], label: &str) -> Result<(), String> {
    if value
        .into_iter()
        .all(|channel| channel.is_finite() && (0.0..=1.0).contains(&channel))
    {
        Ok(()) // 三个通道均可直接构造 Bevy sRGB 颜色。
    } else {
        Err(format!("{label}的 RGB 通道必须是 0～1 的有限数")) // 任一损坏通道拒绝整组颜色。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 纯数据测试不启动 Bevy 渲染器。

    #[test]
    fn global_profile_round_trips_with_stable_defaults() {
        let asset = WorldEnvironmentAsset::default(); // 使用编译期安全默认值构造首版文件。
        let json = serde_json::to_string_pretty(&asset).expect("全局世界环境必须可序列化");
        let decoded: WorldEnvironmentAsset =
            serde_json::from_str(&json).expect("全局世界环境必须可反序列化");
        assert_eq!(decoded, asset); // 六组属性和版本都必须无损往返。
        assert!(decoded.validate().is_ok()); // 编译期默认值本身必须始终合法。
        assert_eq!(
            decoded.celestial_lighting.night_peak_illuminance_lux,
            1_000.0
        ); // 动态场景默认午夜照度必须保持本次确定的一千 lux。
    }

    #[test]
    fn invalid_global_values_are_rejected_before_runtime_adoption() {
        let mut asset = WorldEnvironmentAsset::default(); // 从合法值逐项制造损坏输入。
        asset.celestial_lighting.day_base_color_srgb[1] = 1.1; // sRGB 通道越界。
        assert!(asset.validate().is_err());
        asset = WorldEnvironmentAsset::default();
        asset.celestial_lighting.night_peak_illuminance_lux = f32::NAN; // 非有限照度。
        assert!(asset.validate().is_err());
        asset = WorldEnvironmentAsset::default();
        asset
            .celestial_lighting
            .light_source_angular_diameter_degrees = 0.0; // 零角直径退化。
        assert!(asset.validate().is_err());
    }

    #[test]
    fn a_single_scene_override_is_isolated_and_following_resynchronizes() {
        let mut global = CelestialLightingProfile::default(); // 模拟初始全局作者值。
        let mut overrides = SceneCelestialLightingOverrides {
            day_base_illuminance_lux: Some(12_000.0), // 只固定日间照度。
            ..default()
        };
        global.night_peak_illuminance_lux = 480.0; // 全局其他字段变化仍应进入场景。
        let resolved = resolve(&global, &overrides);
        assert_eq!(resolved.day_base_illuminance_lux, 12_000.0); // 覆盖字段不受全局同字段变化影响。
        assert_eq!(resolved.night_peak_illuminance_lux, 480.0); // 未覆盖字段实时跟随全局。
        overrides.day_base_illuminance_lux = None; // 恢复跟随后删除场景字段。
        global.day_base_illuminance_lux = 15_000.0;
        assert_eq!(
            resolve(&global, &overrides).day_base_illuminance_lux,
            15_000.0
        ); // 重新同步最新全局值。
    }

    #[test]
    fn resolution_keeps_independent_day_night_color_direction_and_size() {
        let global = CelestialLightingProfile::default(); // 其他属性继续从全局继承。
        let overrides = SceneCelestialLightingOverrides {
            night_peak_illuminance_lux: Some(72.0),       // 独立夜间照度。
            night_base_color_srgb: Some([0.2, 0.3, 0.4]), // 独立月光颜色。
            azimuth_offset_degrees: Some(-15.0),          // 负角应规范为等价正角。
            light_source_angular_diameter_degrees: Some(1.25), // 独立软阴影范围。
            ..default()
        };
        let resolved = resolve(&global, &overrides);
        assert_eq!(resolved.day_base_illuminance_lux, 9_000.0);
        assert_eq!(resolved.night_peak_illuminance_lux, 72.0);
        assert_eq!(resolved.night_base_color_srgb, [0.2, 0.3, 0.4]);
        assert_eq!(resolved.azimuth_offset_degrees, 345.0);
        assert_eq!(resolved.light_source_angular_diameter_degrees, 1.25);
    }
}
