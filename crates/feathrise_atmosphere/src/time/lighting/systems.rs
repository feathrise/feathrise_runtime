// 天体光照 ECS 系统：统一写入玩法方向光、相机环境光、清屏色和环境 IBL 强度。

use bevy::color::Mix; // 颜色在日夜和低角度日光之间平滑插值。
use bevy::core_pipeline::Skybox; // 天空盒模式的可见 Cube 亮度随昼夜与天气统一变化。
use bevy::ecs::system::SystemParam; // 把同属最终光照输入的资源收拢为单一系统参数。
use bevy::light::SunDisk; // 动态场景角直径通过方向光光盘进入光栅与 Solari。
use bevy::prelude::*; // Commands、Query、光照组件和资源等 ECS 常用类型。
use feathrise_scene_core::view::PrimarySceneView; // 只给项目玩法主相机配置独立环境光。
use feathrise_settings::EnvironmentPreviewSettings; // F1 方向光无限制倍率在最终照度阶段应用。

use super::helpers::{
    DAWN_CLEAR_COLOR,    // 六点朝霞暖桃色。
    DUSK_CLEAR_COLOR,    // 十八点夕阳橙红色。
    LOW_SUN_COLOR,       // 地平线附近暖色日光。
    MOONLIGHT_COLOR,     // 环境光夜色继续使用原固定冷蓝基准。
    NIGHT_CLEAR_COLOR,   // 夜间深蓝黑清屏色。
    lighting_sample,     // 当前时刻完整光照采样。
    season_light_factor, // 动态场景日间峰值继续接受现有季节倍率。
};
use crate::{
    AuthoredSkyboxRequest,            // 相机作者请求声明动态天空或天空盒模式。
    AuthoredSkyboxStatus,             // 主 Cube 采用状态决定是否关闭 Bevy 太阳盘。
    WeatherAuthoringPreview,          // F12 打开时以作者相机作为当前可见天空。
    WeatherPreviewCamera,             // F12显式作者视口复用同一昼夜与天气环境光。
    WorldCelestialLight,              // 查询带作者基准的玩法方向光。
    WorldCelestialLightKind,          // 区分固定兼容灯、真实太阳灯和真实月亮灯。
    WorldClock,                       // 查询当前日期与时刻。
    WorldEnvironmentModifiers,        // 查询天气等环境系统在最终写入前生成的连续修正。
    WorldSkyProfile,                  // 查询天空盒模式的作者日间可见亮度。
    time::WorldEnvironmentMapProfile, // 查询相机IBL补光与镜面反射的日间强度基准。
    time::constants::{
        DAY_AMBIENT_BRIGHTNESS,        // 动态场景完整日景的相机环境光基准。
        MOON_ANGULAR_DIAMETER_RADIANS, // 月亮方向光与可见月亮共享固定视角尺寸。
        NIGHT_AMBIENT_BRIGHTNESS,      // 动态场景午夜最低相机环境光基准。
        NIGHT_SKY_BRIGHTNESS_FACTOR,   // 动态场景午夜天空、补光与反射最低倍率。
    },
}; // 完成世界时间最终光照写入所需类型导入。

/// 朝霞和夕阳覆盖基础昼夜清屏色的最大比例。
const TWILIGHT_CLEAR_COLOR_MIX: f32 = 0.82; // 保留少量场景作者天空底色，同时让晨昏景观足够明显。
/// 晨昏暖色进入玩法环境光的最大比例。
const TWILIGHT_AMBIENT_COLOR_MIX: f32 = 0.22; // 轻微暖色塑造氛围但不显著扭曲角色和地表材质。
/// 小于该照度的动态天体灯不执行接触阴影。
const ACTIVE_CELESTIAL_LIGHT_EPSILON_LUX: f32 = 0.01; // 标准级联阴影资格保持稳定，仅零贡献灯跳过逐像素接触阴影成本。

/// 世界光照系统每帧共同消费并写回的资源集合。
#[derive(SystemParam)]
pub(crate) struct WorldLightingInputs<'w> {
    clock: Res<'w, WorldClock>, // 世界时间是天体位置与昼夜曲线的唯一输入。
    environment: Res<'w, WorldEnvironmentModifiers>, // 天气只通过连续环境修正参与最终合成。
    settings: Res<'w, EnvironmentPreviewSettings>, // F1测试倍率在作者基准和自然系数之后应用。
    preview: Option<Res<'w, WeatherAuthoringPreview>>, // F12 关闭时测试和正式玩法默认选择主相机。
    clear_color: ResMut<'w, ClearColor>, // 最终天空色由本系统统一写回，避免多个系统争抢所有权。
}

/// 在保留昼夜亮度关系的前提下，把天气乘色和天空亮度应用到清屏色。
fn apply_sky_modifiers(
    celestial_color: Color, // 世界时间已经生成的日景、夜景、朝霞或夕阳颜色。
    tint: Color,            // 天气提供的线性乘色色温，白色表示不改变原色。
    tint_strength: f32,     // 零至一决定从中性白乘色过渡到完整天气乘色。
    brightness_factor: f32, // 动态大气回退底色与环境 IBL 共用的天气亮度倍率。
) -> Color {
    let celestial = celestial_color.to_linear(); // 所有乘算在线性色彩空间完成，避免sRGB直接相乘造成偏色。
    let tint = tint.to_linear(); // 调试或外部调用即使提供sRGB颜色也先转换到同一空间。
    let strength = if tint_strength.is_finite() {
        tint_strength.clamp(0.0, 1.0) // 合法混色强度严格限制在中性到完整天气色之间。
    } else {
        0.0 // 损坏强度安全保留世界时间原始天空。
    }; // 完成天气乘色强度规范化。
    let brightness = if brightness_factor.is_finite() {
        brightness_factor.max(0.0) // 天空允许被压暗或增强，但负倍率没有物理意义。
    } else {
        1.0 // 损坏亮度倍率回退中性值，避免整个天空变为NaN。
    }; // 完成天气天空亮度规范化。
    let red_multiplier = 1.0 + (tint.red - 1.0) * strength; // 红通道从中性一连续过渡到天气乘色值。
    let green_multiplier = 1.0 + (tint.green - 1.0) * strength; // 绿通道保持同一连续乘色口径。
    let blue_multiplier = 1.0 + (tint.blue - 1.0) * strength; // 蓝通道保持同一连续乘色口径。
    Color::linear_rgba(
        celestial.red * red_multiplier.max(0.0) * brightness, // 天气只能按合法非负颜色和亮度调整红通道。
        celestial.green * green_multiplier.max(0.0) * brightness, // 昼夜原始绿通道亮度关系不会被绝对目标色抬高。
        celestial.blue * blue_multiplier.max(0.0) * brightness, // 午夜蓝黑仍保持蓝黑，不会混成亮灰顶棚。
        celestial.alpha,                                        // 清屏色透明度沿用世界时间结果。
    ) // 返回同时适用于正式场景日夜和晨昏的最终清屏色。
} // 结束天气天空乘色与压暗。

/// 将世界时钟采样统一应用到所有显式标记的玩法渲染组件。
pub(crate) fn apply_world_lighting(
    mut commands: Commands, // 新生成的玩法主相机缺少 AmbientLight 时通过命令补齐。
    mut inputs: WorldLightingInputs, // 统一读取世界时间、天气修正和设置，并写回最终天空色。
    mut lights: Query<(
        &WorldCelestialLight,  // 保存每个场景作者配置的日间基准。
        &mut DirectionalLight, // 写入当前照度和颜色。
        &mut Transform,        // 写入太阳或月亮方向。
        &mut Visibility,       // 动态场景双灯保持稳定提取，避免昼夜交界反复拆建Bevy阴影视图。
        Option<&mut SunDisk>,  // 动态场景角直径写入光栅与Solari，并在夜间隐藏可见太阳盘。
    )>,
    mut cameras: Query<
        (Entity, Option<&mut AmbientLight>), // 已有环境光就更新，新增相机则延迟插入。
        Or<(With<PrimarySceneView>, With<WeatherPreviewCamera>)>, // 只接管玩法主相机和明确加入天气预览的F12相机。
    >,
    skybox_modes: Query<(
        &AuthoredSkyboxRequest,    // 场景模式声明。
        &AuthoredSkyboxStatus,     // 主 Cube 实际采用证据。
        Has<PrimarySceneView>,         // F12 关闭时选择正式玩法相机。
        Has<WeatherPreviewCamera>, // F12 打开时选择作者相机。
    )>,
    mut skyboxes: Query<(&WorldSkyProfile, &mut Skybox)>, // 只调整显式标记且真实存在的作者天空盒。
    mut environment_maps: Query<(
        &WorldEnvironmentMapProfile, // IBL补光与反射保存独立作者基准，不能借当前帧结果连续累乘。
        &mut EnvironmentMapLight,    // 直接写真正参与PBR的环境贴图，而非一次性生成描述。
    )>,
) {
    let sample = lighting_sample(&inputs.clock); // 每帧只计算一次天文和昼夜曲线。
    let preview_active = inputs
        .preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 缺失资源的最小测试与 F12 关闭都选择玩法相机。
    let visible_skybox_active =
        skybox_modes
            .iter()
            .any(|(request, status, is_gameplay_camera, is_preview_camera)| {
                status.uses_visible_skybox(request) // 仅模式和真实采用证据同时成立。
                && if preview_active {
                    is_preview_camera // F12 打开时离屏视口决定当前可见背景。
                } else {
                    is_gameplay_camera // F12 关闭时正式窗口决定当前可见背景。
                }
            });
    let brightness_multiplier = inputs.settings.directional_light_brightness_multiplier; // 不施加旧版四倍隐藏上限。
    let mut day_clear_color = None; // 没有玩法天体光时不覆盖场景自行设置的清屏颜色。
    let mut uses_dynamic_day_night_factor = false; // 只要当前场景存在动态场景天体光，环境、动态天空底色和 IBL 就选择同一个昼夜系数。
    for (profile, mut light, mut transform, mut visibility, sun_disk) in &mut lights {
        let daylight_color = LOW_SUN_COLOR.mix(&profile.base_color, sample.sun_height_factor); // 低角度太阳保持暖橙，月亮灯不复用该颜色。
        let (natural_illuminance, celestial_color, natural_source_direction) =
            if profile.uses_authored_day_night_curve {
                match profile.kind {
                    WorldCelestialLightKind::Sun => (
                        profile.base_illuminance
                            * season_light_factor(inputs.clock.day_of_year())
                            * sample.sunlight_factor, // 太阳直射只按真实日间高度曲线升落，日出日落自然归零。
                        daylight_color, // 太阳颜色随真实太阳高度恢复作者日光色。
                        sample.sun_source_direction, // 太阳灯永远沿西落轨迹，不再向东方月亮方向旋转。
                    ),
                    WorldCelestialLightKind::Moon => (
                        profile.night_peak_illuminance * sample.moonlight_factor, // 月亮从月升零值连续升到午夜作者峰值，再向月落归零。
                        profile.night_color, // 月亮灯只使用独立夜间作者色，不与零照度太阳颜色混合。
                        sample.moon_source_direction, // 月亮灯和可见月亮共享同一自然方向。
                    ),
                    WorldCelestialLightKind::LegacyCombined => (
                        profile.night_peak_illuminance
                            + (profile.base_illuminance
                                * season_light_factor(inputs.clock.day_of_year())
                                - profile.night_peak_illuminance)
                                * sample.dynamic_day_night_factor, // 防御外部误构造时保留旧动态单灯亮度，不再用于正式动态场景。
                        profile
                            .night_color
                            .mix(&daylight_color, sample.dynamic_day_night_factor), // 兼容异常旧实体的连续色温。
                        sample.source_direction, // 异常旧实体只走固定兼容方向，禁止重新引入天顶交接。
                    ),
                }
            } else {
                let base_illuminance = if sample.is_day {
                    profile.base_illuminance // 固定场景白天继续使用原日间基准。
                } else {
                    profile.night_peak_illuminance // 固定场景夜间继续使用旧月光峰值。
                };
                let fixed_color = if sample.is_day {
                    daylight_color // 固定场景白天继续使用原低角度暖色过渡。
                } else {
                    profile.night_color // 固定场景夜间继续直接切换为旧冷蓝月光色。
                };
                (
                    base_illuminance * sample.direct_factor, // HN-01/HN-02 的零边界和午夜峰值保持不变。
                    fixed_color,                             // 固定场景颜色行为完整保留。
                    sample.source_direction, // 固定场景继续复用原单灯太阳/月亮方向。
                )
            };
        light.illuminance = (natural_illuminance
            * brightness_multiplier
            * inputs.environment.direct_light_factor.max(0.0))
        .max(0.0); // 天气透光率在现有场景、时段、季节和F1倍率之后合成，负值统一归零。
        light.color = celestial_color.mix(
            &inputs.environment.light_tint, // 阴雨天气使用低饱和冷灰目标色。
            inputs.environment.light_tint_strength.clamp(0.0, 1.0), // 防御外部环境系统写入越界混色比例。
        ); // 天气只混合光色，不改变太阳或月亮方向。
        let source_direction =
            Quat::from_rotation_y(profile.azimuth_offset_radians) * natural_source_direction; // 世界 Y 轴偏移只旋转最终水平朝向并保持高度角不变。
        transform.translation = source_direction * 100.0; // 保留非零位置供 looking_at 稳定计算方向。
        transform.rotation = Transform::from_translation(transform.translation)
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation; // 方向光局部 -Z 指向场景中心，阴影随天体轨迹旋转。
        if let Some(mut sun_disk) = sun_disk {
            let desired_angular_size = if profile.kind == WorldCelestialLightKind::Moon {
                MOON_ANGULAR_DIAMETER_RADIANS // 月亮视觉和光源范围使用固定零点五一八度。
            } else {
                profile.angular_diameter_radians // 太阳与固定场景继续使用作者或地球默认角直径。
            };
            if sun_disk.angular_size != desired_angular_size {
                sun_disk.angular_size = desired_angular_size; // 角直径变化同帧进入 Solari 光源范围和软阴影采样。
            }
            if profile.uses_authored_day_night_curve {
                let desired_disk_intensity = if !visible_skybox_active
                    && profile.kind == WorldCelestialLightKind::Sun
                    && sample.sunlight_factor > 0.0
                {
                    1.0 // 只有真实太阳灯在地平线上方时允许 Bevy 大气显示太阳盘。
                } else {
                    0.0 // 天空盒、月亮灯和夜间太阳灯都不生成第二个大气圆盘。
                };
                if sun_disk.intensity != desired_disk_intensity {
                    sun_disk.intensity = desired_disk_intensity; // 太阳盘显隐与独立太阳灯时段保持一致。
                }
            }
        }
        if profile.uses_authored_day_night_curve {
            if *visibility != Visibility::Inherited {
                *visibility = Visibility::Inherited; // 两盏灯始终保留稳定渲染身份，零照度灯不会产生直射贡献。
            }
            let active = light.illuminance > ACTIVE_CELESTIAL_LIGHT_EPSILON_LUX; // 最终天气和F1修正后判断当前天体是否具有实际贡献。
            light.contact_shadows_enabled = active; // 零照度灯不执行逐像素接触阴影，但不触碰标准级联阴影作者资格。
            // `shadow_maps_enabled`由生成时作者值和全局阴影画质系统共同拥有；这里禁止改写，
            // 否则白天首帧会把月亮记录成“作者禁用阴影”，夜间永远无法恢复玩家级联阴影，
            // 还会在时间拖动时反复移除/重建Shadow views并触发Bevy 0.19.1 GPU预处理批次数量断言。
        }
        uses_dynamic_day_night_factor |= profile.uses_authored_day_night_curve; // 动态场景双直射灯之外的环境、动态天空底色和 IBL 必须采用同一个昼夜进度。
        day_clear_color.get_or_insert(profile.day_clear_color); // 单场景首盏玩法天体光提供清屏日景基准。
    }
    let environment_factor = if uses_dynamic_day_night_factor {
        sample.dynamic_day_night_factor // 动态场景环境、动态天空底色和 IBL 只读取午夜零、正午一的统一连续系数。
    } else {
        sample.environment_day_factor // HN-01、HN-02继续使用原五至七点和十七至十九点曲线。
    }; // 为清屏色、环境光色和所有亮度通道选择唯一场景曲线。
    let sky_brightness_factor = if uses_dynamic_day_night_factor {
        NIGHT_SKY_BRIGHTNESS_FACTOR + (1.0 - NIGHT_SKY_BRIGHTNESS_FACTOR) * environment_factor // 动态天空回退底色、漫反射补光和镜面反射共用同一最终倍率。
    } else {
        sample.sky_brightness_factor // 固定场景保留迁移前的原天空倍率。
    }; // 完成当前场景动态天空回退底色与 IBL 亮度解析。
    let base_ambient_brightness = if uses_dynamic_day_night_factor {
        NIGHT_AMBIENT_BRIGHTNESS
            + (DAY_AMBIENT_BRIGHTNESS - NIGHT_AMBIENT_BRIGHTNESS) * environment_factor // 动态环境光与天空使用同一系数，避免十九点单独跌到最低值。
    } else {
        sample.ambient_brightness // 固定场景保留迁移前的相机环境光亮度。
    }; // 完成天气修正前的场景环境光基准。
    if let Some(day_color) = day_clear_color {
        let base_clear_color = NIGHT_CLEAR_COLOR.mix(&day_color, environment_factor); // 动态场景到午夜才完全进入夜景，固定场景继续保留旧晨昏窗口。
        let dawn_clear_color = base_clear_color.mix(
            &DAWN_CLEAR_COLOR,                             // 朝霞使用较柔和的暖桃色。
            sample.dawn_factor * TWILIGHT_CLEAR_COLOR_MIX, // 六点达到朝霞最大覆盖比例。
        );
        let celestial_clear_color = dawn_clear_color.mix(
            &DUSK_CLEAR_COLOR,                             // 夕阳使用更浓的橙红色。
            sample.dusk_factor * TWILIGHT_CLEAR_COLOR_MIX, // 十八点达到夕阳最大覆盖比例。
        ); // 先保留世界时间权威的日夜、朝霞和夕阳连续结果。
        inputs.clear_color.0 = apply_sky_modifiers(
            celestial_clear_color,                // 先保留完整日夜、朝霞和夕阳连续结果。
            inputs.environment.sky_tint, // 阴雨与薄雾以乘色方式改变色温，不使用绝对亮色覆盖夜景。
            inputs.environment.sky_tint_strength, // 云量决定中性白到天气色温的连续乘色比例。
            inputs.environment.sky_brightness_factor, // 动态大气回退底色与环境 IBL 同步接受天气压暗倍率。
        ); // 天气在晨昏结果之后叠加，同时保持夜间相对亮度和暖色天光。
    }
    let neutral_ambient_color = MOONLIGHT_COLOR.mix(&Color::WHITE, environment_factor); // 环境光色温与当前场景统一明暗曲线同步过渡。
    let twilight_ambient_factor =
        (sample.dawn_factor + sample.dusk_factor).clamp(0.0, 1.0) * TWILIGHT_AMBIENT_COLOR_MIX; // 六点和十八点加入轻微暖色，窗口之外保持原色。
    let celestial_ambient_color =
        neutral_ambient_color.mix(&LOW_SUN_COLOR, twilight_ambient_factor); // 朝霞和夕阳为角色及地表补充柔和暖色氛围。
    let ambient_color = celestial_ambient_color.mix(
        &inputs.environment.light_tint, // 让环境光与阴雨天空使用一致的低饱和色温。
        inputs.environment.light_tint_strength.clamp(0.0, 1.0), // 与方向光共用连续天气混色比例。
    ); // 完成昼夜、晨昏与天气三层环境光色合成。
    let ambient_brightness = (base_ambient_brightness
        * inputs.environment.ambient_light_factor.max(0.0))
    .max(inputs.environment.minimum_ambient_brightness.max(0.0)); // 阴雨可压低环境光，但夜间始终保留天气指定的可玩性下限。
    for (entity, ambient) in &mut cameras {
        if let Some(mut ambient) = ambient {
            ambient.color = ambient_color; // 日出日落附近也连续插值，避免色温在边界瞬间跳变。
            ambient.brightness = ambient_brightness; // 白天和夜晚都叠加连续天气倍率与最低可玩亮度。
            ambient.affects_lightmapped_meshes = true; // 当前项目没有把环境光重复烘焙进光照贴图。
        } else {
            commands.entity(entity).insert(AmbientLight {
                color: ambient_color,             // 新相机首帧使用当前昼夜连续色温。
                brightness: ambient_brightness,   // 新相机立即获得包含天气修正的最终环境亮度。
                affects_lightmapped_meshes: true, // 与后续逐帧更新保持同一光照贴图策略。
            });
        }
    }
    for (profile, mut skybox) in &mut skyboxes {
        skybox.brightness = profile.base_brightness
            * sky_brightness_factor
            * inputs.environment.sky_brightness_factor.max(0.0); // 可见 Cube 与 IBL 共享昼夜及天气压暗倍率，避免阴天背景和地物反射脱节。
    }
    for (profile, mut environment_map) in &mut environment_maps {
        environment_map.intensity = profile.carrier_base_intensity()
            * sky_brightness_factor
            * inputs.environment.sky_brightness_factor.max(0.0); // 两个独立作者值通过共同载体同步变暗，比例在漫反射图中保持不变。
    }
}

#[cfg(test)] // ECS 测试验证渲染组件写入和测试倍率行为。
mod tests {
    use super::super::helpers::dynamic_day_night_factor; // 断言公开环境昼夜系数与最终ECS写入完全一致。
    use super::*; // 测试直接注册内部光照应用系统。
    use crate::{WorldTimeSnapshot, time::WorldEnvironmentMapProfile}; // 构造环境反射日间基准和指定时刻快照。

    /// 使用统一线性色彩空间和容差比较两种可能采用不同存储空间的颜色。
    fn assert_color_close(actual: Color, expected: Color) {
        let actual = actual.to_linear(); // 把系统混色结果转换到渲染使用的线性空间。
        let expected = expected.to_linear(); // 把期望颜色转换到同一个线性空间。
        let epsilon = 0.000_001; // 只容忍颜色空间往返产生的浮点舍入误差。
        assert!((actual.red - expected.red).abs() < epsilon); // 红色通道必须近似相等。
        assert!((actual.green - expected.green).abs() < epsilon); // 绿色通道必须近似相等。
        assert!((actual.blue - expected.blue).abs() < epsilon); // 蓝色通道必须近似相等。
        assert!((actual.alpha - expected.alpha).abs() < epsilon); // 透明度通道必须近似相等。
    }

    /// 构建只包含统一光照系统的测试应用。
    fn lighting_app(hour: f32, multiplier: f32) -> App {
        let mut app = App::new(); // 使用最小 ECS 世界验证组件同步，不初始化渲染器。
        app.insert_resource(WorldClock::from_snapshot(WorldTimeSnapshot {
            month: 6,                              // 使用六月以覆盖高角度夏季太阳。
            day: 15,                               // 使用月中日期与默认世界时钟一致。
            seconds_since_midnight: hour * 3600.0, // 将测试小时换算为当日秒数。
        }));
        let settings = EnvironmentPreviewSettings {
            directional_light_brightness_multiplier: multiplier, // 写入被测无限制倍率。
            ..default() // 其余正式设置保持默认值，避免测试绑定无关字段。
        }; // 从正式默认设置构造测试配置。
        app.insert_resource(settings); // 光照系统每帧读取该资源。
        app.insert_resource(WorldEnvironmentModifiers::default()); // 默认中性修正确保原有世界时间测试结果保持不变。
        app.insert_resource(ClearColor(Color::BLACK)); // 提供被测清屏颜色资源。
        app.add_systems(Update, apply_world_lighting); // 只注册统一光照应用系统。
        app // 返回可继续生成场景实体的测试应用。
    }

    #[test] // 超过旧版四倍上限的 F1 倍率必须原样进入最终照度。
    fn directional_light_accepts_unbounded_positive_multiplier() {
        let mut app = lighting_app(12.0, 6.5); // 六月正午并使用六点五倍测试亮度。
        let light_entity = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    illuminance: 30_000.0, // 初始值会被统一系统重算。
                    ..default()            // 其他方向光字段沿用 Bevy 默认值。
                },
                WorldCelestialLight::new(30_000.0, Color::WHITE, Color::srgb(0.5, 0.7, 1.0)), // 保存作者日景基准。
            ))
            .id(); // 保存实体用于更新后读取。
        app.update(); // 应用正午季节系数和测试倍率。
        let light = app.world().get::<DirectionalLight>(light_entity).unwrap(); // 读取更新后的方向光。
        assert!(light.illuminance > 30_000.0 * 6.0); // 六点五倍不得被旧版四倍上限截断。
    }

    #[test] // 午夜应同时更新冷月光、环境光、动态天空回退底色和环境 IBL。
    fn midnight_updates_all_gameplay_lighting_channels() {
        let mut app = lighting_app(0.0, 1.0); // 构造六月午夜正式倍率应用。
        let light_entity = app
            .world_mut()
            .spawn((
                DirectionalLight::default(), // 方向光由统一系统写入最终照度和颜色。
                WorldCelestialLight::new(20_000.0, Color::WHITE, Color::srgb(0.6, 0.8, 1.0)), // 保存日间基准。
            ))
            .id(); // 保存方向光实体。
        let camera_entity = app
            .world_mut()
            .spawn((
                PrimarySceneView,                                       // 标记为玩法主相机。
                AmbientLight::default(), // 预先插入环境光以便同一帧直接更新。
                EnvironmentMapLight::default(), // 提供真正参与PBR的环境反射写入目标。
                WorldEnvironmentMapProfile::new(3_000.0, 10_000.0), // 补光较低但公共载体保持完整镜面基准。
            ))
            .id(); // 保存相机实体用于断言。
        app.update(); // 应用午夜光照采样。
        let light = app.world().get::<DirectionalLight>(light_entity).unwrap(); // 读取午夜方向光。
        let ambient = app.world().get::<AmbientLight>(camera_entity).unwrap(); // 读取玩法相机独立环境光。
        let environment_map = app
            .world()
            .get::<EnvironmentMapLight>(camera_entity)
            .unwrap(); // 读取午夜环境反射强度。
        assert!((light.illuminance - 800.0).abs() < 0.1); // 午夜月光等于两万日光基准的百分之四。
        assert_color_close(light.color, MOONLIGHT_COLOR); // 午夜方向光必须切换为固定冷蓝色。
        assert_eq!(ambient.brightness, 8.0); // 午夜玩法环境光必须降到八。
        assert!((environment_map.intensity - 200.0).abs() < 0.1); // 环境 IBL 必须降至日间镜面基准的百分之二，不能保留正午高光。
        assert_color_close(app.world().resource::<ClearColor>().0, NIGHT_CLEAR_COLOR); // 午夜清屏色必须为深蓝黑。
    }

    #[test] // 动态太阳灯与月亮灯必须分别消费作者值和轨迹，并始终保留可恢复的标准阴影资格。
    fn authored_dynamic_lights_use_independent_values_directions_and_disks() {
        let mut app = lighting_app(12.0, 1.0); // 从正午开始验证月亮零照度但渲染与阴影身份保持稳定。
        let day_color = Color::srgb(0.8, 0.7, 0.6); // 使用可辨识日光色检查太阳灯字段选择。
        let night_color = Color::srgb(0.1, 0.2, 0.3); // 使用可辨识月光色检查月亮灯字段选择。
        let day_clear_color = Color::srgb(0.2, 0.4, 0.8); // 两盏灯共享同一场景日景基准。
        let sun_entity = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    shadow_maps_enabled: true,     // 生成阶段授予太阳标准级联阴影作者资格。
                    contact_shadows_enabled: true, // 活动状态系统只允许按照度切换接触阴影。
                    ..default()                    // 其余方向光字段使用Bevy安全默认值。
                }, // 太阳最终照度和颜色由统一系统写入，标准阴影资格不得被改写。
                Transform::default(), // 初始方向留空以验证真实太阳轨迹写入。
                SunDisk::EARTH,       // 作者角直径应覆盖地球默认值。
                WorldCelestialLight::new_authored(
                    WorldCelestialLightKind::Sun, // 第一实体只承担太阳职责。
                    10_000.0,                     // 日间作者峰值。
                    day_color,                    // 日间作者颜色。
                    123.0,                        // 夜间字段仅为完整配置保留，太阳灯不消费。
                    night_color,                  // 夜间颜色仅为完整配置保留。
                    90.0_f32.to_radians(),        // 同时验证方位角偏移。
                    1.25_f32.to_radians(),        // 太阳作者角直径。
                    day_clear_color,              // 场景日间清屏色。
                ),
            ))
            .id(); // 保存太阳实体供昼夜切换后读取。
        let moon_entity = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    shadow_maps_enabled: true, // 月亮生成首帧就必须拥有标准级联阴影作者资格。
                    contact_shadows_enabled: true, // 白天零照度时仅临时关闭接触阴影。
                    ..default()                // 其余方向光字段使用Bevy安全默认值。
                }, // 月亮最终照度和颜色由统一系统写入，标准阴影资格必须跨昼夜保留。
                Transform::default(), // 初始方向留空以验证真实月亮轨迹写入。
                SunDisk::OFF,         // 月亮不显示第二个大气太阳盘。
                WorldCelestialLight::new_authored(
                    WorldCelestialLightKind::Moon, // 第二实体只承担月亮职责。
                    10_000.0,                      // 日间字段只为完整配置保留。
                    day_color,                     // 日光色不进入月亮灯。
                    123.0,                         // 午夜月亮作者峰值。
                    night_color,                   // 独立月光作者色。
                    90.0_f32.to_radians(),         // 与太阳共享水平偏移。
                    1.25_f32.to_radians(),         // 月亮运行时应忽略太阳作者角直径。
                    day_clear_color,               // 两盏灯共享场景日景基准。
                ),
            ))
            .id(); // 保存月亮实体供夜间断言。

        let noon_sample = lighting_sample(app.world().resource::<WorldClock>()); // 取得六月正午太阳季节与高度系数。
        app.update(); // 同帧应用双方向光最终状态。
        let noon_sun = app.world().get::<DirectionalLight>(sun_entity).unwrap(); // 读取正午太阳灯。
        let noon_moon = app.world().get::<DirectionalLight>(moon_entity).unwrap(); // 读取正午月亮灯。
        assert!(
            (noon_sun.illuminance
                - 10_000.0
                    * season_light_factor(app.world().resource::<WorldClock>().day_of_year())
                    * noon_sample.sunlight_factor)
                .abs()
                < 0.01
        ); // 太阳只消费日间峰值、季节和太阳高度曲线。
        assert_eq!(noon_moon.illuminance, 0.0); // 白天月亮灯必须无贡献。
        assert_eq!(
            *app.world().get::<Visibility>(sun_entity).unwrap(),
            Visibility::Inherited
        ); // 正午太阳灯进入渲染提取。
        assert_eq!(
            *app.world().get::<Visibility>(moon_entity).unwrap(),
            Visibility::Inherited
        ); // 正午月亮灯保留稳定渲染身份，零照度保证其没有画面贡献。
        assert!(noon_sun.shadow_maps_enabled); // 正午太阳保留生成时作者标准阴影资格。
        assert!(noon_sun.contact_shadows_enabled); // 正午太阳有贡献时允许相机接触阴影。
        assert!(noon_moon.shadow_maps_enabled); // 白天不得把月亮标准阴影资格记录为作者关闭。
        assert!(!noon_moon.contact_shadows_enabled); // 白天零照度月亮跳过无效逐像素接触阴影。

        app.world_mut().resource_mut::<WorldClock>().set_hour(0.0); // 切到月亮夜间弧线最高点与照度峰值。
        let midnight_sample = lighting_sample(app.world().resource::<WorldClock>()); // 读取与可见月亮共享的权威方向。
        app.update(); // 同帧把太阳照度归零并启用月亮灯，双灯渲染身份保持不变。
        let midnight_sun = app.world().get::<DirectionalLight>(sun_entity).unwrap(); // 读取午夜太阳灯零贡献状态。
        let midnight_moon = app.world().get::<DirectionalLight>(moon_entity).unwrap(); // 读取午夜月亮灯峰值状态。
        assert_eq!(midnight_sun.illuminance, 0.0); // 夜间太阳灯不得保留直射或水面高光。
        assert!((midnight_moon.illuminance - 123.0).abs() < 0.001); // 午夜精确达到独立月亮峰值。
        assert_color_close(midnight_moon.color, night_color); // 月亮灯必须使用独立夜间作者色。
        assert!(midnight_sun.shadow_maps_enabled); // 零照度太阳保留稳定阴影视图，避免昼夜拖动触发Bevy批次崩溃。
        assert!(!midnight_sun.contact_shadows_enabled); // 午夜太阳不执行无效接触阴影。
        assert!(midnight_moon.shadow_maps_enabled); // 月亮必须真正生成可投射玩家影子的标准级联阴影。
        assert!(midnight_moon.contact_shadows_enabled); // 午夜月亮有贡献时允许近距离接触阴影补强。
        assert_eq!(
            *app.world().get::<Visibility>(sun_entity).unwrap(),
            Visibility::Inherited
        ); // 午夜太阳仍保留稳定提取身份，照度零确保不照亮场景。
        assert_eq!(
            *app.world().get::<Visibility>(moon_entity).unwrap(),
            Visibility::Inherited
        ); // 午夜月亮无需重新创建渲染与级联阴影视图即可立即投影。
        let moon_transform = app.world().get::<Transform>(moon_entity).unwrap(); // 读取月亮灯最终世界方向。
        let expected_source =
            Quat::from_rotation_y(90.0_f32.to_radians()) * midnight_sample.moon_source_direction; // 月亮灯与可见月亮共享方向并接受作者方位偏移。
        assert!(
            moon_transform
                .translation
                .normalize()
                .distance(expected_source)
                < 0.000_01
        ); // Y 轴偏移不改变月亮高度角。
        assert!((*moon_transform.forward()).distance(-expected_source) < 0.000_01); // 玩家影子必须沿反月亮方向延伸。
        let sun_disk = app.world().get::<SunDisk>(sun_entity).unwrap(); // 读取太阳作者光源范围。
        let moon_disk = app.world().get::<SunDisk>(moon_entity).unwrap(); // 读取固定月亮光源范围。
        assert_eq!(sun_disk.angular_size, 1.25_f32.to_radians()); // 太阳继续响应 F12 作者角直径。
        assert_eq!(sun_disk.intensity, 0.0); // 午夜太阳盘必须隐藏。
        assert_eq!(moon_disk.angular_size, MOON_ANGULAR_DIAMETER_RADIANS); // 月亮软阴影范围与可见月亮固定角直径一致。
        assert_eq!(moon_disk.intensity, 0.0); // 月亮灯不生成 Bevy 大气中的第二个太阳盘。
    }

    #[test] // 十八点半太阳必须留在西方且熄灭，东方低月亮独立投影，双灯阴影视图保持稳定。
    fn dynamic_dusk_uses_eastern_moon_without_stale_sun_contribution_or_fixed_scene_changes() {
        let mut app = lighting_app(18.5, 1.0); // 用户截图时刻用于锁定水面高光和玩家影子的共同根因。
        let dynamic_sun_entity = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    shadow_maps_enabled: true,     // 动态太阳生成时具备标准级联阴影作者资格。
                    contact_shadows_enabled: true, // 接触阴影按最终照度单独启停。
                    ..default()                    // 其余字段保持Bevy默认值。
                }, // 世界时间只接管照度、颜色、方向和接触阴影，不改标准阴影作者资格。
                Transform::default(), // 初始方向故意留空，确保断言读取本帧真实西侧结果。
                SunDisk::EARTH,       // 动态太阳盘必须在日落后隐藏。
                WorldCelestialLight::new_authored(
                    WorldCelestialLightKind::Sun,  // 该实体只沿太阳轨迹运行。
                    9_000.0,                       // 使用正式动态场景默认日间照度。
                    Color::srgb(1.0, 0.93, 0.82),  // 使用正式动态场景默认日光颜色。
                    1_000.0,                       // 月亮峰值只为完整配置保留。
                    Color::srgb(0.42, 0.52, 0.78), // 使用正式动态场景默认夜光颜色。
                    0.0,                           // 排除方位角偏移对方向断言的干扰。
                    0.533_f32.to_radians(),        // 角直径与本测试无关但保持正式默认值。
                    Color::srgb(0.4, 0.7, 1.0),    // 提供玩法日景清屏色。
                ), // 动态场景太阳灯不再承担月亮职责。
            ))
            .id(); // 保存动态场景太阳实体供更新后读取。
        let dynamic_moon_entity = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    shadow_maps_enabled: true,     // 动态月亮从生成首帧就保留玩家级联阴影资格。
                    contact_shadows_enabled: true, // 接触阴影按最终照度单独启停。
                    ..default()                    // 其余字段保持Bevy默认值。
                }, // 世界时间不得在白天把月亮标准阴影资格永久写成关闭。
                Transform::default(), // 初始方向留空以验证东方低空月亮写入。
                SunDisk::OFF,         // 独立月亮视觉负责圆盘画面。
                WorldCelestialLight::new_authored(
                    WorldCelestialLightKind::Moon, // 该实体只沿月亮轨迹运行。
                    9_000.0,                       // 日间字段仅保持配置完整。
                    Color::srgb(1.0, 0.93, 0.82),  // 日光色不进入月亮灯。
                    1_000.0,                       // 月亮午夜峰值为一千 lux。
                    Color::srgb(0.42, 0.52, 0.78), // 月亮使用独立冷色。
                    0.0,                           // 排除方位角偏移干扰。
                    0.533_f32.to_radians(),        // 太阳角字段不覆盖固定月亮角直径。
                    Color::srgb(0.4, 0.7, 1.0),    // 与太阳共享场景日景基准。
                ), // 动态场景月亮灯与可见月亮保持同向。
            ))
            .id(); // 保存动态场景月亮实体供更新后读取。
        let fixed_entity = app
            .world_mut()
            .spawn((
                DirectionalLight::default(), // 固定场景方向光仍由同一系统写入。
                Transform::default(),        // 初始变换与动态实体一致，便于直接比较最终方向。
                SunDisk::EARTH,              // 固定场景光盘不得被动态场景显隐规则改写。
                WorldCelestialLight::new(
                    9_000.0,                    // 固定场景保留旧日间作者照度。
                    Color::WHITE,               // 固定场景日光颜色不参与方向选择。
                    Color::srgb(0.4, 0.7, 1.0), // 固定场景日景清屏色保持原入口。
                ), // 旧构造器必须继续选择瞬切但交界零照度的兼容方向。
            ))
            .id(); // 保存固定场景方向光实体供更新后读取。
        let sample = lighting_sample(app.world().resource::<WorldClock>()); // 取得同一时刻独立太阳、月亮和固定兼容方向。

        app.update(); // 同帧把三种明确职责各自的方向、照度和稳定渲染身份写入组件。

        let dynamic_sun_direction = app
            .world()
            .get::<Transform>(dynamic_sun_entity)
            .unwrap()
            .translation
            .normalize(); // 读取动态场景太阳灯最终单位方向。
        let dynamic_moon_direction = app
            .world()
            .get::<Transform>(dynamic_moon_entity)
            .unwrap()
            .translation
            .normalize(); // 读取动态场景月亮灯最终单位方向。
        let fixed_direction = app
            .world()
            .get::<Transform>(fixed_entity)
            .unwrap()
            .translation
            .normalize(); // 读取固定场景最终单位方向。
        let dynamic_disk = app.world().get::<SunDisk>(dynamic_sun_entity).unwrap(); // 读取日落后的动态场景太阳盘。
        let fixed_disk = app.world().get::<SunDisk>(fixed_entity).unwrap(); // 读取相同时刻的固定场景兼容光盘。
        assert!(dynamic_sun_direction.distance(sample.sun_source_direction) < 0.000_01); // 零照度太阳必须留在西侧地平线，不能飞到天顶。
        assert!(dynamic_moon_direction.distance(sample.moon_source_direction) < 0.000_01); // 活动方向光必须与东方可见月亮精确同向。
        assert!(dynamic_sun_direction.y < 0.01); // 日落后的太阳端点接近地平线。
        assert!(dynamic_moon_direction.y < 0.5); // 十八点半刚升起的月亮不能位于头顶。
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(dynamic_sun_entity)
                .unwrap()
                .illuminance,
            0.0
        ); // 太阳熄灭后不能继续制造水面高光或玩家影子。
        assert!(
            app.world()
                .get::<DirectionalLight>(dynamic_moon_entity)
                .unwrap()
                .illuminance
                > 0.0
        ); // 东方月亮从低照度连续升起。
        assert_eq!(
            *app.world().get::<Visibility>(dynamic_sun_entity).unwrap(),
            Visibility::Inherited
        ); // 熄灭太阳保留稳定提取身份，由零照度保证不产生直射、反射高光或阴影暗化。
        assert_eq!(
            *app.world().get::<Visibility>(dynamic_moon_entity).unwrap(),
            Visibility::Inherited
        ); // 月亮无需重建Shadow views即可成为当前唯一有照度的阴影方向光。
        assert!(
            app.world()
                .get::<DirectionalLight>(dynamic_sun_entity)
                .unwrap()
                .shadow_maps_enabled
        ); // 零照度太阳仍保留稳定标准阴影视图，避免时间拖动触发Bevy GPU批次断言。
        assert!(
            app.world()
                .get::<DirectionalLight>(dynamic_moon_entity)
                .unwrap()
                .shadow_maps_enabled
        ); // 东方月亮必须拥有实际可投射玩家影子的标准级联阴影。
        assert!(fixed_direction.distance(sample.source_direction) < 0.000_01); // 固定场景必须继续采用旧方向。
        assert_eq!(dynamic_disk.intensity, 0.0); // 动态场景日落后不得看见太阳盘横穿天空。
        assert_eq!(fixed_disk.intensity, SunDisk::EARTH.intensity); // 固定场景光盘强度保持迁移前行为。

        app.world_mut().resource_mut::<WorldClock>().set_hour(17.0); // 回到日落前验证太阳盘会正常恢复。
        app.update(); // 同帧应用白天太阳盘显隐状态。
        assert_eq!(
            app.world()
                .get::<SunDisk>(dynamic_sun_entity)
                .unwrap()
                .intensity,
            1.0
        ); // 日落前动态场景仍显示真实太阳。
        assert_eq!(
            *app.world().get::<Visibility>(dynamic_sun_entity).unwrap(),
            Visibility::Inherited
        ); // 日落前太阳灯恢复当前唯一有照度的活动职责。
        assert_eq!(
            *app.world().get::<Visibility>(dynamic_moon_entity).unwrap(),
            Visibility::Inherited
        ); // 日落前月亮保持稳定提取身份，但零照度不会产生阴影暗化或反射高光。
    }

    #[test] // 十九点环境仍应明显亮于午夜，月亮直射则按真实夜间高度继续升向午夜峰值。
    fn dynamic_nineteen_keeps_twilight_environment_above_midnight_while_moon_rises() {
        let mut app = lighting_app(19.0, 1.0); // 从用户指出过暗的十九点验证环境亮度和月亮直射的职责分离。
        let light_entity = app
            .world_mut()
            .spawn((
                DirectionalLight::default(), // 方向光用于比较十九点与午夜的最终照度。
                Transform::default(),        // 动态天体方向由统一系统同步，确保场景类型被正确识别。
                WorldCelestialLight::new_authored(
                    WorldCelestialLightKind::Moon, // 十九点只有东方月亮灯具有直射贡献。
                    9_000.0,                       // 使用正式动态场景默认日间照度。
                    Color::srgb(1.0, 0.93, 0.82),  // 使用正式动态场景默认日光色。
                    1_000.0,                       // 使用正式动态场景月亮午夜峰值。
                    Color::srgb(0.42, 0.52, 0.78), // 使用正式动态场景夜光色。
                    0.0,                           // 排除方位角对亮度断言的干扰。
                    0.533_f32.to_radians(),        // 使用正式默认太阳角直径。
                    Color::srgb(0.4, 0.7, 1.0),    // 提供易于比较的日间天空色。
                ), // 动态构造器让月亮直射与环境连续昼夜曲线分别解析。
            ))
            .id(); // 保存方向光实体以读取最终照度。
        let camera_entity = app
            .world_mut()
            .spawn((
                PrimarySceneView,                                    // 标记为玩法相机以接收统一环境光。
                AmbientLight::default(),                         // 提供环境光写入目标。
                EnvironmentMapLight::default(), // 提供PBR天空补光与反射公共写入目标。
                WorldEnvironmentMapProfile::new(300.0, 1_000.0), // 公共强度载体使用一千基准。
            ))
            .id(); // 保存相机实体以读取环境光与 IBL 两个通道。

        let day_night_factor = dynamic_day_night_factor(app.world().resource::<WorldClock>()); // 取得运行时和F12共同使用的十九点唯一系数。
        let expected_evening_light =
            1_000.0 * lighting_sample(app.world().resource::<WorldClock>()).moonlight_factor; // 月亮直射按东方低空高度从零向午夜峰值增加。
        let expected_evening_ambient = NIGHT_AMBIENT_BRIGHTNESS
            + (DAY_AMBIENT_BRIGHTNESS - NIGHT_AMBIENT_BRIGHTNESS) * day_night_factor; // 环境光使用相同系数解析自己的日夜端点。
        let expected_evening_ibl_factor =
            NIGHT_SKY_BRIGHTNESS_FACTOR + (1.0 - NIGHT_SKY_BRIGHTNESS_FACTOR) * day_night_factor; // 动态天空回退底色与 IBL 使用相同系数解析百分之二夜间端点。
        app.update(); // 应用十九点动态场景完整光照结果。
        let evening_light = app
            .world()
            .get::<DirectionalLight>(light_entity)
            .unwrap()
            .illuminance; // 保存十九点方向光照度。
        let evening_ambient = app
            .world()
            .get::<AmbientLight>(camera_entity)
            .unwrap()
            .brightness; // 保存十九点环境光亮度。
        let evening_reflection = app
            .world()
            .get::<EnvironmentMapLight>(camera_entity)
            .unwrap()
            .intensity; // 保存十九点PBR天空补光与反射强度。

        app.world_mut().resource_mut::<WorldClock>().set_hour(0.0); // 切到同日曲线的午夜最低点。
        app.update(); // 应用午夜动态场景完整光照结果。
        let midnight_light = app
            .world()
            .get::<DirectionalLight>(light_entity)
            .unwrap()
            .illuminance; // 读取午夜月亮方向光峰值照度。
        let midnight_ambient = app
            .world()
            .get::<AmbientLight>(camera_entity)
            .unwrap()
            .brightness; // 读取午夜环境光最低亮度。
        let midnight_reflection = app
            .world()
            .get::<EnvironmentMapLight>(camera_entity)
            .unwrap()
            .intensity; // 读取午夜PBR天空补光与反射最低强度。

        assert!((evening_light - expected_evening_light).abs() < 0.01); // 十九点月亮只按自身夜间高度曲线解析。
        assert!((evening_ambient - expected_evening_ambient).abs() < 0.001); // 环境光必须由同一十九点系数解析。
        assert!((evening_reflection - 1_000.0 * expected_evening_ibl_factor).abs() < 0.01); // PBR 补光与反射公共载体必须同步使用同一系数。
        assert!((midnight_light - 1_000.0).abs() < 0.001); // 午夜月亮方向光精确达到默认夜间峰值。
        assert!(evening_light < midnight_light); // 月亮从东方升起后应继续增强，而不是整夜固定照度。
        assert!(evening_ambient > midnight_ambient); // 十九点暮光环境仍明显亮于午夜，避免总画面反向变亮。
        assert!(evening_reflection > midnight_reflection); // 环境 IBL 在十九点保持比午夜更亮的暮光余量。
        assert!((midnight_ambient - NIGHT_AMBIENT_BRIGHTNESS).abs() < 0.001); // 午夜环境光精确采用夜间端点。
        assert!((midnight_reflection - 1_000.0 * NIGHT_SKY_BRIGHTNESS_FACTOR).abs() < 0.01); // 午夜环境 IBL 保留百分之二基础值。
    }

    #[test] // 新生成且尚无环境光组件的玩法相机必须在首帧被即时接管。
    fn newly_spawned_gameplay_camera_receives_independent_ambient_light() {
        let mut app = lighting_app(12.0, 1.0); // 构造六月正午光照环境。
        let camera_entity = app.world_mut().spawn(PrimarySceneView).id(); // 模拟场景刚生成且未手工附加AmbientLight的玩法相机。
        app.update(); // 运行统一光照系统并应用延迟组件插入命令。
        let ambient = app
            .world()
            .get::<AmbientLight>(camera_entity)
            .expect("玩法相机首帧结束后必须拥有独立环境光"); // 读取世界时间系统新插入的相机环境光。
        assert_eq!(ambient.brightness, 80.0); // 正午环境亮度必须达到设计基准八十。
        assert_color_close(ambient.color, Color::WHITE); // 白天环境光保持中性，不污染材质色相。
    }

    #[test] // 天气等环境系统只能通过通用修正影响最终玩法光照，且所有通道必须使用同一帧输出。
    fn environment_modifiers_affect_light_ambient_skybox_ibl_and_clear_color() {
        let mut app = lighting_app(12.0, 1.0); // 使用六月正午排除晨昏色和夜间曲线干扰。
        let light_tint = Color::srgb(0.42, 0.48, 0.56); // 模拟阴雨天气的低饱和冷灰光色。
        let sky_tint = Color::linear_rgb(0.50, 0.62, 0.78); // 模拟厚云覆盖后的冷灰蓝线性乘色。
        app.insert_resource(WorldEnvironmentModifiers {
            direct_light_factor: 0.5,        // 方向光降低到未修正结果的一半。
            ambient_light_factor: 0.25,      // 正午环境光八十降低到二十。
            minimum_ambient_brightness: 5.0, // 当前二十高于保底，不应触发钳制。
            sky_brightness_factor: 0.4,      // 动态天空回退底色与 IBL 保留四成亮度。
            light_tint,                      // 所有玩法光照统一混向冷灰色。
            light_tint_strength: 1.0,        // 满强度便于精确断言最终色。
            sky_tint,                        // 清屏天空统一乘入厚云冷灰色温。
            sky_tint_strength: 1.0,          // 满强度便于精确断言最终乘色。
        }); // 用非中性资源覆盖测试应用中的默认修正。
        let light_entity = app
            .world_mut()
            .spawn((
                DirectionalLight::default(), // 最终照度与光色由世界时间系统唯一写入。
                WorldCelestialLight::new(20_000.0, Color::WHITE, Color::srgb(0.4, 0.7, 1.0)), // 保存场景作者日景基准。
            ))
            .id(); // 保存方向光实体供更新后断言。
        let camera_entity = app
            .world_mut()
            .spawn((
                PrimarySceneView,                                       // 标记为玩法主相机。
                AmbientLight::default(),                            // 提供环境光写入目标。
                Skybox::default(),              // 提供天空盒模式可见亮度写入目标。
                WorldSkyProfile::new(10_000.0), // 保存作者天空盒日间基准。
                EnvironmentMapLight::default(), // 提供环境贴图强度写入目标。
                WorldEnvironmentMapProfile::new(3_000.0, 10_000.0), // 两个环境通道仍共享天气连续倍率。
            ))
            .id(); // 保存相机实体供更新后断言。
        let expected_direct_illuminance =
            20_000.0 * lighting_sample(app.world().resource::<WorldClock>()).direct_factor * 0.5; // 复用权威天体采样，断言只隔离天气五成透光倍率。

        app.update(); // 同一帧合成天体曲线和环境修正。

        let light = app.world().get::<DirectionalLight>(light_entity).unwrap(); // 读取最终方向光结果。
        let ambient = app.world().get::<AmbientLight>(camera_entity).unwrap(); // 读取最终环境光结果。
        let skybox = app.world().get::<Skybox>(camera_entity).unwrap(); // 读取天空盒模式最终可见亮度。
        let environment_map = app
            .world()
            .get::<EnvironmentMapLight>(camera_entity)
            .unwrap(); // 读取最终环境反射结果。
        assert!((light.illuminance - expected_direct_illuminance).abs() < 0.1); // 五成透光必须精确乘在既有天体照度结果上。
        assert_color_close(light.color, light_tint); // 满混色强度必须精确采用天气光色。
        assert!((ambient.brightness - 20.0).abs() < f32::EPSILON); // 正午八十乘四分之一应为二十。
        assert_color_close(ambient.color, light_tint); // 环境光与方向光使用同一天气色调。
        assert!((skybox.brightness - 4_000.0).abs() < 0.1); // 天空盒作者亮度接受与 IBL 相同的四成阴雨倍率。
        assert!((environment_map.intensity - 4_000.0).abs() < 0.1); // 环境 IBL 镜面作者基准接受四成阴雨压暗倍率。
        let day_sky = Color::srgb(0.4, 0.7, 1.0); // 与玩法天体光标记中的作者日景基准保持一致。
        let expected_clear_color = apply_sky_modifiers(day_sky, sky_tint, 1.0, 0.4); // 正午无晨昏覆盖，可直接验证天气乘色和压暗。
        assert_color_close(app.world().resource::<ClearColor>().0, expected_clear_color); // 动态大气回退底色必须接受天气亮度倍率，并保留作者日景相对色彩。
    }

    #[test] // 恶劣天气不得用绝对冷灰目标色抬亮午夜天空，昼夜差异必须连续保留。
    fn weather_sky_modifiers_preserve_dark_night() {
        let day = Color::srgb(0.45, 0.70, 1.0); // 模拟场景作者日景天空。
        let night = NIGHT_CLEAR_COLOR; // 使用正式午夜深蓝黑基准。
        let rain_tint = Color::linear_rgb(0.48, 0.57, 0.70); // 大雨正式线性冷灰乘色。
        let day_weather = apply_sky_modifiers(day, rain_tint, 0.8, 0.25); // 大雨正午仍可辨识云层背景。
        let night_weather = apply_sky_modifiers(night, rain_tint, 0.8, 0.25); // 同一大雨倍率作用到午夜。
        let day_linear = day_weather.to_linear(); // 转到线性空间比较实际亮度通道。
        let night_linear = night_weather.to_linear(); // 午夜结果同样转为线性空间。
        let day_peak = day_linear.red.max(day_linear.green).max(day_linear.blue); // 取最亮通道作为保守亮度比较。
        let night_peak = night_linear
            .red
            .max(night_linear.green)
            .max(night_linear.blue); // 读取午夜最亮蓝通道。
        assert!(night_peak < day_peak * 0.01); // 午夜应至少比同天气日景暗两个数量级，不能形成亮灰天空。
        assert!(night_linear.blue > night_linear.red); // 午夜仍保留世界时间冷蓝轮廓，而非被天气洗成中性灰。
    } // 结束恶劣天气夜景亮度保护测试。

    #[test] // 六点朝霞和十八点夕阳必须让天空呈现明显暖色而非直接跳到日景或夜景。
    fn sunrise_and_sunset_apply_warm_clear_colors() {
        for (hour, expected_twilight_color) in [(6.0, DAWN_CLEAR_COLOR), (18.0, DUSK_CLEAR_COLOR)] {
            let mut app = lighting_app(hour, 1.0); // 分别构造晨昏峰值时刻的最小光照应用。
            let day_clear_color = Color::srgb(0.40, 0.66, 0.94); // 使用明显偏蓝的作者日景色验证暖色叠加。
            app.world_mut().spawn((
                DirectionalLight::default(), // 生成统一系统可以接管的玩法方向光。
                WorldCelestialLight::new(20_000.0, Color::WHITE, day_clear_color), // 提供照度、日光色和作者日景天空基准。
            ));
            app.update(); // 应用对应时刻的环境和天空颜色。
            let clear_color = app.world().resource::<ClearColor>().0.to_srgba(); // 转为 sRGB 便于按视觉通道检查暖色倾向。
            let target = expected_twilight_color.to_srgba(); // 读取该时段配置的朝霞或夕阳目标色。
            assert!(clear_color.red > clear_color.blue); // 晨昏峰值必须由暖红通道主导而不是保持日景蓝色。
            assert!((clear_color.red - target.red).abs() < 0.15); // 最大混合后应明显接近配置的晨昏目标色。
        }
    }

    #[test] // 没有玩法天体光标记时主菜单清屏色和普通方向光不得被接管。
    fn unmarked_lights_and_clear_color_are_unchanged() {
        let mut app = lighting_app(0.0, 9.0); // 使用能明显暴露误写的午夜和九倍倍率。
        let original_clear = Color::srgb(0.018, 0.016, 0.014); // 主菜单暗墨清屏色。
        app.insert_resource(ClearColor(original_clear)); // 模拟主菜单已经设置自己的清屏色。
        let light_entity = app
            .world_mut()
            .spawn(DirectionalLight {
                illuminance: 2_500.0, // 模拟主菜单独立方向光。
                ..default()           // 其他字段沿用默认值。
            })
            .id(); // 保存未标记光源实体。
        app.update(); // 统一系统运行但不应命中该光源。
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(light_entity)
                .unwrap()
                .illuminance,
            2_500.0
        ); // 主菜单光照保持原值。
        assert_eq!(app.world().resource::<ClearColor>().0, original_clear); // 无玩法标记时清屏色保持原值。
    }
}
