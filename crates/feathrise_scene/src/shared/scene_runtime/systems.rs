// 正式地图公共工具：消除城市与独立探索场景的重复代码。

#[cfg(test)]
use bevy::camera::visibility::RenderLayers;
use bevy::light::SunDisk; // 所有玩法天体方向光显式携带可由动态地图更新的光盘范围。
use bevy::prelude::*;
use feathrise_atmosphere::{WorldCelestialLight, WorldCelestialLightKind}; // 动态地图显式生成太阳灯和月亮灯。
use feathrise_scene_core::view::PrimarySceneView;
use feathrise_settings::SceneDisplaySettings; // 读取 F1 测试面板已加载的地图规划显隐设置。
use feathrise_scene_core::state::{ScenePhase, LoadingProgress, LoadingStage, SceneStartupGate};

use super::types::SceneEnvironment;

/// Blender与Bevy共同约定的地图规划对象统一名称前缀。
const BLENDER_PLANNING_OBJECT_PREFIX: &str = "规划_"; // 区域、标签和导线只通过这一个前缀识别。
type PlanningEntityData = (Entity, &'static Name, Option<&'static Visibility>);
type ChangedPlanningEntityFilter = Or<(Added<Name>, Changed<Visibility>)>;

/// 构造玩法天体方向光，并显式保留两类正式阴影资格。
fn gameplay_directional_light(illuminance: f32, color: Color) -> DirectionalLight {
    DirectionalLight {
        illuminance,                   // 使用地图作者配置的日景照度。
        shadow_maps_enabled: true,     // 标准级联阴影继续作为主要远距离阴影。
        contact_shadows_enabled: true, // 相机设置开启时允许补强近距离接地阴影。
        color,                         // 使用地图作者配置的高角度日光颜色。
        ..default()                    // 偏置等其余字段沿用 Bevy 正式默认值。
    }
} // 结束玩法天体方向光构造。

/// 判断导入场景节点是否属于可切换的Blender地图规划对象。
fn is_blender_planning_object_name(name: &Name) -> bool {
    name.as_str().starts_with(BLENDER_PLANNING_OBJECT_PREFIX) // 只匹配名称开头，避免普通对象名称中偶然包含“规划”。
} // 结束Blender地图规划对象名称判断。

/// 把当前规划显隐状态应用到一组命名场景节点。
fn apply_blender_planning_visibility<'a>(
    commands: &mut Commands, // 通过延迟命令覆盖导入节点的Visibility组件。
    named_entities: impl Iterator<Item = (Entity, &'a Name, Option<&'a Visibility>)>, // 接收全部节点或本帧新增/变化节点。
    desired_visibility: Visibility, // 开启时使用Inherited，关闭时使用Hidden。
) {
    for (entity, name, current_visibility) in named_entities {
        // 遍历调用方提供的命名节点集合。
        if !is_blender_planning_object_name(name) {
            // 普通地形、水域、角色和玩法地标都不属于规划对象。
            continue; // 跳过没有统一规划前缀的节点。
        } // 完成规划对象名称筛选。
        if current_visibility == Some(&desired_visibility) {
            // 节点已经处于目标状态时无需再次触发组件变更。
            continue; // 避免Changed<Visibility>在稳定状态下反复唤醒系统。
        } // 完成当前显隐状态比较。
        commands.entity(entity).insert(desired_visibility); // 写入目标Visibility并保留场景层级与其他组件。
    } // 完成全部命名节点显隐同步。
} // 结束地图规划节点批量显隐应用。

/// 同步Blender地图规划对象显隐，并处理首次实例化与GLB热重载。
pub fn sync_blender_planning_visibility(
    mut commands: Commands,      // 给匹配节点写入目标Visibility组件。
    settings: Res<SceneDisplaySettings>, // 读取测试面板“显示规划”已持久化的当前状态。
    mut previous_setting: Local<Option<bool>>, // 缓存上次状态，仅在开关变化时扫描全部命名节点。
    changed_named_entities: Query<PlanningEntityData, ChangedPlanningEntityFilter>, // 捕获新场景节点和热重载恢复的Visibility。
    all_named_entities: Query<PlanningEntityData>, // 开关变化时覆盖当前世界全部命名节点。
) {
    let show_planning = settings.show_map_planning; // 获取本帧应显示还是隐藏规划对象。
    let desired_visibility = if show_planning {
        Visibility::Inherited // 开启时恢复场景层级继承，不强制突破隐藏的地图根。
    } else {
        Visibility::Hidden // 关闭时直接隐藏规划对象及其网格子节点。
    }; // 完成本帧目标Visibility选择。
    let setting_changed = *previous_setting != Some(show_planning); // 首次运行和按钮切换都需要全量同步。
    if setting_changed {
        // 开关变化会影响已实例化的全部规划对象。
        apply_blender_planning_visibility(
            &mut commands,             // 使用当前系统命令队列写入可见性。
            all_named_entities.iter(), // 扫描当前世界全部命名节点。
            desired_visibility,        // 应用按钮切换后的统一目标状态。
        ); // 完成全量规划显隐同步。
    } else {
        // 设置稳定时只处理新实例化或热重载修改的节点。
        apply_blender_planning_visibility(
            &mut commands,                 // 使用当前系统命令队列写入可见性。
            changed_named_entities.iter(), // 只扫描本帧新增名称或变化可见性的节点。
            desired_visibility,            // 重新套用当前稳定设置。
        ); // 完成增量规划显隐同步。
    } // 完成全量与增量更新路径选择。
    *previous_setting = Some(show_planning); // 保存本帧状态供下一帧判断按钮是否变化。
} // 结束Blender地图规划对象显隐同步。

/// 标记当前玩法场景的地形碰撞已经可用于生成动态角色。
pub fn mark_scene_collision_ready(commands: &mut Commands, scene: ScenePhase) {
    commands.queue(move |world: &mut World| {
        let scene_display_name = world
            .get_resource::<LoadingProgress>()
            .map(|progress| progress.target_display_name(scene).to_owned())
            .unwrap_or_else(|| scene.display_name().to_owned()); // 无加载事务时才使用场景枚举名称兜底。
        let mut gate_matched = false;
        if let Some(mut gate) = world.get_resource_mut::<SceneStartupGate>()
            && gate.target == scene
        {
            gate.collision_ready = true;
            gate_matched = true;
        }

        if gate_matched {
            let scene_assets_ready = world
                .get_resource::<SceneStartupGate>()
                .is_none_or(|gate| gate.scene_assets_ready); // 先结束不可变借用，再写加载进度资源。
            if let Some(mut progress) = world.get_resource_mut::<LoadingProgress>() {
                if scene_assets_ready {
                    progress.stage = LoadingStage::DynamicEntities; // 无阻塞大资源时直接进入角色生成阶段。
                    progress.completed = 0; // 应用域与子地图尚待完成门统计。
                    progress.total = 1; // 应用域稍后发布自己的真实进度。
                    progress.label = format!("正在生成{scene_display_name}应用内容..."); // 显示编辑器地图清单中的实际名称。
                } else {
                    progress.stage = LoadingStage::SceneAssets; // 保留地图资源加载阶段。
                    progress.completed = 0; // 地图私有队列将在下一帧写入真实包数。
                    progress.total = 1; // 防止队列首次更新前出现零分母。
                    progress.label = format!("正在准备{scene_display_name}大资源队列..."); // 明确具体地图碰撞已完成但仍需资源。
                } // 完成地图是否存在阻塞资源队列的分支。
            }
            info!("{scene_display_name}地图碰撞已就绪，开始生成动态实体"); // 日志与玩家加载提示使用同一名称快照。
        }
    });
}

/// 几何、资产、子地图和显式注册应用域全数完成后，解除当前场景门。
pub fn complete_scene_startup(world: &mut World) {
    let Some(gate) = world.get_resource::<SceneStartupGate>() else { return; };
    if gate.ready || gate.target != *world.resource::<State<ScenePhase>>().get()
        || !gate.collision_ready || !gate.scene_assets_ready { return; }
    let status = world.get_resource::<crate::SceneApplicationHooks>().map_or(
        crate::SceneApplicationStartupStatus { completed: 0, total: 0, error: None },
        |hooks| (hooks.startup)(world, gate));
    let error = gate.sub_maps_error.clone().or(status.error.clone());
    let ready = status.completed == status.total && error.is_none() && gate.sub_maps_ready;
    let target = gate.target;
    let display = world.get_resource::<LoadingProgress>()
        .map(|value| value.target_display_name(target).to_owned())
        .unwrap_or_else(|| target.display_name().to_owned());
    let mut gate = world.resource_mut::<SceneStartupGate>();
    gate.application_ready = status.completed == status.total && status.error.is_none();
    gate.application_error = status.error;
    gate.ready = ready;
    if let Some(mut progress) = world.get_resource_mut::<LoadingProgress>() {
        progress.stage = if error.is_some() { LoadingStage::Failed }
            else if ready { LoadingStage::Complete } else { LoadingStage::DynamicEntities };
        progress.completed = if ready { 1 } else { status.completed };
        progress.total = if ready { 1 } else { status.total.max(1) };
        progress.label = match error {
            Some(error) => format!("{display}应用内容加载失败：{error}"),
            None if ready => format!("{display}加载完成"),
            None => format!("正在准备{display}应用内容..."),
        };
    }
}

/// 生成动态地图的主相机与太阳、月亮；地形和碰撞由作者内容完成后放行。
pub fn spawn_scene_common(
    commands: &mut Commands,
    clear_color: &mut ResMut<ClearColor>,
    env: &SceneEnvironment,
    scene: ScenePhase,
    layers: &crate::SceneViewLayers,
) -> (Entity, Entity, Option<Entity>) {
    **clear_color = ClearColor(env.clear_color);
    let camera_entity = commands.spawn((
        Name::new("PrimarySceneView"),
        PrimarySceneView,
        Camera3d::default(),
        Transform::from_xyz(0.0, 50.0, -50.0).looking_at(Vec3::ZERO, Vec3::Y),
        IsDefaultUiCamera,
        layers.camera.clone(), // 主视图统一覆盖角色、天气与技能层。
        DespawnOnExit(scene),
    )).id();

    // 【功能设计】两盏灯共享首帧安全配置，正式全局与地图配置在同帧ApplyLighting前接管。
    let base_profile = WorldCelestialLight::new(
        env.light_illuminance, env.light_color, env.clear_color,
    );
    let profile = |kind| WorldCelestialLight::new_authored(
        kind,
        base_profile.base_illuminance,
        base_profile.base_color,
        base_profile.night_peak_illuminance,
        base_profile.night_color,
        base_profile.azimuth_offset_radians,
        base_profile.angular_diameter_radians,
        base_profile.day_clear_color,
    );
    let rotation = Quat::from_euler(
        EulerRot::ZYX, 0.0, env.light_rotation.0, env.light_rotation.1,
    );
    let sun = commands.spawn((
        Name::new("SunDirectionalLight"),
        profile(WorldCelestialLightKind::Sun),
        SunDisk::EARTH,
        gameplay_directional_light(env.light_illuminance, env.light_color),
        layers.light.clone(),
        Transform::from_rotation(rotation),
        DespawnOnExit(scene),
    )).id();
    let moon = commands.spawn((
        Name::new("MoonDirectionalLight"),
        profile(WorldCelestialLightKind::Moon),
        SunDisk::OFF, // 月亮使用独立视觉圆盘；Solari角直径由正式配置更新。
        gameplay_directional_light(0.0, base_profile.night_color),
        layers.light.clone(),
        Transform::from_rotation(rotation),
        DespawnOnExit(scene),
    )).id();
    (camera_entity, sun, Some(moon)) // 继续通过明确实体同步配置，避免扫描世界寻找灯光。
}

#[cfg(test)] // 规划名称与显隐回归测试只在测试构建中编译。
mod tests {
    use super::*; // 复用统一前缀匹配函数和运行时显隐系统。
    const AUXILIARY_TEST_LAYER: usize = 5; // 独立辅助视图夹具，不从应用库读取层号。

    #[test]
    fn default_scene_layers_exclude_application_and_authoring_layers() {
        let layers = crate::SceneViewLayers::default();
        assert!(layers.camera.intersects(&RenderLayers::layer(0)));
        assert!(layers.light.intersects(&RenderLayers::layer(0)));
        for layer in [6, 7, 8, 9, 31, AUXILIARY_TEST_LAYER] {
            assert!(!layers.camera.intersects(&RenderLayers::layer(layer)));
            assert!(!layers.light.intersects(&RenderLayers::layer(layer)));
        }
    }

    #[test] // 玩法天体光必须为相机画质开关提供接触阴影光源资格。
    fn celestial_light_enables_standard_and_contact_shadows() {
        let color = Color::srgb(0.9, 0.8, 0.7); // 构造可辨识的作者日光颜色。
        let light = gameplay_directional_light(42_000.0, color); // 走正式方向光构造路径。
        assert!(light.shadow_maps_enabled); // 接触阴影不得替代既有标准级联阴影。
        assert!(light.contact_shadows_enabled); // 开启相机组件后必须存在实际可投射的正式光源。
        assert_eq!(light.illuminance, 42_000.0); // 新资格不得覆盖地图作者照度。
        assert_eq!(light.color, color); // 新资格不得覆盖地图作者颜色。
    } // 结束玩法天体方向光阴影资格回归测试。

    #[test] // 统一前缀必须覆盖区域、标签和导线，同时排除普通地图对象。
    fn blender_planning_name_uses_one_prefix() {
        let planning_names = [
            Name::new("规划_区域_府城核心区"), // 区域色块使用统一前缀和区域类别。
            Name::new("规划_标签_府城核心区"), // 文字标签使用统一前缀和标签类别。
            Name::new("规划_导线_主官道与路线出口"), // 道路导线使用统一前缀和导线类别。
        ]; // 完成代表性规划名称表。
        for name in &planning_names {
            // 逐项验证全部规划类别都可被同一判断识别。
            assert!(is_blender_planning_object_name(name)); // 每个统一前缀名称都必须返回true。
        } // 完成规划名称识别验证。
        assert!(!is_blender_planning_object_name(&Name::new(
            "开封府境地形_可碰撞", // 真实地形名称不得被规划开关影响。
        ))); // 完成普通地形反例断言。
        assert!(!is_blender_planning_object_name(&Name::new(
            "KaifengGrainRoadMarker", // Bevy生成的玩法地标明确排除在Blend规划开关之外。
        ))); // 完成玩法地标反例断言。
    } // 结束统一规划前缀测试。

    #[test] // 规划节点必须响应默认隐藏、按钮开启、新节点和热重载恢复场景。
    fn planning_visibility_follows_setting_and_scene_changes() {
        let mut app = App::new(); // 创建不加载渲染器的最小Bevy测试应用。
        app.add_plugins(MinimalPlugins) // 注册Update调度和本地资源生命周期。
            .insert_resource(SceneDisplaySettings::default()) // 默认关闭规划显示，模拟正式启动。
            .add_systems(Update, sync_blender_planning_visibility); // 注册真实运行时显隐系统。
        let planning = app
            .world_mut() // 获取测试World生成代表性规划节点。
            .spawn((
                Name::new("规划_区域_府城核心区"), // 使用统一前缀的Blender规划对象。
                Visibility::Inherited,             // 模拟GLB导入后的默认可见状态。
            )) // 完成规划节点生成。
            .id(); // 保存实体用于后续状态断言。
        let terrain = app
            .world_mut() // 继续生成不受开关影响的真实地形节点。
            .spawn((
                Name::new("开封府境地形_可碰撞"), // 普通地形不使用规划前缀。
                Visibility::Inherited,            // 地形保持场景默认可见状态。
            )) // 完成普通地形节点生成。
            .id(); // 保存实体用于反例断言。
        app.update(); // 首帧应用默认关闭设置并执行延迟命令。
        assert_eq!(
            app.world().get::<Visibility>(planning), // 读取规划节点同步后的可见性。
            Some(&Visibility::Hidden),               // 默认状态必须隐藏规划对象。
        ); // 完成默认隐藏断言。
        assert_eq!(
            app.world().get::<Visibility>(terrain), // 读取普通地形当前可见性。
            Some(&Visibility::Inherited),           // 普通地形必须保持原有场景继承状态。
        ); // 完成非规划对象不受影响断言。
        app.world_mut()
            .resource_mut::<SceneDisplaySettings>() // 获取测试面板共享设置资源。
            .show_map_planning = true; // 模拟点击“显示规划: 开”。
        app.update(); // 应用设置变化并全量同步已有规划节点。
        assert_eq!(
            app.world().get::<Visibility>(planning), // 读取开启后的规划节点可见性。
            Some(&Visibility::Inherited),            // 开启时必须恢复场景层级继承。
        ); // 完成规划开启断言。
        app.world_mut()
            .resource_mut::<SceneDisplaySettings>() // 再次获取设置模拟关闭按钮。
            .show_map_planning = false; // 模拟点击“显示规划: 关”。
        app.update(); // 关闭后全量隐藏已有规划节点。
        let switched_scene_planning = app
            .world_mut() // 模拟切换地图后新GLB刚实例化的规划节点。
            .spawn((
                Name::new("规划_导线_地图边界_5公里"), // 新场景节点仍使用唯一前缀。
                Visibility::Inherited,                 // 新GLB导入时对象默认继承场景可见性。
            )) // 完成切换地图后的新规划节点生成。
            .id(); // 保存新节点用于增量同步断言。
        app.update(); // Added<Name>增量路径必须继承当前关闭状态。
        assert_eq!(
            app.world().get::<Visibility>(switched_scene_planning), // 读取新地图规划节点的最终可见性。
            Some(&Visibility::Hidden), // 切换地图不得让关闭的规划对象短暂保持可见。
        ); // 完成地图切换后规划状态断言。
        app.world_mut()
            .entity_mut(planning) // 模拟GLB热重载重新写回导入可见性。
            .insert(Visibility::Inherited); // 热重载场景把节点恢复为默认继承状态。
        app.update(); // Changed<Visibility>增量路径必须重新套用关闭设置。
        assert_eq!(
            app.world().get::<Visibility>(planning), // 读取热重载后的最终规划可见性。
            Some(&Visibility::Hidden),               // 当前关闭状态必须覆盖热重载写回值。
        ); // 完成热重载显隐回归断言。
    } // 结束规划节点运行时显隐测试。
} // 结束场景公共系统测试模块。
