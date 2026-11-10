// 天气体积雾：以玩家档位限制光线步进预算，以连续天气和昼夜天空决定雾的外观。

use bevy::{
    camera::RenderTarget, // 只有窗口目标玩法相机可以进入体积雾视图。
    light::{FogVolume, VolumetricFog, VolumetricLight}, // Bevy 原生体积雾、有限雾体积与光束资格。
    prelude::*,           // Commands、Query、Transform 和常用 ECS 类型。
};
use feathrise_scene_core::view::PrimarySceneView; // 唯一玩法主相机标记隔离主菜单和纹理相机。
use feathrise_settings::{AntiAliasingMode, RenderSettings, VolumetricFogQuality}; // 玩家档位决定开关、步进数与 TAA 抖动。
use feathrise_scene_core::state::ScenePhase; // 菜单和加载阶段必须归还体积雾所有权。

use super::{fog::weather_fog_profile, is_gameplay_scene, supports_camera_local_weather}; // 距离雾、云雨与体积雾共享天气色、场景边界和F12正交测绘例外。
use crate::{
    WeatherAuthoringPreview,
    WeatherOutput,
    WeatherPreviewCamera,
    WorldCelestialLight, // 连续天气外观和玩法天体方向光身份。
    weather::components::{
        AuthoredCelestialVolumetricLight, AuthoredGameplayVolumetricFog, WeatherVolumetricFogVolume,
    },
}; // 运行时只接管明确标记的玩法实体并保存作者状态。

/// 相机局部雾体积的完整世界尺寸。
const VOLUME_SCALE: Vec3 = Vec3::new(420.0, 160.0, 420.0); // 覆盖约两百一十米水平半径，避免场景尺度全量体积。
/// 雾体积水平跟随网格。
const HORIZONTAL_SNAP: f32 = 32.0; // 相机在一个格内移动时不重复写 Transform。
/// 雾体积垂直跟随网格。
const VERTICAL_SNAP: f32 = 8.0; // 保持相机位于体积内部，同时避免每帧跟随地形细小起伏。
/// 晴天启用体积雾时的基础密度。
const CLEAR_DENSITY: f32 = 0.0018; // 提供可观察的晨昏光束，但不替代零透明度晴天距离雾。
/// 从晴天到最低能见度天气增加的最大密度。
const WEATHER_DENSITY_RANGE: f32 = 0.0102; // 薄雾与暴雨连续增强近场散射，不形成不透明墙。
/// 无环境贴图时使用的低强度体积环境光。
const AMBIENT_INTENSITY: f32 = 0.035; // 避免背向天体光时雾体积完全消失，同时保留昼夜亮度关系。

/// 同步玩法相机体积雾、天体方向光光束资格和唯一有限雾体积。
#[allow(clippy::type_complexity)]
// 两个 Query 都需原子读取作者状态与玩法资格，拆分会破坏精确恢复边界。
#[allow(clippy::too_many_arguments)] // 系统参数分别承载天气输入、设置、相机、天体光和唯一雾体积生命周期。
pub(crate) fn sync_weather_volumetric_fog(
    mut commands: Commands, // 仅在开关、档位、作者恢复或唯一雾体积生命周期变化时提交命令。
    settings: Option<Res<RenderSettings>>, // 独立天气测试允许缺少设置插件，缺失时按关闭处理。
    output: Res<WeatherOutput>, // 天气连续能见度、色调和环境光倍率决定实际雾外观。
    preview: Option<Res<WeatherAuthoringPreview>>, // F12显式开启时允许离屏作者相机取得有限体积雾视图。
    scene: Option<Res<State<ScenePhase>>>,          // 菜单、加载和独立 ECS 测试共用同一场景边界。
    clear_color: Option<Res<ClearColor>>, // 世界时间写入的最终天空色用于晨昏、夜晚和天气雾色。
    mut cameras: Query<
        (
            Entity,                                 // 当前可能被接管或等待恢复的相机实体。
            Has<Camera3d>,                          // 失去 3D 相机身份时必须归还已有作者状态。
            Has<PrimarySceneView>,                      // 只有唯一玩法主相机具备体积雾资格。
            Has<WeatherPreviewCamera>, // F12作者视口必须显式加入，普通纹理相机仍被排除。
            Option<&Projection>,       // F12正交测绘视图不接收有限相机局部体积。
            Option<&RenderTarget>,     // 窗口目标与纹理目标使用明确隔离。
            Option<&Transform>,        // 玩法根相机在Update完成跟随后的本地位置等于世界位置。
            Option<&VolumetricFog>,    // 当前可能属于作者或天气系统的体积雾组件。
            Option<&AuthoredGameplayVolumetricFog>, // 首次接管前保存的作者状态。
        ),
        (
            Or<(With<Camera3d>, With<AuthoredGameplayVolumetricFog>)>, // 同时覆盖新相机和等待恢复的旧相机。
            Without<WeatherVolumetricFogVolume>, // 与可写 Transform 的自有雾体积查询显式互斥。
        ),
    >,
    celestial_lights: Query<
        (
            Entity,                                    // 当前玩法方向光实体。
            Has<WorldCelestialLight>,                  // 只接管世界时间显式标记的天体光。
            Has<VolumetricLight>,                      // 当前是否具备 Bevy 体积光束资格。
            Option<&AuthoredCelestialVolumetricLight>, // 首次接管前保存的作者资格。
        ),
        With<DirectionalLight>, // 点光和聚光灯不在首版无界接管范围内。
    >,
    mut volumes: Query<
        (Entity, &mut FogVolume, &mut Transform), // 唯一雾体积的外观与分格位置。
        With<WeatherVolumetricFogVolume>,         // 绝不修改场景作者自行创建的其他 FogVolume。
    >,
) {
    let quality = settings
        .as_deref()
        .map_or(VolumetricFogQuality::Off, |settings| {
            settings.graphics.volumetric_fog_quality // 缺失设置资源时保持功能加入前关闭基线。
        }); // 完成本帧独立体积雾质量读取。
    let gameplay_scene = is_gameplay_scene(scene.as_deref()); // 非玩法状态即使残留 PrimarySceneView 也必须恢复。
    let preview_active = preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 缺失资源时严格保持窗口玩法相机基线。
    let clear_color = clear_color.map_or(Color::BLACK, |clear_color| clear_color.0); // 最小测试无天空资源时使用安全黑色。
    let weather_profile = weather_fog_profile(&output, clear_color, gameplay_scene); // 与 DistanceFog 共用颜色和能见度曲线。
    let desired_camera_fog = quality
        .raymarch_steps()
        .filter(|_| gameplay_scene)
        .map(|steps| {
            camera_fog_for_quality(
                steps,                 // 玩家档位只改变光线步进预算。
                settings.as_deref(),   // 当前抗锯齿只决定是否使用 TAA 抖动。
                &output,               // 天气提供连续环境光能量。
                weather_profile.color, // 昼夜与天气同源雾色防止午夜出现白色环境雾。
            )
        }); // 关闭或离开玩法场景时不构造体积雾视图。
    let desired_volume = desired_camera_fog
        .as_ref()
        .map(|_| fog_volume_for_weather(weather_profile)); // 只有启用相机视图时才准备有限雾体积。

    let mut gameplay_camera_position = None; // 项目约束唯一玩法相机，首个合法位置驱动唯一雾体积。
    for (
        entity,
        has_camera_3d,
        has_marker,
        has_preview_marker,
        projection,
        target,
        transform,
        current,
        authored,
    ) in &mut cameras
    {
        let selected_target = if preview_active {
            has_preview_marker && target.is_some() // F12离屏纹理目标由作者显式加入天气预览。
        } else {
            has_marker && target.is_some_and(|target| matches!(target, RenderTarget::Window(_))) // 正常玩法只允许窗口主相机。
        };
        let eligible = has_camera_3d
            && selected_target
            && supports_camera_local_weather(has_preview_marker, projection) // 正交时恢复作者相机状态并清理唯一雾体积。
            && transform.is_some_and(|transform| transform.translation.is_finite())
            && desired_camera_fog.is_some(); // 玩法身份、窗口输出、有限姿态和启用档缺一不可。
        if eligible {
            let Some(desired) = desired_camera_fog else {
                continue; // 防御未来资格条件调整后出现启用参数缺失，运行时不得因画质切换 panic。
            }; // Copy 组件按相机复用同一规范参数。
            if authored.is_none() {
                commands
                    .entity(entity)
                    .insert(AuthoredGameplayVolumetricFog {
                        value: current.copied(), // 仅在本轮首次接管时捕获作者完整组件或缺失状态。
                    }); // 后续天气过渡不得覆盖首次作者基线。
            }
            if current.is_none_or(|current| !camera_fog_matches(current, &desired)) {
                commands.entity(entity).insert(desired); // 新相机、档位或 TAA 状态变化时才更新渲染参数。
            }
            if gameplay_camera_position.is_none() {
                gameplay_camera_position = transform.map(|transform| transform.translation); // 唯一根玩法相机提供本帧分格跟随中心。
            }
        } else if let Some(authored) = authored {
            restore_authored_camera_fog(&mut commands, entity, current, *authored); // 关闭、离场或目标变化时精确恢复作者状态。
            commands
                .entity(entity)
                .remove::<AuthoredGameplayVolumetricFog>(); // 恢复完成即归还相机控制权。
        }
    }

    let has_active_view = gameplay_camera_position.is_some(); // 没有合法玩法视图时不接管天体光或生成雾体积。
    for (entity, has_celestial_marker, has_volumetric_light, authored) in &celestial_lights {
        let eligible = has_active_view && has_celestial_marker; // 世界时间天体光是首版唯一体积光束来源。
        if eligible {
            if authored.is_none() {
                commands
                    .entity(entity)
                    .insert(AuthoredCelestialVolumetricLight {
                        present: has_volumetric_light, // 记录作者原本是否显式启用体积光。
                    }); // 每盏新生成的天体光只在首次接管时保存一次。
            }
            if !has_volumetric_light {
                commands.entity(entity).insert(VolumetricLight); // 阴影开启时由 Bevy 生成遮挡后的天体光束。
            }
        } else if let Some(authored) = authored {
            restore_authored_volumetric_light(
                &mut commands,        // 延迟组件恢复入口。
                entity,               // 当前方向光实体。
                has_volumetric_light, // 当前可能由天气系统插入的光束资格。
                *authored,            // 首次接管前作者资格。
            ); // 关闭或失去天体身份时精确归还作者配置。
            commands
                .entity(entity)
                .remove::<AuthoredCelestialVolumetricLight>(); // 恢复后清理私有所有权标记。
        }
    }

    sync_single_fog_volume(
        &mut commands,            // 唯一雾体积生命周期命令入口。
        &mut volumes,             // 只枚举天气系统自有雾体积。
        desired_volume.as_ref(),  // 关闭时为 None 并清理全部自有体积。
        gameplay_camera_position, // 缺少相机位置时同样不保留渲染体积。
    ); // 完成当前帧有界雾体积创建、更新或清理。
}

/// 按玩家质量和当前抗锯齿构造玩法相机体积雾参数。
fn camera_fog_for_quality(
    step_count: u32,                 // 低档 32、高档 64 的固定光线步进预算。
    settings: Option<&RenderSettings>, // TAA 模式允许启用官方推荐的起点抖动。
    output: &WeatherOutput,          // 天气环境光倍率连续调整低成本体积环境光。
    fog_color: Color,                // 最终昼夜天空与天气共同生成的体积环境光颜色。
) -> VolumetricFog {
    let uses_taa = settings.is_some_and(|settings| {
        settings.graphics.anti_aliasing == AntiAliasingMode::Taa // 只有跨帧累积能稳定随机起点噪点。
    }); // 完成 TAA 兼容判断。
    let ambient_factor = finite_nonnegative(output.ambient_factor, 1.0); // 损坏天气输出回退中性环境光倍率。
    let fog_color = fog_color.to_linear(); // 体积环境光在线性空间沿用同源雾色，避免夜景被白色抬亮。
    VolumetricFog {
        ambient_color: Color::linear_rgba(fog_color.red, fog_color.green, fog_color.blue, 1.0), // 相机环境散射与有限雾体积使用同一 RGB。
        ambient_intensity: AMBIENT_INTENSITY * ambient_factor, // 阴雨和夜景沿用天气环境光能量关系。
        jitter: if uses_taa { 0.5 } else { 0.0 }, // 复用 Bevy 官方 TAA 体积雾示例抖动强度。
        step_count,                               // 玩家低/高档只在此处改变每像素光线步进预算。
    }
}

/// 从距离雾同源天气参数构造无密度纹理的有限体积雾。
fn fog_volume_for_weather(profile: super::fog::WeatherFogProfile) -> FogVolume {
    let color = profile.color.to_linear(); // DistanceFog 已经把昼夜天空与天气色温合成到线性色。
    FogVolume {
        fog_color: Color::linear_rgba(color.red, color.green, color.blue, 1.0), // 体积雾着色器只读取 RGB，显式规范透明通道。
        density_factor: CLEAR_DENSITY + WEATHER_DENSITY_RANGE * profile.severity, // 能见度降低时连续增强散射密度。
        density_texture: None, // 首版不新增美术资源或三维噪声采样成本。
        density_texture_offset: Vec3::ZERO, // 无密度纹理时保持稳定零偏移。
        absorption: 0.28,      // 轻度吸收保留远景层次，避免与 DistanceFog 叠成黑墙。
        scattering: 0.42,      // 散射略强于吸收以突出晨昏与薄雾光束。
        scattering_asymmetry: 0.72, // 视线朝向天体时增强光束，背向时自然减弱。
        light_tint: Color::WHITE, // 天体方向光本身已经包含昼夜与天气颜色。
        light_intensity: 1.0,  // 不额外放大世界时间最终照度。
    }
}

/// 同步唯一自有雾体积；额外残留实例会被清理，避免密度叠加。
fn sync_single_fog_volume(
    commands: &mut Commands, // 创建或清理体积的延迟命令入口。
    volumes: &mut Query<(Entity, &mut FogVolume, &mut Transform), With<WeatherVolumetricFogVolume>>, // 当前所有天气自有体积，正常最多一个。
    desired: Option<&FogVolume>,   // 启用档的天气外观；关闭时为 None。
    camera_position: Option<Vec3>, // 合法玩法相机世界位置决定分格中心。
) {
    let desired_transform = desired
        .zip(camera_position)
        .map(|(_, position)| snapped_volume_transform(position)); // 只有外观和有限相机位置都存在时才保留体积。
    let mut kept_one = false; // 第一份合法体积复用，额外实例统一清理。
    for (entity, mut current, mut transform) in volumes.iter_mut() {
        let Some((desired, desired_transform)) = desired.zip(desired_transform.as_ref()) else {
            commands.entity(entity).despawn(); // 关闭、离场或缺相机时移除自有体积并停止 GPU 体积绘制。
            continue;
        };
        if kept_one {
            commands.entity(entity).despawn(); // 防御热重载或异常重复生成导致雾密度成倍叠加。
            continue;
        }
        kept_one = true; // 当前第一份体积成为唯一复用实例。
        if !fog_volume_matches(&current, desired) {
            *current = desired.clone(); // 天气色或能见度真实变化时只更新体积 uniform 数据。
        }
        if *transform != *desired_transform {
            *transform = *desired_transform; // 相机跨过跟随网格或高度格时才更新世界变换。
        }
    }
    if let (Some(desired), Some(desired_transform)) = (desired, desired_transform)
        && !kept_one
    {
        commands.spawn((
            Name::new("WeatherVolumetricFogVolume"), // 调试检查器中的稳定实体名称。
            WeatherVolumetricFogVolume,              // 唯一自有体积身份，禁止误改作者体积。
            desired.clone(),                         // 当前天气雾色、密度和散射参数。
            desired_transform,                       // 以玩法相机分格位置为中心的有限世界尺寸。
        )); // 新玩法相机或重新开启档位时创建唯一体积。
    }
}

/// 把玩法相机位置量化到稳定网格并附加固定有限体积尺寸。
fn snapped_volume_transform(position: Vec3) -> Transform {
    Transform::from_xyz(
        snap(position.x, HORIZONTAL_SNAP), // 水平 X 轴每三十二米更新一次。
        snap(position.y, VERTICAL_SNAP),   // 垂直轴每八米更新一次以覆盖地形高度变化。
        snap(position.z, HORIZONTAL_SNAP), // 水平 Z 轴使用同一网格，避免方向偏置。
    )
    .with_scale(VOLUME_SCALE) // 单位立方体缩放为固定有界天气雾区域。
}

/// 把有限坐标量化到最近的正间距网格。
fn snap(value: f32, spacing: f32) -> f32 {
    (value / spacing).round() * spacing // 调用前已验证相机世界位置有限，间距由正常数常量提供。
}

/// 比较全部相机体积雾字段，稳定输入不触发组件变更检测。
fn camera_fog_matches(current: &VolumetricFog, desired: &VolumetricFog) -> bool {
    current.ambient_color == desired.ambient_color
        && current.ambient_intensity == desired.ambient_intensity
        && current.jitter == desired.jitter
        && current.step_count == desired.step_count
}

/// 比较全部无纹理天气雾字段，稳定天气不重复上传体积参数。
fn fog_volume_matches(current: &FogVolume, desired: &FogVolume) -> bool {
    current.fog_color == desired.fog_color
        && current.density_factor == desired.density_factor
        && current.density_texture == desired.density_texture
        && current.density_texture_offset == desired.density_texture_offset
        && current.absorption == desired.absorption
        && current.scattering == desired.scattering
        && current.scattering_asymmetry == desired.scattering_asymmetry
        && current.light_tint == desired.light_tint
        && current.light_intensity == desired.light_intensity
}

/// 关闭或失去资格时恢复相机作者原本的体积雾组件。
fn restore_authored_camera_fog(
    commands: &mut Commands,                 // 相机组件延迟写入入口。
    entity: Entity,                          // 当前相机实体。
    current: Option<&VolumetricFog>,         // 当前天气系统覆盖后的组件。
    authored: AuthoredGameplayVolumetricFog, // 首次接管前保存的作者状态。
) {
    match authored.value {
        Some(desired) if current.is_none_or(|current| !camera_fog_matches(current, &desired)) => {
            commands.entity(entity).insert(desired); // 作者原本有组件时精确恢复全部字段。
        }
        None if current.is_some() => {
            commands.entity(entity).remove::<VolumetricFog>(); // 作者原本没有组件时移除天气系统引入部分。
        }
        _ => {} // 当前状态已经等于作者基线时不重复写入。
    }
}

/// 关闭或失去天体身份时恢复方向光作者原本的体积光资格。
fn restore_authored_volumetric_light(
    commands: &mut Commands,                    // 方向光组件延迟写入入口。
    entity: Entity,                             // 当前方向光实体。
    current: bool,                              // 当前是否存在 VolumetricLight。
    authored: AuthoredCelestialVolumetricLight, // 首次接管前保存的作者资格。
) {
    match (authored.present, current) {
        (true, false) => {
            commands.entity(entity).insert(VolumetricLight); // 恢复作者原有光束资格。
        }
        (false, true) => {
            commands.entity(entity).remove::<VolumetricLight>(); // 移除仅由天气系统引入的光束资格。
        }
        _ => {} // 当前状态已经等于作者基线时不重复写入。
    }
}

/// 返回有限非负天气倍率，否则使用安全回退。
fn finite_nonnegative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0) // 环境光倍率允许降至零但不允许负能量。
    } else {
        fallback // 损坏天气输出回退中性倍率，避免向渲染 uniform 写入 NaN。
    }
}

#[cfg(test)] // ECS 与纯参数测试放在相邻子模块，保持运行时代码文件可维护长度。
mod tests;
