//! 动态天空 ECS 系统：没有已采用作者天空盒时安装 Bevy 大气，并在两种模式间精确让权。

use bevy::camera::Hdr; // AtmosphereSettings 的 required HDR 资格需要精确恢复。
use bevy::core_pipeline::Skybox; // 动态天空接管期间移除可见 Cube，IBL 组件保持独立。
use bevy::light::{Atmosphere, atmosphere::ScatteringMedium}; // 原生地球大气实体与散射介质资产。
use bevy::pbr::AtmosphereSettings; // 每个合格透视相机启用原生 LUT 动态天空。
use bevy::prelude::*; // Commands、Query、Assets、Projection 与场景状态。
use feathrise_scene_core::view::PrimarySceneView; // 正式玩法主相机资格。
use feathrise_scene_core::state::ScenePhase; // 菜单和加载场景归还作者相机状态。

use super::components::{DynamicSkyAtmosphere, DynamicSkyCameraRuntime}; // 私有所有权快照和大气身份。
use crate::weather::{
    WeatherAuthoringPreview, WeatherPreviewCamera, is_gameplay_scene, supports_camera_local_weather,
}; // 动态天空与天气表现共享玩法及 F12 正交边界。
use crate::{AuthoredSkyMode, AuthoredSkyboxRequest, AuthoredSkyboxStatus}; // 场景互斥模式只有在主 Cube 真正采用后才关闭动态回退。

/// 可被动态天空接管或正等待恢复的相机数据。
type DynamicSkyCameraData = (
    Entity,
    Has<PrimarySceneView>,                        // 正式玩法相机在玩法场景始终有资格。
    Has<WeatherPreviewCamera>,                // F12 作者相机还需预览开启且非正交。
    Option<&'static Camera>,                  // 最小 ECS 测试允许没有实际 Camera 组件。
    Option<&'static Projection>,              // F12 顶、前、侧视图必须排除。
    Option<&'static AtmosphereSettings>,      // 首次接管时冻结，运行中缺失则修复。
    Option<&'static Skybox>,                  // 首次接管时冻结并立即移除可见背景 Cube。
    Option<&'static AuthoredSkyboxRequest>,   // 作者模式声明本身不能代表资源已经就绪。
    Option<&'static AuthoredSkyboxStatus>,    // 只有已采用主 Cube 才取得可见天空所有权。
    Has<Hdr>,                                 // 离场时只移除本领域引入的 HDR。
    Option<&'static DynamicSkyCameraRuntime>, // 所有权快照让标记移除后仍能恢复。
);

/// 查询只覆盖当前候选和仍有运行时快照的相机，不扫描普通辅助相机。
type DynamicSkyCameraFilter = Or<(
    With<PrimarySceneView>,
    With<WeatherPreviewCamera>,
    With<DynamicSkyCameraRuntime>,
)>;

/// 同步唯一地球大气与全部合格玩法天空相机。
pub(in crate::time) fn sync_dynamic_sky(
    mut commands: Commands,               // 安装、恢复相机组件并按需生成大气实体。
    scene: Option<Res<State<ScenePhase>>>, // 正式宿主据此隔离菜单，最小测试可省略。
    preview: Option<Res<WeatherAuthoringPreview>>, // F12 关闭时不得接管常驻离屏相机。
    mut media: Option<ResMut<Assets<ScatteringMedium>>>, // 渲染资产尚未初始化时保留 ClearColor 回退。
    atmospheres: Query<(Entity, Has<DynamicSkyAtmosphere>), With<Atmosphere>>, // 不重复创建项目大气单例。
    cameras: Query<DynamicSkyCameraData, DynamicSkyCameraFilter>, // 候选与待恢复相机均为小集合。
) {
    let gameplay = is_gameplay_scene(scene.as_deref()); // 与云、雨、雾使用同一正式场景边界。
    let preview_active = preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 缺失资源时 F12 作者相机无资格。
    let mut needs_atmosphere = false; // 至少一台相机接管后才延迟创建散射资产。
    for (
        entity,
        is_gameplay_camera,
        is_preview_camera,
        camera,
        projection,
        settings,
        skybox,
        authored_request,
        authored_status,
        has_hdr,
        runtime,
    ) in &cameras
    {
        let active = camera.is_none_or(|camera| camera.is_active); // 排除切换场景期间已停用的旧渲染视图。
        let eligible_preview =
            is_preview_camera && preview_active && supports_camera_local_weather(true, projection); // 只有 F12 透视天气预览取得动态天空。
        let eligible = gameplay && active && (is_gameplay_camera || eligible_preview); // 菜单与普通辅助相机永不接管。
        let authored_skybox_active = authored_request.is_some_and(|request| {
            request.mode == AuthoredSkyMode::Skybox // 场景必须明确选择天空盒模式。
                && authored_status.is_some_and(|status| status.visible_skybox_active) // 主 Cube 必须已完整采用或在安全热换中保留。
                && skybox.is_some() // 防御延迟命令或外部移除，不让空背景关闭动态大气。
        });
        if eligible && authored_skybox_active {
            if settings.is_some() {
                commands.entity(entity).remove::<AtmosphereSettings>(); // 天空盒内烘焙背景不得与原生大气叠加。
            }
            continue; // 保留当前作者 Skybox 与已有动态天空恢复快照，不覆盖主 Cube。
        }
        if eligible {
            needs_atmosphere = true; // 本帧必须确保场景至少存在一个原生大气实体。
            if runtime.is_none() {
                commands.entity(entity).insert(DynamicSkyCameraRuntime {
                    original_settings: settings.cloned(), // 外部质量参数完整冻结供离场归还。
                    original_skybox: skybox.cloned(), // 动态模式只隐藏外部背景，作者天空盒就绪分支不会在此冻结。
                    original_hdr: has_hdr,            // 判断 HDR 是否由本次 required 关系首次引入。
                });
            }
            let mut camera_commands = commands.entity(entity); // 同一相机组件变更保持一个延迟命令入口。
            if settings.is_none() {
                camera_commands.insert(AtmosphereSettings::default()); // 原生 LUT 模式提供稳定实时昼夜天空。
            }
            if skybox.is_some() {
                camera_commands.remove::<Skybox>(); // 明确退役可见天空盒，避免烘焙云和动态云叠加。
            }
            continue;
        }
        let Some(runtime) = runtime else {
            continue; // 从未取得所有权的非目标相机保持完全不变。
        };
        restore_camera(&mut commands, entity, runtime, authored_skybox_active); // 离场仅归还动态天空持有的状态，不覆盖后来已采用的作者天空盒。
    }
    if needs_atmosphere {
        ensure_earth_atmosphere(&mut commands, media.as_deref_mut(), &atmospheres); // 同帧为新相机提供最近大气。
    }
}

/// 恢复动态天空接管前的相机大气、可见天空盒与 HDR 状态。
fn restore_camera(
    commands: &mut Commands,           // 延迟组件操作在本 Update 末统一生效。
    entity: Entity,                    // 当前失去动态天空资格的相机。
    runtime: &DynamicSkyCameraRuntime, // 首次接管冻结的作者状态。
    authored_skybox_active: bool,      // 天空盒已接管时，旧动态快照不再拥有可见背景。
) {
    let mut entity_commands = commands.entity(entity); // 保证移除与恢复围绕同一实体。
    if authored_skybox_active {
        entity_commands.remove::<AtmosphereSettings>(); // F12关闭或改正交后继续保持天空盒与大气互斥。
    } else if let Some(settings) = runtime.original_settings.clone() {
        entity_commands.insert(settings); // 作者本来启用大气时恢复其完整质量参数。
    } else {
        entity_commands.remove::<AtmosphereSettings>(); // 本领域新增的大气资格必须清理。
    }
    if !authored_skybox_active {
        if let Some(skybox) = runtime.original_skybox.clone() {
            entity_commands.insert(skybox); // 只有未转交作者天空盒时才归还外部原背景。
        } else {
            entity_commands.remove::<Skybox>(); // 不删除异步加载完成后新采用的作者Cube。
        }
    }
    if runtime.original_hdr {
        entity_commands.insert(Hdr); // 作者原 HDR 资格始终保留。
    } else if runtime.original_settings.is_none() && !authored_skybox_active {
        entity_commands.remove::<Hdr>(); // 仅由 AtmosphereSettings required 关系引入的 HDR 一并移除。
    }
    entity_commands.remove::<DynamicSkyCameraRuntime>(); // 释放快照，下次接管重新冻结最新作者状态。
}

/// 复用现有大气或延迟创建唯一默认地球大气。
fn ensure_earth_atmosphere(
    commands: &mut Commands,                      // 无现有实体时生成项目大气。
    media: Option<&mut Assets<ScatteringMedium>>, // 缺少渲染资产时不阻断 ClearColor 安全回退。
    atmospheres: &Query<(Entity, Has<DynamicSkyAtmosphere>), With<Atmosphere>>, // 当前全部原生大气通常为零或一。
) {
    let mut project_atmosphere = None; // 记录首个本领域已采用实体。
    let mut any_atmosphere = None; // 外部作者大气可直接复用，避免生成第二颗行星。
    for (entity, is_project_atmosphere) in atmospheres.iter() {
        any_atmosphere.get_or_insert(entity); // 保留稳定查询顺序的首个大气。
        if is_project_atmosphere {
            if project_atmosphere.is_some() {
                commands.entity(entity).despawn(); // 只清理由本领域异常重复生成的实体。
            } else {
                project_atmosphere = Some(entity); // 首个项目大气保持全局唯一身份。
            }
        }
    }
    if project_atmosphere.is_some() {
        return; // 正式稳定帧没有资产写入或组件命令。
    }
    if let Some(entity) = any_atmosphere {
        commands.entity(entity).insert(DynamicSkyAtmosphere); // 采用作者已有大气，不改写其物理参数。
        return;
    }
    let Some(media) = media else {
        return; // 设备/插件初始化不完整时让相机显示世界时间 ClearColor 回退。
    };
    let medium = media.add(ScatteringMedium::earth(256, 256)); // 固定 Earth LUT 分辨率仅在首次玩法相机出现时创建。
    commands.spawn((
        Name::new("WorldDynamicAtmosphere"), // 层级诊断使用稳定 ASCII 名称。
        DynamicSkyAtmosphere,                // 后续稳定帧与异常去重按该身份处理。
        Atmosphere::earth(medium), // 地球半径、空气层厚度与地表反照率使用 Bevy 物理默认值。
    ));
}
