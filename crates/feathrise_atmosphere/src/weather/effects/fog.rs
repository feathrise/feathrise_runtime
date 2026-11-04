// 天气距离雾：在世界时间完成当帧天空颜色后，仅同步玩法主相机的连续雾效。

use bevy::prelude::*; // DistanceFog、FogFalloff、ClearColor与ECS查询来自Bevy预导入。
use feathrise_scene_core::view::PrimarySceneView; // 只给玩法主相机添加雾，小地图与菜单相机不匹配。
use feathrise_scene_core::state::ScenePhase; // 菜单和加载状态用于关闭可能残留的玩法相机雾。

use super::{is_gameplay_scene, supports_camera_local_weather}; // 与云雨体积共享玩法场景和F12正交测绘边界。
use crate::{WeatherAuthoringPreview, WeatherOutput, WeatherPreviewCamera}; // 能见度和色调决定雾，F12可显式选择作者相机。

/// 晴天完整能见度时的最远雾终点。
const CLEAR_FOG_END: f32 = 1_600.0; // 远于主要玩法视距且透明度为零，不改变晴天画面。
/// 最恶劣天气仍保留的雾终点基础距离。
const MIN_FOG_END: f32 = 100.0; // 薄雾和大雨保留近距离战斗辨识空间。
/// 最强天气雾透明度上限。
const MAX_FOG_ALPHA: f32 = 0.90; // 不使用完全不透明雾，避免远景形成生硬纯色墙。

/// 与Bevy组件解耦的连续雾参数，便于纯逻辑测试距离和颜色映射。
#[derive(Clone, Copy, Debug)] // 每帧按值生成一次并复用于所有玩法主相机。
pub(super) struct WeatherFogProfile {
    pub(super) color: Color, // 与最终ClearColor和天气色调混合后的雾色，供距离雾和体积雾共用。
    directional_light_color: Color, // 太阳方向只保留轻微散射高光。
    start: f32,              // 从该相机距离开始逐步积累雾。
    end: f32,                // 到该相机距离达到配置透明度上限。
    pub(super) severity: f32, // 零至一恶劣程度供体积雾连续推导密度。
} // 结束连续雾参数声明。

/// 将连续天气能见度应用到全部玩法主相机。
pub(crate) fn apply_weather_fog(
    mut commands: Commands,     // 新生成玩法相机缺少DistanceFog时在帧末补齐组件。
    output: Res<WeatherOutput>, // 读取同帧完成F1预览合成的连续天气输出。
    preview: Option<Res<WeatherAuthoringPreview>>, // 独立测试允许缺失，正式F12打开时只把连续雾写到作者相机。
    scene: Option<Res<State<ScenePhase>>>,          // 菜单、加载与独立测试环境使用同一安全入口。
    clear_color: Option<Res<ClearColor>>, // 无渲染插件的最小ECS测试允许缺少全局清屏色资源。
    mut cameras: Query<
        (
            Entity,                    // 新目标相机缺少雾时通过命令补齐。
            Has<PrimarySceneView>,         // 正式玩法主相机资格。
            Has<WeatherPreviewCamera>, // F12显式天气作者视口资格。
            Option<&Projection>,       // F12正交测绘视图不应用以相机欧氏距离计算的雾。
            Option<&mut DistanceFog>,  // 已接管相机原地更新，不逐帧重建组件。
        ),
        Or<(With<PrimarySceneView>, With<WeatherPreviewCamera>)>, // 菜单、小地图和普通辅助相机保持隔离。
    >,
) {
    let gameplay_scene = is_gameplay_scene(scene.as_deref()); // 先确定残留相机是否仍允许显示天气。
    let clear_color = clear_color.map_or(Color::BLACK, |clear_color| clear_color.0); // 无清屏色时使用不污染色温的黑色兜底。
    let profile = weather_fog_profile(&output, clear_color, gameplay_scene); // 每帧只计算一次连续雾参数。
    for (entity, has_gameplay_marker, has_preview_marker, projection, fog) in &mut cameras {
        let preview_active = preview
            .as_deref()
            .is_some_and(WeatherAuthoringPreview::active); // 缺失资源保持正式玩法相机基线。
        let selected = (if preview_active {
            has_preview_marker // 作者会话只更新F12离屏视口。
        } else {
            has_gameplay_marker // 普通运行只更新玩法主相机。
        }) && supports_camera_local_weather(has_preview_marker, projection); // 正交作者视图继续保留天空和光照，但必须清理距离雾。
        if !selected {
            if has_preview_marker && fog.is_some() {
                commands.entity(entity).remove::<DistanceFog>(); // 关闭F12后清理作者相机临时雾，不留下辅助视图污染。
            }
            continue; // 非当前天气目标相机保留作者或正式玩法现状。
        }
        if let Some(mut fog) = fog {
            write_fog_component(&mut fog, profile); // 已接管相机原地更新，天气过渡不会重建组件。
        } else {
            commands.entity(entity).insert(distance_fog(profile)); // 新玩法相机同帧末获得当前天气雾。
        } // 结束已有与新增距离雾分支。
    } // 结束玩法主相机遍历。
} // 结束天气距离雾系统。

/// 根据最终天空色和天气输出计算连续雾参数。
pub(super) fn weather_fog_profile(
    output: &WeatherOutput, // 连续能见度与线性天气色调。
    clear_color: Color,     // 世界时间已经写入昼夜、晨昏与天气后的最终天空色。
    gameplay_scene: bool,   // 非玩法场景将雾透明度归零以隔离菜单。
) -> WeatherFogProfile {
    let visibility = if gameplay_scene {
        finite_unit(output.visibility, 1.0) // 玩法场景使用天气状态机提供的归一化能见度。
    } else {
        1.0 // 菜单与加载状态强制使用完整能见度，关闭残留相机雾。
    }; // 完成有效能见度选择。
    let severity = (1.0 - visibility).clamp(0.0, 1.0); // 恶劣程度与能见度反向连续对应。
    let end = MIN_FOG_END + (CLEAR_FOG_END - MIN_FOG_END) * visibility.powi(3); // 三次曲线让大雨与薄雾明显压缩远景且晴天快速退远。
    let start = end * (0.08 + 0.48 * visibility); // 低能见度更早起雾，高能见度只在远景轻微积累。
    let sky = clear_color.to_linear(); // 最终天空已包含昼夜、晨昏和天气压暗，是雾亮度的唯一基准。
    let tint_strength = 0.25 + severity * 0.55; // 恶劣天气逐步增强冷灰乘色，但不写入绝对亮色。
    let tinted_red =
        sky.red * (1.0 + (finite_channel(output.tint.red, 1.0) - 1.0) * tint_strength).max(0.0); // 红通道只相对调整最终天空，午夜不会被固定灰色抬亮。
    let tinted_green =
        sky.green * (1.0 + (finite_channel(output.tint.green, 1.0) - 1.0) * tint_strength).max(0.0); // 绿通道沿用相同昼夜相对亮度口径。
    let tinted_blue =
        sky.blue * (1.0 + (finite_channel(output.tint.blue, 1.0) - 1.0) * tint_strength).max(0.0); // 蓝通道保留夜景和阴雨的冷色倾向。
    let luminance = tinted_red * 0.2126 + tinted_green * 0.7152 + tinted_blue * 0.0722; // 在线性空间计算亮度供去饱和使用。
    let desaturation = severity * 0.55; // 雾越浓越接近同亮度灰色，但不改变昼夜能量级别。
    let red = tinted_red + (luminance - tinted_red) * desaturation; // 红通道向同亮度灰色连续靠拢。
    let green = tinted_green + (luminance - tinted_green) * desaturation; // 绿通道使用同一去饱和比例。
    let blue = tinted_blue + (luminance - tinted_blue) * desaturation; // 蓝通道仍保留足够冷色辨识度。
    let alpha = (severity / 0.65).clamp(0.0, 1.0) * MAX_FOG_ALPHA; // 薄雾目标能见度达到首版雾透明度上限。
    let color = Color::linear_rgba(red, green, blue, alpha); // 主雾色保留昼夜天空亮度与天气去饱和结果。
    let directional_light_color = Color::linear_rgba(
        red,             // 太阳散射高光与主雾使用相同红通道。
        green,           // 太阳散射高光与主雾使用相同绿通道。
        blue,            // 太阳散射高光与主雾使用相同蓝通道。
        severity * 0.08, // 只保留轻微方向高光，厚云不应出现强烈晴天光柱。
    ); // 完成方向光雾散射颜色。
    WeatherFogProfile {
        color,                   // 保存包含天气透明度的主雾色。
        directional_light_color, // 保存轻微太阳方向散射色。
        start,                   // 保存连续雾起点距离。
        end,                     // 保存连续雾终点距离。
        severity,                // 保存与能见度同源的恶劣程度供体积雾使用。
    } // 返回本帧完整雾参数。
} // 结束天气雾参数计算。

/// 从纯逻辑参数构造Bevy距离雾组件。
fn distance_fog(profile: WeatherFogProfile) -> DistanceFog {
    DistanceFog {
        color: profile.color, // 雾颜色已经包含昼夜天空、天气色调与透明度。
        directional_light_color: profile.directional_light_color, // 保留轻微日月方向散射。
        directional_light_exponent: 16.0, // 较高指数把方向高光限制在天体附近。
        falloff: FogFalloff::Linear {
            start: profile.start, // 连续天气决定开始积雾距离。
            end: profile.end,     // 连续天气决定达到最大雾量距离。
        }, // 线性雾便于按地图米制尺度直接控制可玩视距。
    } // 返回可直接插入玩法相机的距离雾组件。
} // 结束距离雾组件构造。

/// 原地覆盖已有距离雾的全部天气控制字段。
fn write_fog_component(fog: &mut DistanceFog, profile: WeatherFogProfile) {
    fog.color = profile.color; // 每帧同步天空与天气共同决定的雾色。
    fog.directional_light_color = profile.directional_light_color; // 每帧同步轻微天体散射色。
    fog.directional_light_exponent = 16.0; // 固定聚光指数避免过渡时出现额外参数跳变。
    fog.falloff = FogFalloff::Linear {
        start: profile.start, // 天气转差时雾起点连续向相机靠近。
        end: profile.end,     // 天气转差时雾终点连续向相机靠近。
    }; // 完成已有雾组件的线性衰减覆盖。
} // 结束已有距离雾写入。

/// 返回有限零至一值，否则使用安全回退。
fn finite_unit(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) // 能见度严格限制在天气输出定义域。
    } else {
        fallback // 损坏输出按晴天完整能见度处理，避免全屏雾墙。
    } // 结束能见度规范化分支。
} // 结束有限零至一值辅助函数。

/// 返回有限非负颜色通道，否则使用中性回退。
fn finite_channel(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0) // HDR线性色允许超过一但不允许负通道。
    } else {
        fallback // 非有限色调回退中性白通道。
    } // 结束颜色通道规范化分支。
} // 结束有限颜色通道辅助函数。

#[cfg(test)] // 纯逻辑与最小ECS测试锁定距离映射及相机隔离。
mod tests {
    use super::*; // 测试访问雾参数计算和ECS应用系统。

    #[test] // 能见度降低时雾必须连续靠近相机且透明度增强。
    fn reduced_visibility_moves_fog_closer_and_increases_alpha() {
        let clear = WeatherOutput::default(); // 晴天使用完整能见度与中性色调。
        let mist = WeatherOutput {
            visibility: 0.35,                             // 使用首版薄雾目标能见度。
            tint: LinearRgba::new(0.72, 0.78, 0.82, 1.0), // 使用薄雾浅冷灰目标色。
            ..clear // 从相同字段构造只改变雾相关参数的薄雾样本。
        };
        let sky = Color::srgb(0.30, 0.45, 0.65); // 模拟世界时间已经写入的日间天空色。
        let clear_profile = weather_fog_profile(&clear, sky, true); // 计算晴天雾参数。
        let mist_profile = weather_fog_profile(&mist, sky, true); // 计算薄雾参数。
        assert!(mist_profile.start < clear_profile.start); // 薄雾必须更早开始积累。
        assert!(mist_profile.end < clear_profile.end); // 薄雾必须更近达到目标雾量。
        assert!(mist_profile.color.alpha() > clear_profile.color.alpha()); // 薄雾透明度必须高于晴天零雾。
    } // 结束能见度雾映射测试。

    #[test] // 午夜雨雾必须继承最终夜空亮度，不能被固定天气灰色抬成明亮雾墙。
    fn weather_fog_preserves_day_night_relative_brightness() {
        let mist = WeatherOutput {
            visibility: 0.35, // 使用首版薄雾目标能见度和最大雾透明度。
            tint: LinearRgba::new(0.72, 0.78, 0.82, 1.0), // 使用薄雾冷灰乘色。
            ..default()       // 其余天气通道保持晴天默认值。
        }; // 构造相同薄雾天气参数，只改变最终天空时段。
        let day = weather_fog_profile(&mist, Color::srgb(0.30, 0.45, 0.65), true); // 模拟日间最终天空。
        let night = weather_fog_profile(&mist, Color::srgb(0.008, 0.012, 0.035), true); // 模拟午夜深蓝黑最终天空。
        let day = day.color.to_linear(); // 在同一线性空间比较日夜雾亮度。
        let night = night.color.to_linear(); // 读取午夜雾的实际渲染色通道。
        let day_peak = day.red.max(day.green).max(day.blue); // 日间最亮通道作为相对基准。
        let night_peak = night.red.max(night.green).max(night.blue); // 午夜最亮通道不得接近日间灰雾。
        assert!(night_peak < day_peak * 0.1); // 午夜雾至少比同天气日间暗一个数量级。
        assert!(night.blue > night.red); // 午夜雾仍保留深蓝夜色，不退化成中性灰墙。
        assert_eq!(night.alpha, day.alpha); // 时段只改变雾色亮度，天气能见度对应的透明度保持一致。
    } // 结束雾色昼夜亮度回归测试。

    #[test] // 系统只能给PrimarySceneView插入距离雾，普通菜单或辅助相机不受影响。
    fn fog_system_only_targets_gameplay_camera_marker() {
        let mut app = App::new(); // 使用无渲染插件的最小ECS世界验证组件隔离。
        app.insert_resource(WeatherOutput::default()); // 提供系统必需的连续天气输出。
        app.insert_resource(ClearColor(Color::srgb(0.2, 0.3, 0.5))); // 提供世界时间最终天空色。
        app.add_systems(Update, apply_weather_fog); // 只注册被测距离雾系统。
        let gameplay_camera = app.world_mut().spawn(PrimarySceneView).id(); // 生成玩法主相机标记实体。
        let auxiliary_camera = app.world_mut().spawn_empty().id(); // 生成不带标记的小地图或菜单相机替身。
        app.update(); // 执行天气雾同步并应用延迟组件插入。
        assert!(app.world().get::<DistanceFog>(gameplay_camera).is_some()); // 玩法主相机必须获得距离雾。
        assert!(app.world().get::<DistanceFog>(auxiliary_camera).is_none()); // 普通相机实体不得被天气接管。
    } // 结束玩法相机雾隔离测试。

    #[test] // F12打开时正式雾只迁移到作者相机，关闭后必须完整归还玩法相机。
    fn authoring_preview_switches_weather_fog_camera_without_leaking() {
        let mut app = App::new(); // 最小ECS世界足以验证显式相机所有权切换。
        app.insert_resource(WeatherOutput {
            visibility: 0.35, // 使用明显薄雾，确保插入的是可观察正式参数。
            ..default()       // 其余通道保持合法天气输出。
        });
        app.insert_resource(ClearColor(Color::srgb(0.2, 0.3, 0.5))); // 提供昼夜系统最终天空色。
        app.insert_resource(WeatherAuthoringPreview::new(true)); // 模拟F12把天气表现锚点切到作者相机。
        app.add_systems(Update, apply_weather_fog); // 注册唯一被测同步系统。
        let gameplay_camera = app.world_mut().spawn(PrimarySceneView).id(); // 玩法窗口相机保持存在但被全屏编辑器覆盖。
        let preview_camera = app.world_mut().spawn(WeatherPreviewCamera).id(); // F12离屏视口显式取得天气资格。
        app.update(); // 作者预览活动时执行第一次相机选择。
        assert!(app.world().get::<DistanceFog>(gameplay_camera).is_none()); // 隐藏玩法相机不得获得作者预览雾。
        assert!(app.world().get::<DistanceFog>(preview_camera).is_some()); // 作者视口必须显示同一正式距离雾。

        *app.world_mut().resource_mut::<WeatherAuthoringPreview>() =
            WeatherAuthoringPreview::default(); // 模拟同帧关闭F12并归还自然天气。
        app.update(); // 延迟命令应插入玩法雾并移除作者临时组件。
        assert!(app.world().get::<DistanceFog>(gameplay_camera).is_some()); // 玩法相机重新取得正式天气雾。
        assert!(app.world().get::<DistanceFog>(preview_camera).is_none()); // 作者相机不得残留上次会话雾组件。
    } // 结束F12距离雾所有权切换测试。

    #[test] // F12透视与正交切换时距离雾必须同帧清理并能重新建立。
    fn authoring_orthographic_view_removes_distance_fog_and_perspective_restores_it() {
        let mut app = App::new();
        app.insert_resource(WeatherOutput {
            visibility: 0.35, // 使用明显薄雾验证组件确实受视图边界控制。
            ..default()
        });
        app.insert_resource(ClearColor(Color::srgb(0.2, 0.3, 0.5)));
        app.insert_resource(WeatherAuthoringPreview::new(true));
        app.add_systems(Update, apply_weather_fog);
        let preview_camera = app
            .world_mut()
            .spawn((
                WeatherPreviewCamera,
                Projection::Perspective(PerspectiveProjection::default()),
            ))
            .id();
        app.update();
        assert!(app.world().get::<DistanceFog>(preview_camera).is_some()); // 透视作者画面保留完整正式薄雾。

        app.world_mut()
            .entity_mut(preview_camera)
            .insert(Projection::Orthographic(
                OrthographicProjection::default_3d(),
            ));
        app.update();
        assert!(app.world().get::<DistanceFog>(preview_camera).is_none()); // 正交测绘视图绝不残留上帧雾组件。

        app.world_mut()
            .entity_mut(preview_camera)
            .insert(Projection::Perspective(PerspectiveProjection::default()));
        app.update();
        assert!(app.world().get::<DistanceFog>(preview_camera).is_some()); // 返回透视后由同一正式系统恢复。
    }

    #[test] // 菜单状态即使玩法相机尚未清理也必须把天气雾透明度归零。
    fn menu_state_disables_fog_on_stale_gameplay_camera() {
        let mut app = App::new(); // 使用最小ECS世界模拟场景切换清理间隙。
        let output = WeatherOutput {
            visibility: 0.35, // 保留离开玩法地图前的薄雾能见度。
            ..default()       // 其余天气通道保持晴天默认值。
        }; // 从晴天结构构造薄雾输出。
        app.insert_resource(output); // 写入仍保留的恶劣天气输出。
        app.insert_resource(State::new(ScenePhase::Idle)); // 当前已经进入主菜单。
        app.insert_resource(ClearColor(Color::BLACK)); // 提供菜单清屏色。
        app.add_systems(Update, apply_weather_fog); // 注册被测距离雾系统。
        let camera = app.world_mut().spawn(PrimarySceneView).id(); // 模拟尚未被场景清理的玩法相机。
        app.update(); // 菜单阶段同步应插入透明天气雾。
        let fog = app.world().get::<DistanceFog>(camera).unwrap(); // 读取菜单阶段的残留相机雾。
        assert_eq!(fog.color.alpha(), 0.0); // 主菜单不得显示天气雾。
    } // 结束菜单残留相机雾测试。
} // 结束天气距离雾测试模块。
