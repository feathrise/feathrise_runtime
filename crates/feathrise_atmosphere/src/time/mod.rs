// atmosphere::time：统一管理环境月份日期、世界本地天体弧线、月光与玩法场景环境亮度。

mod celestial_visuals; // 动态地图可见太阳和月亮圆盘、相机跟随与独立昼夜轨迹。
mod clock; // 日期推进、新游戏重置与存档恢复功能域。
pub mod components; // 玩法方向光与环境 IBL 的显式 ECS 标记。
pub mod constants; // 时间倍率、固定昼夜边界和光照基准。
mod dynamic_sky; // 动态模式接管玩法透视相机；天空盒真正就绪时主动让出可见背景。
mod lighting; // 本地天体弧线、年度倾斜与统一光照写入功能域。
pub mod resources; // 全局世界时钟资源。
pub mod types; // 存档快照与跨 crate 调度集。

use bevy::prelude::*; // App、Plugin、Update 与 IntoScheduleConfigs 用于注册世界时间系统。
use feathrise_scene_core::input::GamePauseSystemSet; // 世界推进必须等待本帧所有暂停来源应用完成。
use feathrise_settings::RuntimeSettingsSyncSet; // 应用/作者输入先映射成共享环境请求，再推进本帧时钟。
use feathrise_scene_core::state::in_active_scene; // 主菜单和加载场景不推进时钟，也不接管其光照。

pub(crate) use components::WorldEnvironmentMapProfile; // 天空状态机与最终光照系统共享内部IBL补光和反射基准。
pub use components::{WorldCelestialLight, WorldCelestialLightKind, WorldSkyProfile}; // 场景与光追共享天体身份；天空盒模式复用亮度基准标记。
pub use lighting::dynamic_day_night_factor; // 编辑器与诊断界面复用运行时唯一昼夜曲线。
pub use resources::{WorldClock, WorldEnvironmentModifiers}; // UI、存档、天气与玩法系统通过顶层路径访问时钟和环境修正。
pub use types::{WorldTimeSnapshot, WorldTimeSystemSet}; // 存档快照和调度集属于公开集成接口。

/// 世界时间插件，统一注册时钟推进、新游戏重置和天体光照同步。
#[derive(Default)] // 插件不携带配置，固定参数集中在 constants 模块。
pub struct WorldTimePlugin; // 零尺寸插件由游戏主插件全局注册一次。

impl Plugin for WorldTimePlugin {
    fn build(&self, app: &mut App) {
        dynamic_sky::install_atmosphere_render_state_cleanup(app); // Bevy 0.19.1移除大气设置后必须同帧清理旧HDR渲染门控，避免F12天空盒切换格式冲突。
        crate::skybox::install_authored_environment_map_support(app); // 先建立补光比例派生缓存和共享黑Cube。
        crate::skybox::install_main_pass_viewport_skybox_shader(app); // 天空盒模式在 DLSS/固定比例下按真实主通道视口重建方向。
        app.init_resource::<feathrise_settings::EnvironmentPreviewSettings>()
            .init_resource::<WorldClock>() // 新游戏和旧存档缺省为六月十五日正午。
            .init_resource::<WorldEnvironmentModifiers>() // 未注册天气时使用中性修正，保证现有昼夜画面完全不变。
            .configure_sets(
                Update, // 时间推进与光照写入都运行在主 Update 调度。
                (
                    WorldTimeSystemSet::Advance
                        .after(GamePauseSystemSet::Apply) // 独立宿主未装设置映射时，仍保留原虚拟时间暂停顺序。
                        .after(RuntimeSettingsSyncSet), // 本帧游戏/作者暂停请求先完成映射，不能多推进一帧；天气与最终光照经现有阶段链继承此顺序。
                    WorldTimeSystemSet::SnapshotIo, // 模拟推进后统一保存或恢复时间与天气快照。
                    WorldTimeSystemSet::ApplyLighting, // 再把最终时刻转换为当前帧光照。
                )
                    .chain(), // 三个公开调度集保持推进、快照、最终光照的明确顺序。
            )
            .add_systems(
                Update, // 新游戏重置在所有场景运行，自动推进只在玩法场景运行。
                (
                    clock::advance_world_clock.run_if(in_active_scene()), // 虚拟时间暂停或离开玩法场景时不会推进。
                )
                    .chain()
                    .in_set(WorldTimeSystemSet::Advance), // 外部时刻滑块排在整个推进集之前即可同帧生效。
            )
            .add_systems(
                Update, // 天空候选必须先于最终昼夜亮度写入完成采用。
                (
                    crate::skybox::begin_authored_skybox_requests, // 请求变化时按模式建立 IBL 两件套或天空三件套强句柄。
                    crate::skybox::observe_authored_skybox_assets, // 当前资源组热更新成功或失败只触发匹配相机换代。
                    crate::skybox::apply_authored_skyboxes, // 完整资源组就绪后原子安装可见 Cube 与/或 IBL。
                )
                    .chain()
                    .in_set(WorldTimeSystemSet::ApplyLighting), // 链末命令在下一行世界光照前可见。
            )
            .add_systems(
                Update, // 光照在每个玩法帧同步，也能即时接管新生成的标记实体。
                lighting::apply_world_lighting
                    .in_set(WorldTimeSystemSet::ApplyLighting)
                    .after(crate::skybox::apply_authored_skyboxes) // 新环境首个可见帧就使用正确昼夜与天气亮度。
                    .run_if(in_active_scene()), // 主菜单和加载页的独立灯光与清屏色保持不变。
            )
            .add_systems(
                Update, // 可见圆盘在光照之后读取同帧最终作者配置，并在菜单或固定场景主动清理。
                (
                    dynamic_sky::sync_dynamic_sky, // 仅在没有已采用作者天空盒时启用动态大气。
                    celestial_visuals::sync_celestial_visuals, // 两种天空模式共用同一组动态太阳/月亮圆盘。
                )
                    .chain()
                    .in_set(WorldTimeSystemSet::ApplyLighting)
                    .after(lighting::apply_world_lighting), // 大气与可见天体都读取同帧最终方向光结果。
            );
    }
}
