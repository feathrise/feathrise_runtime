//! 天气 GPU 降雨：复用单个 Hanabi 发射体，在玩法主相机前方生成局部雨幕。

use bevy::camera::visibility::RenderLayers; // 专属渲染层确保小场景和其他辅助相机不绘制雨丝。
use bevy::ecs::system::SystemParam; // 按天气输入与实体查询职责收拢降雨系统参数。
use bevy::prelude::*; // ECS查询、资源、Transform、Visibility和资产容器来自Bevy预导入。
use feathrise_scene_core::view::PrimarySceneView; // 玩法主相机是降雨跟随和场景存在性的权威标记。
use feathrise_scene_core::constants::render_layers::WEATHER_EFFECT_LAYER; // 雨丝只写入天气专属渲染层。
use feathrise_environment::rain_shelter::{RainShelterGpu, RainShelterState};
use feathrise_settings::{EffectsQuality, RenderSettings}; // 统一特效质量缩放天气 GPU 粒子数量。
use feathrise_scene_core::state::ScenePhase;
use bevy_hanabi::prelude::*; // Hanabi效果资产、表达式、粒子修饰器与发射器组件。 // 菜单和加载阶段必须立即销毁玩法降雨实体。 // 与地表和水花共用当前完整遮雨体快照。
mod occlusion;
mod shelter; // Hanabi逐雨丝渲染遮挡和固定属性声明独立维护。
mod water; // 透明水面不写主深度，雨丝用局部水面高度图独立裁切。 // 雨向深度图自动识别实际屋顶，逐片元截断穿顶雨丝。
pub(crate) use occlusion::sync_weather_rain_occlusion; // 后置消费生产端原子纹理身份。
pub(crate) use water::sync_weather_rain_water; // 与水面生产端按共享SystemSet协调发布。

use super::{is_gameplay_scene, supports_camera_local_weather}; // 环境、雾和降雨共用场景边界及F12正交测绘例外。
use crate::{
    WeatherAuthoringPreview, WeatherOutput, WeatherPreviewCamera,
    weather::constants::DEFAULT_WIND_DIRECTION,
}; // 连续天气输出控制降水和雨向，F12显式预览相机可临时成为唯一跟随锚点。

/// 雨发射圆盘半径。
const RAIN_RADIUS: f32 = 100.0; // 直径二百米，把开阔地中可观察到的局部雨幕边界进一步推远。
/// 雨发射圆盘沿玩法相机水平视线前移的距离。
const RAIN_FORWARD_OFFSET: f32 = 20.0; // 最大拉远二十米时仍让玩家前方保留约五十米雨区，并把镜头前向覆盖推到约七十米。
/// 相机水平视线可稳定归一化的最小长度平方。
const RAIN_HORIZONTAL_FORWARD_EPSILON_SQUARED: f32 = 1.0e-6; // 距垂直方向约零点零五七度以内不前移，避免浮点残差被放大成二十米抖动。
/// 发射圆盘位于相机上方的高度。
const RAIN_EMITTER_HEIGHT: f32 = 18.0; // 雨丝从视野上方生成并有足够距离穿过地面附近。
/// 雨丝初始位置在发射体下方覆盖的垂直范围。
const RAIN_VERTICAL_SPREAD: f32 = 30.0; // 从相机上方十八米铺到下方十二米，暂停单批也能直接充满视野。
/// 单个雨粒子的生存时间。
const RAIN_LIFETIME: f32 = 1.45; // 三十米每秒下落速度可覆盖约四十三米垂直距离。
/// Hanabi单次小批次发射间隔。
const RAIN_BURST_PERIOD: f32 = 0.05; // 每秒二十小批次兼顾连续观感与暂停时首帧静态雨丝。
/// 满强度大雨每小批次生成粒子数。
const RAIN_PARTICLES_PER_BURST: f32 = 2_080.0; // 半径翻倍后按四倍面积同步提高到每秒约四万一千六百条，保持原有单位面积密度。
/// F1暂停静态预览相对普通单批的密度倍率。
const PAUSED_PREVIEW_BATCH_MULTIPLIER: f32 = 4.0; // 满雨量一次生成八千三百二十条，保持扩大前的暂停画面单位面积密度。
/// Hanabi雨效果允许同时存活的最大粒子数。
const RAIN_EFFECT_CAPACITY: u32 = 131_072; // 满雨量约六万条稳态粒子保留超过两倍预算，避免卡帧集中提交时截断。
/// 雨丝生命周期起点长度。
const RAIN_STREAK_START_LENGTH: f32 = 0.65; // 从一点二米缩短到六十五厘米，减弱近景雨线过长的视觉问题。
/// 雨丝生命周期末端长度。
const RAIN_STREAK_END_LENGTH: f32 = 0.35; // 末端缩短到三十五厘米并继续保持自然淡出。
/// 允许创建降雨实体的最小降水量。
const RAIN_EMISSION_EPSILON: f32 = 0.01; // 极小过渡尾量不再维持GPU效果实体。
/// 动态雨速Hanabi属性名称。
const RAIN_VELOCITY_PROPERTY: &str = "weather_rain_velocity"; // 每帧通过EffectProperties连续更新风偏与下落速度。

/// 天气雨发射体标记与F1暂停预览状态，保证全局最多保留一个实例且不会零时间重复堆积。
#[derive(Component, Debug)] // 少量CPU状态只用于暂停预览单批次控制，不复制天气状态机。
pub(crate) struct WeatherRainEmitter {
    shelter_snapshot: RainShelterGpu, // 仅完整遮雨数据变化时写入97个Hanabi属性，暂停仍可更新。
    preview_precipitation: f32,       // 创建实体时的雨量；暂停拖动明显变化时重建以清掉旧静态粒子。
    effects_quality: EffectsQuality,  // 当前发射密度档；暂停切档时重建以替换静态预览。
    paused_preview_seeded: bool,      // 暂停期间只允许PostUpdate向GPU提交一次静态雨丝批次。
} // 结束天气雨发射体状态声明。

/// 系统本地缓存GPU效果资产句柄，天气开关时只重建实体而不重复创建资产。
#[derive(Default)] // 初次出现降雨时延迟构建资产，未安装Hanabi资产容器时保持空。
pub(crate) struct RainEffectCache {
    effect: Option<Handle<EffectAsset>>, // 强句柄保证多次晴雨切换期间效果资产持续存活。
} // 结束降雨资产缓存声明。

/// 玩法相机只读查询数据；静态引用用于定义可复用的Bevy查询形状。
type RainCameraQueryData = (
    &'static Transform,          // 发射圆盘读取目标相机姿态和水平视线。
    Option<&'static Camera>,     // 最小ECS测试允许缺少真实Camera组件。
    Has<WeatherPreviewCamera>,   // true表示F12显式天气预览视口。
    Option<&'static Projection>, // F12正交测绘视图不生成只覆盖相机附近两百米的局部雨盘。
);
/// 玩法相机与雨发射体显式互斥，允许同一系统安全写入发射体Transform。
type RainCameraQueryFilter = (
    Or<(With<PrimarySceneView>, With<WeatherPreviewCamera>)>, // 普通辅助相机仍不具备降雨资格。
    Without<WeatherRainEmitter>,                          // 相机和天气发射体保持查询互斥。
);
/// 已有雨发射体每帧同步所需的完整组件集合。
type RainEmitterQueryData = (
    Entity,
    &'static mut WeatherRainEmitter,
    &'static mut Transform,
    &'static mut Visibility,
    Option<&'static mut EffectSpawner>,
    Option<&'static mut EffectProperties>,
);
/// 雨发射体与玩法相机显式互斥，满足Transform并行借用约束。
type RainEmitterQueryFilter = (
    With<WeatherRainEmitter>,      // 只更新天气系统自有Hanabi发射体。
    Without<PrimarySceneView>,         // 与玩法相机查询互斥。
    Without<WeatherPreviewCamera>, // 与F12作者相机查询同样互斥。
);

/// 降雨同步系统共同消费的天气、场景、暂停状态与可选GPU资产。
#[derive(SystemParam)]
pub(crate) struct RainSyncInputs<'w> {
    roof_mask: Option<Res<'w, super::rain_occlusion::RainOcclusionMap>>, // 自动屋顶纹理与水面保持独立槽。
    water_mask: Option<Res<'w, feathrise_environment::rain_water::RainWaterMask>>, // 场景生产的局部真实水面范围与固定图像身份。
    shelter: Option<Res<'w, RainShelterState>>, // 独立天气测试没有遮雨插件时安全使用空快照。
    output: Res<'w, WeatherOutput>,             // 连续雨量、风力与风向来自天气核心唯一输出。
    scene: Option<Res<'w, State<ScenePhase>>>,   // 正式游戏隔离菜单，最小ECS测试可缺失状态机。
    virtual_time: Option<Res<'w, Time<Virtual>>>, // F1暂停时关闭循环burst并保留静态预览。
    settings: Option<Res<'w, RenderSettings>>,    // 独立天气测试可缺失设置并安全使用高档粒子基线。
    effects: Option<ResMut<'w, Assets<EffectAsset>>>, // 未安装Hanabi资产容器时安全降级。
    preview: Option<Res<'w, WeatherAuthoringPreview>>, // 独立测试允许缺失，F12打开时雨区围绕作者相机生成。
}

/// 降雨系统负责读取玩法相机并独占更新雨发射体。
#[derive(SystemParam)]
pub(crate) struct RainSyncQueries<'w, 's> {
    cameras: Query<'w, 's, RainCameraQueryData, RainCameraQueryFilter>,
    emitters: Query<'w, 's, RainEmitterQueryData, RainEmitterQueryFilter>,
}

/// 同步玩法相机附近唯一的GPU雨发射体。
pub(crate) fn sync_weather_rain(
    mut commands: Commands,            // 创建、更新并在场景切换中销毁降雨实体。
    mut inputs: RainSyncInputs,        // 读取天气、场景和暂停状态，并按需登记GPU效果资产。
    mut queries: RainSyncQueries,      // 查询玩法主相机并独占更新已有雨发射体。
    mut cache: Local<RainEffectCache>, // 单系统本地强句柄复用同一GPU效果资产。
) {
    let shelter_gpu = inputs
        .shelter
        .as_ref()
        .map_or_else(RainShelterGpu::default, |state| state.gpu); // 复制固定大小快照，不扫描场景几何。
    let precipitation = finite_unit(inputs.output.precipitation, 0.0); // 损坏雨量安全回退为停雨。
    let wind = finite_unit(inputs.output.wind, 0.0); // 损坏风力安全回退为垂直落雨。
    let wind_direction = inputs.output.wind_direction; // 复制同帧风向，后续可独立借用可变GPU资产容器。
    let effects_quality = inputs
        .settings
        .as_deref()
        .map_or(EffectsQuality::High, |settings| {
            settings.graphics.effects_quality
        }); // 旧组合根或最小测试缺失资源时严格保持现有雨量基线。
    let virtual_time_paused = inputs
        .virtual_time
        .as_deref()
        .is_some_and(Time::<Virtual>::is_paused); // 无虚拟时钟的最小测试按正常运行处理，正式F1暂停明确关闭循环发射。
    let gameplay_scene = is_gameplay_scene(inputs.scene.as_deref()); // 菜单和加载阶段无条件停发。
    let preview_active = inputs
        .preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 缺失作者资源时严格保持玩法主相机基线。
    let emitter_position = queries
        .cameras
        .iter()
        .find(|(_, camera, is_preview, projection)| {
            *is_preview == preview_active // 作者会话与玩法会话始终只选择一个相机锚点。
                && camera.is_none_or(|camera| camera.is_active) // 场景切换中停用相机不继续承载降雨。
                && supports_camera_local_weather(*is_preview, *projection) // 正交视图不得显示局部雨盘截断边缘。
        })
        .map(|(transform, _, _, _)| rain_emitter_position(transform)); // 发射圆盘按所选相机水平视线前移后定位。
    let active_emitter_position = if gameplay_scene && precipitation > RAIN_EMISSION_EPSILON {
        emitter_position // 玩法场景、有效雨量和主相机共同决定本帧唯一发射位置。
    } else {
        None // 非玩法或极小过渡尾量统一走停雨清理。
    };
    let mut kept_emitter = false; // 遍历时只允许第一个有效发射体保留。
    for (entity, mut emitter, mut transform, mut visibility, spawner, properties) in
        &mut queries.emitters
    {
        let Some(emitter_position) = active_emitter_position else {
            commands.entity(entity).despawn(); // 无有效发射位置时清掉全部旧GPU粒子。
            continue;
        };
        if kept_emitter {
            commands.entity(entity).despawn(); // 停雨、离开玩法或意外重复时立即清掉旧GPU粒子。
            continue; // 不再更新即将销毁的发射体。
        } // 结束失效与重复发射体处理。
        if virtual_time_paused
            && ((emitter.preview_precipitation - precipitation).abs() > 0.02
                || emitter.effects_quality != effects_quality)
        {
            commands.entity(entity).despawn(); // F1明显改变雨量时重建实例，避免不同密度静态批次在GPU中累计。
            continue; // 本帧创建路径会按新雨量生成干净发射体。
        } // 结束暂停预览雨量变化重建。
        kept_emitter = true; // 当前实体成为本帧唯一保留的雨发射体。
        transform.translation = emitter_position; // 发射圆盘每帧跟随相机位置与水平视线前移目标。
        transform.rotation = Quat::IDENTITY; // 固定世界朝向，镜头旋转不会改变雨向。
        transform.scale = Vec3::ONE; // 固定粒子定义尺度，避免相机层级缩放污染雨丝。
        *visibility = Visibility::Visible; // 有效发射体保持可见，停雨路径直接销毁实体。
        if let Some(mut spawner) = spawner {
            let desired_count: CpuValue<f32> =
                rain_burst_particle_count(precipitation, effects_quality).into(); // 同一纯函数驱动新旧发射体的目标批次数量。
            if spawner.settings.count() != desired_count {
                spawner.settings.set_count(desired_count); // 只在雨量或特效档变化时更新Hanabi发射参数。
            }
            let should_be_active = !virtual_time_paused; // 正常游戏持续发射，暂停交给单批预热系统。
            if spawner.active != should_be_active {
                spawner.active = should_be_active; // 稳定帧不重复修改发射器活动状态。
            }
            if virtual_time_paused {
                if spawner.spawn_count != 0 {
                    spawner.spawn_count = 0; // 只清一次上一帧计数，防止零delta重复提取同一批粒子。
                }
            } else {
                if emitter.preview_precipitation != precipitation {
                    emitter.preview_precipitation = precipitation; // 自然雨量真实变化时刷新暂停比较基准。
                }
                if emitter.effects_quality != effects_quality {
                    emitter.effects_quality = effects_quality; // 运行期切档只调整后续发射量，不重建固定容量资产。
                }
                if !emitter.paused_preview_seeded {
                    emitter.paused_preview_seeded = true; // 活动雨丝已存在时，随后暂停无需额外预热。
                }
            } // 结束正常运行与暂停发射分支。
        } // Hanabi首帧尚未附加EffectSpawner时由插件在PostUpdate自动创建。
        if let Some(mut properties) = properties {
            let velocity: Value = rain_velocity(wind, wind_direction).into(); // 固定风速不反复触发包含193项遮雨数据的组件提取。
            if properties.get_stored(RAIN_VELOCITY_PROPERTY) != Some(velocity) {
                // 真实风速变化才借用属性可变引用。
                properties.set(RAIN_VELOCITY_PROPERTY, velocity); // 新生成雨丝持续读取当前同源风力与方向。
            } // 完成风速变化检测。
        } // Hanabi属性组件存在时完成动态风向同步。
    } // 结束现有雨发射体遍历。
    if kept_emitter {
        return; // 已复用唯一实例时无需创建新发射体。
    }
    let Some(emitter_position) = active_emitter_position else {
        return; // 停雨、离开玩法或无主相机时不创建发射体。
    };
    let Some(effects) = inputs.effects.as_deref_mut() else {
        return; // 未安装HanabiPlugin时不创建不可渲染实体，核心天气插件仍可独立运行测试。
    }; // 结束Hanabi资产容器存在性检查。
    let effect = cache
        .effect
        .get_or_insert_with(|| effects.add(build_rain_effect()))
        .clone(); // 首次降雨构建一次资产，后续晴雨循环复用同一强句柄。
    let velocity = rain_velocity(wind, wind_direction); // 生成实体前按天气核心同源风场计算本帧默认雨速。
    let spawner_settings = rain_spawner_settings(precipitation, effects_quality); // 组件首帧直接使用当前雨量与特效档，避免等待Hanabi覆盖。
    let mut rain_properties = EffectProperties::default()
        .with_properties([(RAIN_VELOCITY_PROPERTY.to_owned(), velocity.into())]); // 新实体先安装原有动态风速。
    shelter::set_properties(&mut rain_properties, &shelter_gpu); // 首帧就携带完整遮雨快照，避免屋内闪一帧雨。
    let water_mask = inputs.water_mask.as_deref().cloned().unwrap_or_default(); // 独立天气宿主缺水面生产者时禁用裁切。
    water::set_properties(&mut rain_properties, &water_mask); // 首帧范围与材质纹理来自同一完成状态。
    let roof_mask = inputs.roof_mask.as_deref().cloned().unwrap_or_default(); // 缺失生产者使用无覆盖fallback。
    commands.spawn((
        Name::new("WeatherRainEmitter"), // 调试层级中使用稳定英文ASCII名称。
        WeatherRainEmitter {
            shelter_snapshot: shelter_gpu, // 与首帧EffectProperties同批发布。
            preview_precipitation: precipitation, // 记录当前雨量供F1拖动时判断是否需要清空重建。
            effects_quality,               // 记录当前粒子档供运行期更新与暂停切档重建。
            paused_preview_seeded: !virtual_time_paused, // 正常游戏已有连续发射，暂停新建实例等待单批静态预热。
        }, // 标记实体供后续帧跟随、去重、暂停预览和清理。
        ParticleEffect::new(effect),     // 实例化缓存的全局空间GPU雨效果。
        EffectMaterial {
            images: vec![water_mask.image, roof_mask.image], // 槽0保持水面，槽1读取自动屋顶完整深度与矩阵。
        }, // 两种遮挡都仅在正式片元材质可见阶段采样。
        EffectSpawner::new(&spawner_settings).with_active(!virtual_time_paused), // 暂停首帧即关闭循环burst，正常游戏立即持续发射。
        rain_properties, // 风速和遮雨属性在同一组件中原子交给Hanabi。
        Transform::from_translation(emitter_position), // 初始发射圆盘直接使用相机前方二十米与上方十八米的完整目标位置。
        Visibility::Visible, // 新实体需在暂停预览的首个可编译帧绘制静态雨丝。
        RenderLayers::layer(WEATHER_EFFECT_LAYER), // 小场景和普通辅助相机不会包含天气专属层。
    )); // 生成全局唯一玩法降雨发射体。
} // 结束GPU降雨同步系统。

/// 变换传播与完整遮雨发布后再同步粒子属性，确保移动屋檐和暂停预览同帧生效。
pub(crate) fn sync_weather_rain_shelter(
    state: Option<Res<RainShelterState>>, // 缺失资源时清空旧遮雨信息。
    mut emitters: Query<(&mut WeatherRainEmitter, &mut EffectProperties)>, // 只遍历唯一雨发射体，不遍历GPU粒子。
) {
    // 注册在PostUpdate RainShelterUpload后、Hanabi属性整理之前。
    let gpu = state
        .as_ref()
        .map_or_else(RainShelterGpu::default, |state| state.gpu); // 固定大小完成快照。
    for (mut emitter, mut properties) in &mut emitters {
        // 最多一个正常雨发射体。
        if emitter.shelter_snapshot != gpu {
            // 稳定帧不创建属性名，也不标记EffectProperties变化。
            shelter::set_properties(&mut properties, &gpu); // 在提取前原子更新193项值。
            emitter.shelter_snapshot = gpu; // 同批记录已发布快照。
        } // 完成真实变化同步。
    } // 完成单实例更新。
} // 结束同帧渲染遮雨发布。

/// Hanabi完成本帧常规发射器处理后，为F1暂停中新建的雨效只提交一批静态雨丝。
pub(crate) fn seed_paused_weather_rain_preview(
    virtual_time: Option<Res<Time<Virtual>>>, // 只有F1等暂停虚拟时间时才需要静态预热。
    mut emitters: Query<(
        &mut WeatherRainEmitter, // 单实例保存是否已经成功提交过暂停预览批次。
        &CompiledParticleEffect, // 等待Hanabi和渲染世界确认效果已可接收GPU初始化命令。
        &mut EffectSpawner,      // 在自动tick之后手动写一次spawn_count，下一帧由暂停路径归零。
    )>,
) {
    if !virtual_time
        .as_deref()
        .is_some_and(Time::<Virtual>::is_paused)
    {
        return; // 正常游戏完全沿用Hanabi时钟和循环发射器，不手工覆盖spawn_count。
    } // 结束暂停状态守卫。
    for (mut emitter, compiled, mut spawner) in &mut emitters {
        spawner.active = false; // 防御Update之后的其他写入，确保零delta不会进入循环burst逻辑。
        if emitter.paused_preview_seeded || !compiled.is_ready() {
            continue; // 已预热实例不重复提交；尚未编译完成则留到后续暂停帧再试。
        } // 结束静态预热条件检查。
        spawner.spawn_count = paused_preview_spawn_count(
            emitter.preview_precipitation, // 使用发射体创建时保存的连续雨量。
            emitter.effects_quality,       // 静态预览与正常雨效使用同一特效质量倍率。
        ); // GPU只初始化当前档位的一批静态雨丝。
        emitter.paused_preview_seeded = true; // 即使当前雨量极低也只尝试一次，拖动变化会重建实体。
    } // 结束唯一雨发射体暂停预热。
} // 结束F1暂停静态雨丝预热系统。

/// 构建相机局部覆盖、粒子全局下落的Hanabi雨效果资产。
fn build_rain_effect() -> EffectAsset {
    let writer = ExprWriter::new(); // 所有初始化表达式写入同一Hanabi模块。
    shelter::register_properties(&writer); // 注册固定193项属性，运行期仅修改值而不重建效果资产。
    water::register_properties(&writer); // 两个额外Vec4安全传递高度图范围与启用状态。
    let velocity_property = writer.add_property(
        RAIN_VELOCITY_PROPERTY, // 注册运行期连续更新的雨速属性。
        rain_velocity(0.0, DEFAULT_WIND_DIRECTION).into(), // 默认无风时垂直快速下落并保留合法方向输入。
    ); // 完成动态雨速属性注册。
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.0).expr()); // 新雨丝年龄从零开始。
    let init_lifetime = SetAttributeModifier::new(
        Attribute::LIFETIME,              // 写入Hanabi标准粒子生存时间属性。
        writer.lit(RAIN_LIFETIME).expr(), // 固定寿命保证雨丝越过相机附近地面后及时回收。
    ); // 完成雨丝寿命初始化。
    let init_position = SetPositionCircleModifier {
        center: writer.lit(Vec3::ZERO).expr(), // 圆盘中心使用发射体世界位置。
        axis: writer.lit(Vec3::Y).expr(),      // 水平圆盘法线朝上。
        radius: writer.lit(RAIN_RADIUS).expr(), // 半径一百米的圆盘覆盖镜头前方约二百米直径区域。
        dimension: ShapeDimension::Volume,     // 在整个圆盘内部均匀生成而非只沿圆周。
    }; // 完成雨丝初始位置修饰器。
    let init_vertical_position = SetAttributeModifier::new(
        Attribute::POSITION, // 在水平圆盘随机位置基础上继续写入垂直分布。
        writer
            .attr(Attribute::POSITION) // 读取前一初始化修饰器已经生成的圆盘位置。
            .add(
                writer.rand(VectorType::VEC3F)
                    * writer.lit(Vec3::new(0.0, -RAIN_VERTICAL_SPREAD, 0.0)),
            ) // 只随机向下偏移Y，形成发射圆盘中心上下的完整柱状雨幕。
            .expr(),
    ); // 完成雨丝垂直位置初始化。
    let init_velocity = SetAttributeModifier::new(
        Attribute::VELOCITY,                   // 直接写入世界空间粒子速度。
        writer.prop(velocity_property).expr(), // 每批新粒子读取最新运行期风偏属性。
    ); // 完成动态雨速初始化修饰器。
    let mut color_gradient = bevy_hanabi::Gradient::new(); // 雨丝颜色随寿命略微淡出。
    color_gradient.add_key(0.0, Vec4::new(0.66, 0.76, 0.90, 0.52)); // 新雨丝使用半透明冷蓝灰色。
    color_gradient.add_key(1.0, Vec4::new(0.52, 0.64, 0.82, 0.08)); // 接近地面时淡出避免突然消失。
    let mut size_gradient = bevy_hanabi::Gradient::new(); // 细长四边形沿速度方向表现雨丝。
    size_gradient.add_key(0.0, Vec3::new(RAIN_STREAK_START_LENGTH, 0.025, 1.0)); // 初始雨丝缩短到六十五厘米，宽度仍保持二点五厘米。
    size_gradient.add_key(1.0, Vec3::new(RAIN_STREAK_END_LENGTH, 0.018, 1.0)); // 生命周期末端缩短到三十五厘米，宽度仍按原曲线变细。
    let spawner = rain_spawner_settings(0.40, EffectsQuality::High); // 资产默认保持高档小雨基线，实体首帧按当前设置覆盖。
    let mut module = writer.finish(); // 完成已有粒子表达式构建。
    module.add_texture_slot("rain_water_height", bevy_hanabi::SlotDimension::D2); // Hanabi按登记顺序生成material_texture_0片元绑定。
    module.add_texture_slot("rain_roof_depth", bevy_hanabi::SlotDimension::D2); // material_texture_1独立保存屋顶深度及原子投影元数据。
    EffectAsset::new(RAIN_EFFECT_CAPACITY, spawner, module) // 扩大后的满雨量稳态粒子保留超过两倍容量预算。
        .with_name("weather_rain") // GPU调试和资产检查使用稳定ASCII名称。
        .with_simulation_space(SimulationSpace::Global) // 已生成雨丝不随移动中的相机发射体平移。
        .with_alpha_mode(bevy_hanabi::AlphaMode::Blend) // 半透明雨丝使用正常混合而非发光叠加。
        .init(init_age) // 注册年龄初始化。
        .init(init_lifetime) // 注册寿命初始化。
        .init(init_position) // 注册水平圆盘随机位置初始化。
        .init(init_vertical_position) // 再把雨丝铺到相机上下可见的完整垂直范围。
        .init(init_velocity) // 注册动态风偏速度初始化。
        .init(shelter::RainPropertyIndexModifier) // 首次生成就保存真实属性行号，暂停预览无需等待Update。
        .update(shelter::RainPropertyIndexModifier) // 每次模拟刷新身份，渲染只从粒子读取行号而不依赖缺失全局变量。
        .render(ColorOverLifetimeModifier {
            gradient: color_gradient, // 应用冷蓝灰淡出颜色曲线。
            ..default()               // 其他颜色输出选项沿用Hanabi稳定默认值。
        }) // 完成颜色渲染修饰器。
        .render(SizeOverLifetimeModifier {
            gradient: size_gradient,  // 应用细长雨丝尺寸曲线。
            screen_space_size: false, // 使用世界米制尺寸，远近关系保持自然。
        }) // 完成尺寸渲染修饰器。
        .render(OrientModifier::new(OrientMode::AlongVelocity)) // 雨丝长轴始终跟随下落与风偏方向。
        .render(shelter::RainShelterModifier) // 最后逐雨丝清透明度与尺寸，暂停及屋外背景雨保持正确。
        .render(water::RainWaterModifier) // 在片元阶段沿真实湖面截断雨丝下半段。
        .render(occlusion::RainOcclusionModifier) // 实际屋顶继续裁切剩余片元，天井开口保留降雨。
} // 结束Hanabi雨效果资产构造。

/// 构造指定连续雨量的无限小批次发射配置。
fn rain_spawner_settings(
    precipitation: f32,      // 天气核心输出的连续归一化雨量。
    quality: EffectsQuality, // 画质页选择的 GPU 粒子数量档。
) -> SpawnerSettings {
    SpawnerSettings::burst(
        rain_burst_particle_count(precipitation, quality).into(), // 雨量与特效档共同控制每批粒子数。
        RAIN_BURST_PERIOD.into(), // 正常游戏每秒二十批形成连续雨幕，暂停时组件会关闭自动tick。
    ) // 返回可同时写入EffectAsset默认值和实体覆盖组件的相同配置。
} // 结束雨发射配置构造。

/// 返回F1暂停预热所需的单批静态雨丝数量。
fn paused_preview_spawn_count(precipitation: f32, quality: EffectsQuality) -> u32 {
    (rain_burst_particle_count(precipitation, quality) * PAUSED_PREVIEW_BATCH_MULTIPLIER)
        .round()
        .clamp(
            0.0,
            RAIN_PARTICLES_PER_BURST
                * EffectsQuality::Ultra.particle_multiplier()
                * PAUSED_PREVIEW_BATCH_MULTIPLIER,
        ) as u32 // 单次最多一万零四百条，暂停任意帧数都不会继续增长。
} // 结束暂停预览单批粒子数计算。

/// 返回指定连续雨量与特效档每小批应发射的粒子数。
fn rain_burst_particle_count(precipitation: f32, quality: EffectsQuality) -> f32 {
    RAIN_PARTICLES_PER_BURST * finite_unit(precipitation, 0.0) * quality.particle_multiplier() // 高档单位倍率严格保持现有雨幕密度，其他档只缩放透明粒子数量。
}

/// 返回相对玩法相机的雨丝初始垂直覆盖范围。
#[cfg(test)] // 纯测试辅助函数验证正式常量确实让暂停雨幕跨过相机高度。
fn rain_spawn_height_bounds() -> (f32, f32) {
    (
        RAIN_EMITTER_HEIGHT - RAIN_VERTICAL_SPREAD, // 最低点位于相机下方十二米。
        RAIN_EMITTER_HEIGHT,                        // 最高点位于相机上方十八米。
    ) // 返回静态预览和正常降雨共用的垂直区间。
} // 结束雨幕垂直覆盖范围读取。

/// 返回玩法相机前方与上方的雨发射圆盘世界位置。
pub(super) fn rain_emitter_position(camera_transform: &Transform) -> Vec3 {
    // 雨向捕获复用真实雨盘锚点，避免两套偏移。
    let camera_forward_direction = camera_transform.forward(); // Bevy玩法相机的局部负Z轴以Dir3形式指向当前观察方向。
    let camera_forward = camera_forward_direction.as_vec3(); // 转成普通Vec3以便清除垂直分量并计算水平前移。
    let horizontal_forward = Vec3::new(camera_forward.x, 0.0, camera_forward.z); // 忽略俯仰分量，避免仰视或俯视改变雨幕高度。
    let horizontal_length_squared = horizontal_forward.length_squared(); // 读取水平投影长度以判断是否能够安全归一化。
    let has_stable_horizontal_forward = horizontal_forward.is_finite()
        && horizontal_length_squared > RAIN_HORIZONTAL_FORWARD_EPSILON_SQUARED; // 只有有限且明显偏离垂直方向时才允许固定距离前移。
    let forward_offset = if has_stable_horizontal_forward {
        // 根据水平视线有效性选择正式前移或安全零偏移。
        horizontal_forward / horizontal_length_squared.sqrt() * RAIN_FORWARD_OFFSET // 合法水平视线按固定二十米生成前移偏移。
    } else {
        Vec3::ZERO // 接近垂直或损坏朝向时不前移，避免NaN传播到GPU发射体位置。
    }; // 完成相机水平前移安全计算。
    camera_transform.translation + forward_offset + Vec3::Y * RAIN_EMITTER_HEIGHT // 在水平前移之后统一抬高十八米生成圆盘。
} // 结束雨发射圆盘世界位置计算。

/// 将归一化风力转换为雨丝世界速度。
pub(super) fn rain_velocity(wind: f32, direction: Vec2) -> Vec3 {
    // 雨向捕获与粒子使用相同风向和速度。
    let wind = finite_unit(wind, 0.0); // 非有限或越界风力先规范化。
    let direction = safe_wind_direction(direction); // 风向异常时统一回退默认+X风向并归一化。
    Vec3::new(
        direction.x * wind * 7.0, // 满风横向速度固定七米每秒，与默认风向和场景位置无关。
        -30.0,                    // 所有雨量保持快速自然下落，密度而非速度表达雨势。
        direction.y * wind * 7.0, // 世界Z分量直接使用输入风向，避免雨、云和植被各自偏转。
    ) // 返回世界空间雨丝初始速度。
} // 结束雨丝速度计算。

/// 返回合法归一化水平风向，损坏输出回退默认+X风向。
fn safe_wind_direction(direction: Vec2) -> Vec2 {
    if direction.is_finite() && direction.length_squared() > f32::EPSILON {
        if (direction.length_squared() - 1.0).abs() < 0.000_001 {
            direction // 已归一化天气输出直接复用，保持雨、云和植被位值一致。
        } else {
            direction.normalize() // 非单位合法向量只表达方向，不重复放大归一化风力。
        }
    } else {
        DEFAULT_WIND_DIRECTION // 非有限与零向量安全回退，避免GPU速度属性出现NaN。
    } // 返回雨丝安全风向。
} // 结束雨丝风向规范化。

/// 返回有限零至一值，否则使用安全回退。
fn finite_unit(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) // 雨量与风力严格限制在核心输出定义域。
    } else {
        fallback // 非有限输出按无雨或无风安全处理。
    } // 结束归一化参数分支。
} // 结束有限零至一辅助函数。

#[cfg(test)]
mod tests;
