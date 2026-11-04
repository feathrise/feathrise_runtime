use super::*; // 测试访问降雨同步、标记与雨速函数。

#[test] // 风力只能增加横向偏移，不能减慢雨丝基本下落速度。
fn wind_continuously_tilts_rain_velocity() {
    let calm = rain_velocity(0.0, DEFAULT_WIND_DIRECTION); // 读取无风垂直雨速。
    let windy = rain_velocity(1.0, DEFAULT_WIND_DIRECTION); // 读取中性默认+X风向满风雨速。
    assert_eq!(calm, Vec3::new(0.0, -30.0, 0.0)); // 无风必须完全垂直下落。
    assert_eq!(windy.x, 7.0); // 默认满风只沿+X产生横向速度。
    assert_eq!(windy.z, calm.z); // 中性默认值不得残留第二方向偏置。
    assert_eq!(windy.y, calm.y); // 雨量和风力不改变基本下落速度。
    assert_eq!(
        rain_velocity(1.0, Vec2::new(0.0, 3.0)),
        Vec3::new(0.0, -30.0, 7.0)
    ); // 调用方输入的其他方向仍可正常归一化。
} // 结束动态雨速测试。

#[test] // 无效风向必须统一回退中性+X方向，不能向GPU传播NaN。
fn invalid_rain_wind_uses_neutral_default_direction() {
    let expected = rain_velocity(1.0, DEFAULT_WIND_DIRECTION);
    for direction in [Vec2::ZERO, Vec2::splat(f32::NAN)] {
        assert_eq!(rain_velocity(1.0, direction), expected);
    }
}

#[test] // F1暂停预热无论停留多少帧都只允许一次有限静态批次。
fn paused_preview_spawn_count_is_single_bounded_batch() {
    assert_eq!(paused_preview_spawn_count(0.0, EffectsQuality::High), 0); // 无雨预览不生成静态粒子。
    assert_eq!(paused_preview_spawn_count(0.4, EffectsQuality::High), 3_328); // 高档小雨保持原有三千三百二十八条基线。
    assert_eq!(paused_preview_spawn_count(1.0, EffectsQuality::High), 8_320); // 高档大雨保持原有八千三百二十条基线。
    assert_eq!(
        paused_preview_spawn_count(1.0, EffectsQuality::Ultra),
        10_400
    ); // 极高档单批仍远低于固定GPU容量。
    assert_eq!(paused_preview_spawn_count(f32::NAN, EffectsQuality::Low), 0); // 损坏雨量安全按停雨处理。
    let (minimum, maximum) = rain_spawn_height_bounds(); // 读取相对相机的正式雨幕垂直范围。
    assert!(minimum < 0.0 && maximum > 0.0); // 单批静态预览必须同时覆盖相机下方和上方，不能冻结在头顶平面。
} // 结束暂停雨丝单批数量测试。

#[test] // 四档特效质量必须线性缩放正常降雨批次，并让高档严格保持当前基线。
fn effects_quality_scales_rain_emission_and_stable_frames_do_not_rewrite_spawner() {
    assert_eq!(rain_burst_particle_count(1.0, EffectsQuality::Low), 1_040.0); // 低档把满雨量每批减半。
    assert_eq!(
        rain_burst_particle_count(1.0, EffectsQuality::Medium),
        1_560.0
    ); // 中档保留四分之三。
    assert_eq!(
        rain_burst_particle_count(1.0, EffectsQuality::High),
        2_080.0
    ); // 高档保持现有正式雨幕密度。
    assert_eq!(
        rain_burst_particle_count(1.0, EffectsQuality::Ultra),
        2_600.0
    ); // 极高增加四分之一。

    let mut app = App::new(); // 使用无GPU最小ECS应用验证运行时切档和稳定帧写入。
    app.insert_resource(Assets::<EffectAsset>::default()); // 提供雨效资产容器以创建唯一发射体。
    app.insert_resource(WeatherOutput {
        precipitation: 1.0, // 满雨量便于精确断言四档粒子数。
        ..default()         // 其余天气输出保持安全默认值。
    }); // 完成大雨输出资源。
    app.insert_resource(RenderSettings::default()); // 默认高档必须保持现有雨量基线。
    app.insert_resource(State::new(ScenePhase::Active)); // 动态地图玩法场景允许视觉天气。
    app.add_systems(Update, sync_weather_rain); // 只运行主世界降雨同步，不安装Hanabi渲染插件。
    app.world_mut().spawn((PrimarySceneView, Transform::default())); // 提供玩法主相机定位。
    app.update(); // 首帧创建使用高档数量的发射体。
    app.world_mut().clear_trackers(); // 清除创建帧变更标记以检查下一稳定帧。
    app.update(); // 同一雨量和同一质量下不应重复修改发射器组件。
    let (stable_count, stable_changed) = {
        let world = app.world_mut(); // 在单个世界借用中读取发射器计数与变更状态。
        let mut query = world.query::<Ref<EffectSpawner>>(); // Ref允许检查组件级变更检测。
        let spawner = query.single(world).unwrap(); // 正式玩法场景应只有唯一雨发射体。
        (spawner.settings.count(), spawner.is_changed()) // 复制断言值后释放世界借用。
    }; // 完成稳定帧状态读取。
    assert_eq!(stable_count, CpuValue::Single(2_080.0)); // 默认高档必须保持原每批数量。
    assert!(!stable_changed); // 稳定帧不得重复写Hanabi发射器并触发无意义同步。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .effects_quality = EffectsQuality::Low; // 模拟玩家在画质页即时切到低特效质量。
    app.update(); // 下一帧同步系统应只更新发射批次数量。
    let low_count = {
        let world = app.world_mut(); // 读取切档后的唯一发射器。
        let mut query = world.query::<&EffectSpawner>(); // 只需检查目标发射参数。
        query.single(world).unwrap().settings.count() // 返回低档每批数量。
    }; // 完成低档数量读取。
    assert_eq!(low_count, CpuValue::Single(1_040.0)); // 已有雨发射体必须立即继承低档倍率。
} // 结束降雨特效质量与稳定帧回归测试。

#[test] // 正式雨幕范围、雨丝长度和GPU预算必须保持用户确认的视觉设计值。
fn expanded_rain_design_constants_fit_gpu_capacity() {
    assert_eq!(RAIN_RADIUS, 100.0); // 水平圆盘半径固定一百米，对应直径二百米。
    assert_eq!(RAIN_FORWARD_OFFSET, 20.0); // 圆盘沿相机水平视线前移二十米以扩大前方覆盖。
    assert_eq!(RAIN_PARTICLES_PER_BURST, 2_080.0); // 每批两千零八十条保持扩大前的单位面积密度。
    assert_eq!(RAIN_EFFECT_CAPACITY, 131_072); // GPU容量固定为十三万一千零七十二条。
    assert_eq!(RAIN_STREAK_START_LENGTH, 0.65); // 新雨丝起点长度固定六十五厘米。
    assert_eq!(RAIN_STREAK_END_LENGTH, 0.35); // 精确设计值同时保证雨丝随生命周期缩短且末端仍为正长度。
    let legacy_density = 100.0 / 22.0_f32.powi(2); // 读取扩大前每批粒子数除以半径平方的相对面密度。
    let expanded_density = RAIN_PARTICLES_PER_BURST / RAIN_RADIUS.powi(2); // 计算扩大后二百米直径雨幕的相对面密度。
    assert!(((expanded_density / legacy_density) - 1.0).abs() < 0.01); // 调整到每批两千零八十条后单位面积密度误差必须低于百分之一。
    let live_burst_upper_bound = (RAIN_LIFETIME / RAIN_BURST_PERIOD).ceil() as u32 + 1; // 额外保留一批覆盖离散时钟边界。
    let live_particle_upper_bound = RAIN_PARTICLES_PER_BURST.ceil() as u32 * live_burst_upper_bound; // 计算满雨量下可能同时存活的离散粒子上沿。
    assert!(RAIN_EFFECT_CAPACITY >= live_particle_upper_bound * 2); // GPU容量至少保留两倍上沿，避免卡帧集中提交立即饱和。
    let ultra_live_particle_upper_bound =
        (RAIN_PARTICLES_PER_BURST * EffectsQuality::Ultra.particle_multiplier()).ceil() as u32
            * live_burst_upper_bound; // 计算极高特效档满雨量下的稳态存活上沿。
    assert!(RAIN_EFFECT_CAPACITY >= ultra_live_particle_upper_bound); // 固定容量必须完整容纳极高档，切档不得依赖截断降级。
} // 结束正式雨幕设计常量与容量预算测试。

#[test] // 构建后的Hanabi资产必须实际使用扩大容量并保留全局粒子模拟空间。
fn rain_effect_uses_expanded_capacity_and_global_space() {
    let effect = build_rain_effect(); // 使用正式构造入口生成可检查的纯CPU雨效果资产。
    assert_eq!(effect.capacity(), RAIN_EFFECT_CAPACITY); // 效果资产必须采用十三万一千零七十二粒子容量而非旧值。
    assert_eq!(effect.simulation_space, SimulationSpace::Global); // 已生成雨丝必须留在世界位置，不能随前移中的发射圆盘整体拖动。
} // 结束Hanabi雨效果容量与全局空间测试。

#[test] // 相机水平朝向决定雨幕前移，接近垂直观察时必须安全回退为只抬高。
fn rain_emitter_moves_forward_and_handles_vertical_camera() {
    let forward_camera = Transform::from_xyz(3.0, 5.0, 7.0); // 默认相机朝世界负Z轴观察并使用非零位置。
    assert_eq!(
        rain_emitter_position(&forward_camera), // 读取默认负Z观察方向下的实际发射圆盘位置。
        Vec3::new(3.0, 23.0, -13.0),            // 期望在原坐标基础上向负Z前移二十米并抬高十八米。
    ); // 默认朝向应向负Z前移二十米并抬高十八米。
    let right_camera = Transform::default().looking_to(Vec3::X, Vec3::Y); // 使用语义化观察方向让相机朝世界正X轴观察。
    let right_position = rain_emitter_position(&right_camera); // 计算横向观察时的发射圆盘目标位置。
    assert!((right_position - Vec3::new(20.0, 18.0, 0.0)).length() < 0.000_01); // 正X朝向应只在X轴前移并保持固定高度。
    let vertical_direction = Vec3::new(0.000_1, -1.0, 0.0); // 构造低于前移阈值的近垂直向下观察方向。
    let vertical_camera = Transform::default().looking_to(vertical_direction, Vec3::Z); // 让测试相机使用带微小浮点水平残差的近垂直朝向。
    let vertical_position = rain_emitter_position(&vertical_camera); // 计算接近垂直朝向的安全回退位置。
    assert!((vertical_position - Vec3::new(0.0, 18.0, 0.0)).length() < 0.000_01); // 垂直观察不得生成水平偏移或非有限坐标。
} // 结束雨发射圆盘前移与垂直回退测试。

#[test] // F1暂停创建雨效时循环发射器必须立即关闭，连续Update不能增加spawn_count。
fn paused_virtual_time_keeps_rain_spawner_inactive() {
    let mut app = App::new(); // 使用无GPU最小ECS应用验证Update阶段发射器状态。
    app.insert_resource(Assets::<EffectAsset>::default()); // 提供雨效资产容器以允许创建实体。
    let output = WeatherOutput {
        precipitation: 1.0, // 满雨量进入发射体创建路径。
        ..default()         // 其余天气通道保持晴天默认值。
    }; // 从晴天输出构造大雨预览。
    app.insert_resource(output); // 插入连续天气输出。
    app.insert_resource(State::new(ScenePhase::Active)); // 动态地图玩法场景允许F1视觉天气。
    let mut virtual_time = Time::<Virtual>::default(); // 构造暂停感知虚拟时钟。
    virtual_time.pause(); // 模拟F12编辑器打开后的统一暂停。
    app.insert_resource(virtual_time); // 雨系统据此关闭自动burst。
    app.add_systems(Update, sync_weather_rain); // 只运行Update同步，不安装Hanabi GPU插件。
    app.world_mut().spawn((PrimarySceneView, Transform::default())); // 提供玩法主相机位置。
    app.update(); // 创建带显式inactive EffectSpawner的雨实体。
    app.update(); // 再运行一帧确认Update不会重新激活发射器。
    let (active, spawn_count, seeded) = {
        let world = app.world_mut(); // 在同一可变世界借用内读取唯一雨发射器。
        let mut query = world.query::<(&EffectSpawner, &WeatherRainEmitter)>(); // 查询循环状态与暂停预热标记。
        let (spawner, emitter) = query.single(world).unwrap(); // 正式玩法场景应只有唯一雨实体。
        (
            spawner.active,
            spawner.spawn_count,
            emitter.paused_preview_seeded,
        ) // 复制断言字段后释放世界借用。
    }; // 完成暂停发射器状态读取。
    assert!(!active); // F1暂停期间绝不能让Hanabi循环burst保持活动。
    assert_eq!(spawn_count, 0); // PostUpdate静态预热未安装时，多帧Update始终保持零计数。
    assert!(!seeded); // 等真实Hanabi编译就绪后才由专用PostUpdate系统标记单批已提交。
} // 结束F1暂停循环发射关闭测试。

#[test] // 自然雨量在运行中变化后打开F1不得把已经铺开的雨幕误删重建。
fn entering_pause_keeps_running_rain_emitter_after_natural_intensity_change() {
    let mut app = App::new(); // 使用最小ECS应用复现自然过渡后打开F1的顺序。
    app.insert_resource(Assets::<EffectAsset>::default()); // 提供雨效资产容器以创建唯一发射体。
    let output = WeatherOutput {
        precipitation: 0.3, // 首帧以三成雨量生成发射体。
        ..default()         // 其余天气通道保持晴天默认值。
    }; // 从晴天输出构造逐渐增强的自然降雨。
    app.insert_resource(output); // 插入连续天气输出供同步系统读取。
    app.insert_resource(State::new(ScenePhase::Active)); // 正式玩法地图允许自然降雨。
    app.insert_resource(Time::<Virtual>::default()); // 初始虚拟时间正常运行。
    app.add_systems(Update, sync_weather_rain); // 注册正式降雨同步系统。
    app.world_mut().spawn((PrimarySceneView, Transform::default())); // 提供玩法主相机定位。
    app.update(); // 创建三成雨量发射体。
    app.world_mut()
        .resource_mut::<WeatherOutput>()
        .precipitation = 0.8; // 模拟自然过渡把雨量提高到八成。
    app.update(); // 正常运行帧应刷新发射体保存的当前雨量基准。
    let before_pause = {
        let world = app.world_mut(); // 在单个世界借用中读取唯一实体与同步后的雨量。
        let mut query = world.query::<(Entity, &WeatherRainEmitter)>(); // 查询天气雨发射体状态。
        let (entity, emitter) = query.single(world).unwrap(); // 连续自然过渡不得创建重复发射体。
        assert!((emitter.preview_precipitation - 0.8).abs() < f32::EPSILON); // 运行期基准必须跟上当前自然雨量。
        entity // 保存实体ID供暂停后比较是否被误删重建。
    }; // 完成暂停前发射体读取。
    app.world_mut().resource_mut::<Time<Virtual>>().pause(); // 模拟同一雨量下打开F1。
    app.update(); // 暂停路径应冻结现有粒子而不是重建空白静态雨幕。
    let after_pause = {
        let world = app.world_mut(); // 读取暂停后的唯一雨发射体实体。
        let mut query = world.query_filtered::<Entity, With<WeatherRainEmitter>>(); // 只匹配天气雨发射体。
        query.single(world).unwrap() // 返回当前唯一实体ID并释放世界借用。
    }; // 完成暂停后发射体读取。
    assert_eq!(after_pause, before_pause); // 自然雨量未因F1输入改变时必须保留原GPU粒子实例。
} // 结束自然降雨进入暂停回归测试。

#[test] // F1暂停期间切换特效质量必须替换唯一静态发射体，不能叠加不同密度批次。
fn paused_effects_quality_change_rebuilds_the_single_rain_emitter() {
    let mut app = App::new(); // 使用无GPU最小ECS应用验证暂停切档实体所有权。
    app.insert_resource(Assets::<EffectAsset>::default()); // 提供雨效资产容器以创建发射体。
    app.insert_resource(WeatherOutput {
        precipitation: 1.0, // 满雨量确保新旧档位的发射数量差异明确。
        ..default()         // 其余天气输出保持安全默认值。
    }); // 完成大雨天气输出资源。
    app.insert_resource(RenderSettings::default()); // 首帧使用高档雨效基线。
    app.insert_resource(State::new(ScenePhase::Active)); // 动态地图玩法场景允许F1视觉天气。
    let mut virtual_time = Time::<Virtual>::default(); // 构造与F1相同的暂停感知时钟。
    virtual_time.pause(); // 暂停期间只能等待一次有限静态预热。
    app.insert_resource(virtual_time); // 降雨系统据此关闭循环发射。
    app.add_systems(Update, sync_weather_rain); // 注册正式雨效同步入口。
    app.world_mut().spawn((PrimarySceneView, Transform::default())); // 提供玩法主相机定位。
    app.update(); // 创建高档暂停雨发射体。
    let high_entity = {
        let world = app.world_mut(); // 在单个世界借用中读取唯一高档实体。
        let mut query = world.query_filtered::<Entity, With<WeatherRainEmitter>>(); // 只匹配天气雨发射体。
        query.single(world).unwrap() // 保存实体ID供切档后比较。
    }; // 完成高档实体读取。

    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .effects_quality = EffectsQuality::Low; // 模拟F1暂停时从设置页切到低特效质量。
    app.update(); // 同帧销毁旧静态实例并创建低档唯一实例。
    let (low_entity, low_quality, low_count) = {
        let world = app.world_mut(); // 在单个世界借用中读取切档后的唯一实体。
        let mut query = world.query::<(Entity, &WeatherRainEmitter, &EffectSpawner)>(); // 同时检查身份、档位和发射参数。
        let (entity, emitter, spawner) = query.single(world).unwrap(); // 切档后仍只能保留一个实例。
        (entity, emitter.effects_quality, spawner.settings.count()) // 复制断言字段后释放世界借用。
    }; // 完成低档实体状态读取。
    assert_ne!(low_entity, high_entity); // 暂停静态粒子无法原地降密度，必须替换干净实例。
    assert_eq!(low_quality, EffectsQuality::Low); // 新实例必须记录当前低档。
    assert_eq!(low_count, CpuValue::Single(1_040.0)); // 新实例首帧使用低档满雨量批次数量。
} // 结束暂停特效质量切档回归测试。

#[test] // 没有HanabiPlugin和EffectAsset资源时系统必须静默降级而非panic。
fn rain_system_is_safe_without_hanabi_plugin() {
    let mut app = App::new(); // 不安装HanabiPlugin也不创建EffectAsset资源。
    let output = WeatherOutput {
        precipitation: 1.0, // 请求满强度降雨以进入资产创建路径。
        wind: 0.9,          // 请求明显风偏以覆盖动态参数计算。
        ..default()         // 其余天气通道保持晴天默认值。
    }; // 从有效晴天输出构造强制大雨样本。
    app.insert_resource(output); // 提供降雨系统必需的天气输出。
    app.insert_resource(State::new(ScenePhase::Active)); // 动态地图玩法场景允许运行时调试天气。
    app.add_systems(Update, sync_weather_rain); // 只注册被测降雨系统。
    app.world_mut().spawn((PrimarySceneView, Transform::default())); // 生成可定位的玩法主相机。
    app.update(); // 缺少Hanabi资产容器时应安全返回。
    let emitter_count = {
        let world = app.world_mut(); // 在同一可变世界借用内构造并执行查询。
        let mut query = world.query::<&WeatherRainEmitter>(); // 查询所有可能生成的天气雨实体。
        query.iter(world).count() // 统计实际发射体数量并在块末释放世界借用。
    }; // 完成无Hanabi环境发射体计数。
    assert_eq!(emitter_count, 0); // 不创建无法渲染的半成品雨实体。
} // 结束无Hanabi插件安全测试。

#[test] // 仅提供EffectAsset容器时应创建单实例且严格使用天气专属层。
fn rain_emitter_is_single_and_uses_weather_render_layer() {
    let mut app = App::new(); // 不安装完整HanabiPlugin，避免测试初始化GPU渲染器。
    app.insert_resource(Assets::<EffectAsset>::default()); // 只提供延迟构建雨资产所需的容器，避免AssetServer依赖。
    let output = WeatherOutput {
        precipitation: 0.4, // 使用设计小雨雨量触发发射体创建。
        wind: 0.45,         // 使用设计小雨风力写入初始属性。
        ..default()         // 其余天气通道保持晴天默认值。
    }; // 从晴天结构构造小雨样本。
    app.insert_resource(output); // 提供被测连续天气输出。
    app.insert_resource(State::new(ScenePhase::Active)); // 动态地图玩法场景允许视觉天气。
    app.add_systems(Update, sync_weather_rain); // 注册唯一降雨同步系统。
    app.world_mut().spawn((
        PrimarySceneView,                       // 标记为玩法主相机。
        Transform::from_xyz(3.0, 5.0, 7.0), // 使用非零坐标验证发射体跟随偏移。
    )); // 完成测试玩法相机生成。
    app.update(); // 首帧创建GPU雨效果资产和唯一发射体实体。
    app.update(); // 第二帧验证系统复用而不会重复生成实例。
    let emitters = {
        let world = app.world_mut(); // 在同一可变世界借用中构造查询并复制断言所需轻量值。
        let mut query = world.query_filtered::<
                (&Transform, &RenderLayers), // 读取发射体定位与专属渲染层。
                With<WeatherRainEmitter>,    // 只匹配天气雨发射体。
            >(); // 构造唯一发射体查询。
        query
            .iter(world)
            .map(|(transform, layers)| (transform.translation, layers.clone())) // 复制位置与层，避免测试持有世界引用。
            .collect::<Vec<_>>() // 收集结果便于同时断言数量与字段。
    }; // 完成唯一发射体结果收集。
    assert_eq!(emitters.len(), 1); // 连续更新必须始终只保留一个发射体。
    assert_eq!(emitters[0].0, Vec3::new(3.0, 23.0, -13.0)); // 默认负Z视线让发射圆盘前移二十米并位于相机上方十八米。
    assert!(
        emitters[0]
            .1
            .intersects(&RenderLayers::layer(WEATHER_EFFECT_LAYER))
    ); // 发射体只需与天气层相交即可被玩法主相机绘制。
    assert!(!emitters[0].1.intersects(&RenderLayers::layer(0))); // 雨丝不得泄漏到默认小地图或辅助相机层。
} // 结束雨发射体单例与渲染层测试。

#[test] // F12正交测绘视图不能留下只覆盖相机附近两百米的雨盘，返回透视后应自动恢复。
fn authoring_orthographic_view_removes_rain_emitter_and_perspective_restores_it() {
    let mut app = App::new(); // 无GPU最小ECS应用足以验证唯一雨实体生命周期。
    app.insert_resource(Assets::<EffectAsset>::default());
    app.insert_resource(WeatherOutput {
        precipitation: 0.8, // 使用明显降雨确保透视阶段进入正式创建路径。
        ..default()
    });
    app.insert_resource(State::new(ScenePhase::Active)); // 使用动态地图验证作者视图与玩法雨效切换。
    app.insert_resource(WeatherAuthoringPreview::new(true));
    app.add_systems(Update, sync_weather_rain);
    let preview_camera = app
        .world_mut()
        .spawn((
            WeatherPreviewCamera,
            Transform::from_xyz(4.0, 6.0, 8.0),
            Projection::Perspective(PerspectiveProjection::default()),
        ))
        .id();

    app.update();
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherRainEmitter>>()
            .iter(app.world())
            .count(),
        1
    ); // F12透视必须显示正式GPU雨幕。

    app.world_mut()
        .entity_mut(preview_camera)
        .insert(Projection::Orthographic(
            OrthographicProjection::default_3d(),
        ));
    app.update();
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherRainEmitter>>()
            .iter(app.world())
            .count(),
        0
    ); // 正交测绘视图必须清理旧雨发射体及其GPU粒子，而不是把局部圆盘留在画面中。

    app.world_mut()
        .entity_mut(preview_camera)
        .insert(Projection::Perspective(PerspectiveProjection::default()));
    app.update();
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<WeatherRainEmitter>>()
            .iter(app.world())
            .count(),
        1
    ); // 返回透视后从缓存EffectAsset重建唯一正式雨幕。
}
