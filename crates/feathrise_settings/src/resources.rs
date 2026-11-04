//! 共享渲染输入及后端状态；不读任何应用的配置文件。

mod dlss; // DLSS/Solari 正式运行状态与设置迁移阶段独立维护。
mod fsr; // FSR 3候选结构化状态与相机租约索引。

#[cfg(feature = "dlss")] // SR 管理器内部共享销毁清理索引，不形成公开 API。
pub(crate) use dlss::DlssSuperResolutionLeaseIndex;
pub use dlss::{DlssRuntimeBlocker, DlssRuntimePath, DlssRuntimeStatus, RaytracingSceneCoverage}; // 菜单、光追集成与发布烟雾共用同一结构化状态契约。
#[cfg(feature = "fsr3_upscaling")]
pub(crate) use fsr::Fsr3LeaseIndex; // 候选管理器内部回收销毁相机票据。
pub use fsr::{Fsr3RuntimeBlocker, Fsr3RuntimeStatus}; // 菜单和诊断始终能解释FSR请求回退。

use bevy::prelude::*; // Resource 等 ECS 常用类型

use crate::types::GraphicsSettings;

/// 玩家显示页允许使用的最低曝光补偿，单位为 EV（档）。
pub const MIN_EXPOSURE_COMPENSATION: f32 = -1.0; // 限制过暗设置，避免用户把场景调到难以辨认。
/// 玩家显示页允许使用的最高曝光补偿，单位为 EV（档）。
pub const MAX_EXPOSURE_COMPENSATION: f32 = 1.0; // 限制高光过曝，同时提供清晰可见的亮度调节范围。
/// 保持场景作者当前画面的中性曝光补偿。
pub const DEFAULT_EXPOSURE_COMPENSATION: f32 = 0.0; // 零档不会改变任何窗口相机的作者曝光。
/// 显示页输入完成阶段；相机系统在该集合后读取同帧 FOV 选择。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplaySettingsInputSet; // 避免菜单插件与相机插件通过具体函数路径产生循环依赖。

/// 将外部配置或 UI 写入的曝光补偿规范化到公开支持范围。
pub fn sanitize_exposure_compensation(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_EXPOSURE_COMPENSATION, MAX_EXPOSURE_COMPENSATION) // 合法数值只做范围钳制。
    } else {
        DEFAULT_EXPOSURE_COMPENSATION // 非有限值安全回退中性画面，避免污染渲染 uniform。
    }
}

/// 应用同步显式运行输入的排序锚点；菜单操作先结算，相机等消费者随后读取。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeSettingsSyncSet;

/// 渲染系统实际消费的请求，不含游戏控制、语言、作弊或作者工具偏好。
#[derive(Resource, Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    pub mesh_lod: crate::MeshLodSettings,
    pub graphics: GraphicsSettings,
    pub fullscreen: bool,
    pub show_fps: bool,
    pub max_fps: u32,
    pub exposure_compensation: f32,
}
impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            mesh_lod: Default::default(),
            graphics: Default::default(),
            fullscreen: false,
            show_fps: true,
            max_fps: 0,
            exposure_compensation: DEFAULT_EXPOSURE_COMPENSATION,
        }
    }
}

/// 场景可视化输入只改变显示；规划、辅助空气墙与物理调试不改变玩法碰撞。
#[derive(Resource, Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SceneDisplaySettings {
    pub show_map_planning: bool,
    pub air_wall_display_mode: crate::AirWallDisplayMode,
    pub show_physics_debug: bool,
    pub terrain_backfaces: bool,
}
impl Default for SceneDisplaySettings {
    fn default() -> Self {
        Self {
            show_map_planning: false,
            air_wall_display_mode: Default::default(),
            show_physics_debug: false,
            terrain_backfaces: true,
        }
    }
}

/// 环境预览的显式覆盖；默认保持当前作者光照、正常时钟与草尖基线。
#[derive(Resource, Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EnvironmentPreviewSettings {
    pub world_time_paused: bool,
    pub directional_light_brightness_multiplier: f32,
    pub grass_wind_tip_strength: f32,
}
impl Default for EnvironmentPreviewSettings {
    fn default() -> Self {
        Self {
            world_time_paused: false,
            directional_light_brightness_multiplier: 1.0,
            grass_wind_tip_strength: 0.16,
        }
    }
}

/// 创建窗口/交换链前由所属应用提供的最小显示输入；本类型不负责读取文件。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartupDisplayPreferences {
    pub fullscreen: bool,
    pub vsync: bool,
    pub frame_generation_mode: crate::FrameGenerationMode,
}
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartupWindowPolicy {
    pub force_windowed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_inputs_start_from_authored_baseline() {
        assert_eq!(RenderSettings::default().exposure_compensation, 0.0);
        assert!(!SceneDisplaySettings::default().show_map_planning);
        assert_eq!(
            EnvironmentPreviewSettings::default().directional_light_brightness_multiplier,
            1.0
        );
        assert!(!EnvironmentPreviewSettings::default().world_time_paused);
        assert_eq!(
            EnvironmentPreviewSettings::default().grass_wind_tip_strength,
            0.16
        );
    }
    #[test]
    fn exposure_sanitizes_without_changing_valid_values() {
        for (input, expected) in [(-4.0, -1.0), (3.0, 1.0), (f32::NAN, 0.0), (0.6, 0.6)] {
            assert_eq!(sanitize_exposure_compensation(input), expected);
        }
    }
}
