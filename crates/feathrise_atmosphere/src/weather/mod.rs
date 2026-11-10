// atmosphere::weather：统一管理不绑定地域的自动天气、调试覆盖、存档快照和连续环境输出。

pub mod climate; // 正式场景显式使用的天气区域、四季权重和局部气候配置。
mod components; // 体积雾相机、天体光和唯一有限雾体积的运行时所有权。
pub mod constants; // 天气持续、过渡、晨雾与可玩亮度常量。
mod effects; // 环境光照桥、玩法相机距离雾、世界视线动态云与单发射体GPU降雨表现域。
pub mod messages; // 剧情和调试工具使用的天气控制消息。
pub mod resources; // 私有状态机、F1覆盖和连续环境输出资源。
mod simulation; // 状态推进、权重抽取、输出插值和ECS系统功能域。
pub mod types; // 天气种类、区域、玩法修正、快照与调度集。

use crate::WorldTimeSystemSet; // 天气推进、存档恢复和解析必须嵌入世界时间顺序。
use bevy::prelude::*; // App、Plugin、Update和调度配置用于注册天气系统。
use bevy::transform::TransformSystems; // 体积雾组件与分格 Transform 必须在本帧最终变换传播前落地。
use bevy_hanabi::EffectSystems; // F1暂停雨丝单批预热必须排在Hanabi常规发射器tick与编译之后。

pub use climate::{UNIFORM_CLIMATE_PROFILE, WeatherClimateProfile}; // 场景和扩展场景通过公开配置显式声明天气区域与概率修正。
pub use components::WeatherPreviewCamera; // F12等明确作者视口可主动加入天气表现，普通辅助相机仍保持隔离。
// 时间动态天空与天气局部效果共享场景和F12投影边界。
pub(crate) use effects::{is_gameplay_scene, supports_camera_local_weather};
pub use messages::WeatherCommand; // 剧情、测试面板和工具通过顶层路径发送天气请求。
pub use resources::{
    DynamicSceneWeatherClimate, WeatherAuthoringPreview, WeatherDebugOverride, WeatherOutput,
    WeatherState,
}; // 外部系统通过顶层稳定路径访问天气资源。
pub use types::{
    WeatherGameplayModifiers, WeatherKind, WeatherRegion, WeatherSnapshot, WeatherSystemSet,
}; // 存档与集成层通过顶层路径使用普通类型。

/// 天气插件，注册自动演化、存档预留阶段和连续输出解析。
#[derive(Default)] // 天气气候由资源显式输入，插件本身不保存地点或项目规则。
pub struct WeatherPlugin; // 零尺寸插件由游戏主插件全局注册一次。

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        effects::install_dynamic_cloud_material(app); // 完整渲染宿主登记内置Shader与透明云材质；纯ECS测试安全跳过。
        effects::rain_occlusion::install(app); // 无颜色输出的独立雨向捕获，复用实际屋顶几何。
        app.init_resource::<WeatherState>() // 新游戏默认保持至少四游戏小时晴天。
            .init_resource::<WeatherDebugOverride>() // 调试模式默认关闭且预览为完整目标效果。
            .init_resource::<WeatherAuthoringPreview>() // F12关闭时保持无影响的纯视觉覆盖默认值。
            .init_resource::<DynamicSceneWeatherClimate>() // 动态清单就绪前不推进任何隐式局部气候。
            .init_resource::<WeatherOutput>() // 插件安装首帧即可提供安全晴天输出。
            .add_message::<WeatherCommand>() // 注册剧情和调试工具共用的天气控制消息。
            .configure_sets(
                Update, // 天气模拟、存档衔接和输出解析都运行在主Update调度。
                (
                    WeatherSystemSet::Advance.after(WorldTimeSystemSet::Advance), // 先取得本帧最终月份和时刻，再推进天气。
                    WeatherSystemSet::SnapshotIo.after(WeatherSystemSet::Advance), // 存读档在推进后、解析前原子读写天气状态。
                    WeatherSystemSet::Resolve
                        .after(WeatherSystemSet::SnapshotIo) // 读档恢复必须先于本帧天气输出解析。
                        .before(WorldTimeSystemSet::ApplyLighting), // 世界光照读取同帧最新天气输出。
                    WeatherSystemSet::ApplyEffects
                        .after(WeatherSystemSet::Resolve) // 雾、云、雨和风只能读取解析后的连续输出。
                        .after(WorldTimeSystemSet::ApplyLighting), // 视觉效果读取当帧最终ClearColor，避免前后置职责形成调度环。
                ), // 结束天气公开调度集顺序声明。
            ) // 完成跨插件调度约束注册。
            .add_systems(
                Update, // 命令、场景区域同步和自动推进共享同一推进阶段。
                (
                    simulation::sync_weather_region, // 先把当前玩法场景同步到其显式登记的天气区域。
                    simulation::handle_weather_commands, // 再消费剧情、调试和重置随机序列命令。
                    simulation::advance_weather_state, // 最后按暂停感知虚拟时间推进自动天气。
                )
                    .chain() // 四个天气状态系统保持区域同步、命令处理与自动推进的确定性顺序。
                    .in_set(WeatherSystemSet::Advance), // 整组系统固定在世界时间推进之后。
            ) // 完成天气状态推进系统注册。
            .add_systems(
                Update, // 连续输出每帧解析，暂停时F1预览仍可即时生效。
                (
                    simulation::resolve_weather_output, // 自动天气与调试预览先合成为唯一连续输出资源。
                    effects::apply_weather_environment, // 再把同帧输出桥接给世界时间唯一最终光照写入者。
                )
                    .chain() // 环境桥必须读取本帧刚解析的输出，不能落后一帧。
                    .in_set(WeatherSystemSet::Resolve), // 整组固定在存读档后和世界时间光照前。
            ) // 完成天气输出与环境桥注册。
            .add_systems(
                Update, // 雾和降雨在世界时间写完最终天空颜色后同步。
                (
                    effects::apply_weather_fog, // DistanceFog结合当帧最终ClearColor与天气色调。
                    effects::sync_weather_cloud_curtain, // 世界视线云层跟随唯一视觉相机并按统一风场滚动。
                    effects::sync_weather_rain,          // 单个Hanabi发射体围绕玩法主相机复用。
                )
                    .in_set(WeatherSystemSet::ApplyEffects), // 主菜单与小场景通过标记和专属层保持隔离。
            ) // 完成天气核心Update系统注册。
            .add_systems(
                PostUpdate, // Hanabi在PostUpdate计算本帧GPU粒子初始化数量。
                (
                    (
                        effects::sync_weather_volumetric_fog, // Update完成相机跟随后读取玩法根相机最终本地姿态。
                        ApplyDeferred, // 新雾体积和组件必须在本帧变换传播与渲染提取前落地。
                    )
                        .chain()
                        .before(TransformSystems::Propagate), // 同帧把有限雾体积Transform传播到GlobalTransform。
                    effects::seed_paused_weather_rain_preview
                        .after(EffectSystems::TickSpawners) // 先让暂停的inactive发射器把旧spawn_count归零。
                        .after(EffectSystems::CompileEffects), // 等效果编译并确认渲染资源就绪后再提交唯一静态批次。
                    effects::sync_weather_rain_shelter
                        .after(feathrise_environment::rain_shelter::RainShelterUpload) // 等完整当前世界变换代理，避免移动屋檐滞后一帧。
                        .before(EffectSystems::UpdatePropertiesFromAsset), // Hanabi整体整理/提取属性前写入本帧小快照。
                    effects::sync_weather_rain_water
                        .after(feathrise_environment::rain_water::RainWaterMaskUpdate) // 高度图相机与范围必须和水面波形同帧发布。
                        .before(EffectSystems::UpdatePropertiesFromAsset), // 首次出生和暂停雨丝均读取完整当前属性。
                    effects::sync_weather_rain_occlusion
                        .after(effects::rain_occlusion::RainOcclusionUpdate) // 目标换代与停雨失效先完成。
                        .after(effects::sync_weather_rain_water) // 两个纹理槽各自更新，保持完整材质。
                        .before(EffectSystems::UpdatePropertiesFromAsset), // 同帧交给Hanabi提取。
                ),
            ); // 完成F1暂停雨效防堆积与静态预览注册。
    } // 结束天气插件构建。
} // 结束天气插件实现。
