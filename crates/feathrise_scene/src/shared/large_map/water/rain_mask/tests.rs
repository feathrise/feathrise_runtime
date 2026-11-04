//! 无GPU窗口回归：真实代理生命周期、空几何与相机失效边界。
use super::super::{
    components::{DynamicWaterSurface, WaterJumpCycle},
    footprint::WaterInteractionFootprint,
    material::DynamicWaterMaterial,
};
use super::{
    camera, components::RainMaskCamera, material::RainHeightMaterial, resources::RainMaskCache,
    systems,
}; // 调用实际生产系统与缓存，不另写模拟算法。
use crate::maps::{
    ClipmapRainHeightMaterial, ClipmapWaterMaterial, ClipmapWaterSurface,
    test_clipmap_water_material, test_clipmap_water_surface,
};
use bevy::camera::visibility::NoFrustumCulling; // Clipmap共享环带由片元精确裁切，不按整环Aabb剔除。
use bevy::camera::{primitives::Aabb, visibility::RenderLayers}; // 检查真实源边界与独占捕获层。
use bevy::prelude::*; // 测试复用正式ECS、资源和资产类型。
use feathrise_atmosphere::WeatherOutput; // 以真实天气输出控制捕获资格。
use feathrise_scene_core::view::PrimarySceneView; // 只给正式玩法锚点添加标记。
use feathrise_environment::rain_water::RainWaterMask; // 读取消费者可见的共享有效性。
use feathrise_scene_core::state::ScenePhase; // 覆盖正式场景切换的失效边界。 // 调用实际生产系统与缓存，不另写模拟算法。 // 复用生产Clipmap材质和不可变R8足迹夹具。

#[test] // 实际捕获片元的有色/无色网格入口均必须通过Naga。
fn height_fragment_is_valid_with_and_without_vertex_colors() {
    // 与既有水Shader测试共用wgpu导出的同版本Naga，无新增依赖。
    let production = include_str!("height.wgsl"); // 直接验证生产文件，不能用测试侧高度算法替代。
    for vertex_colors in [false, true] {
        // 覆盖原水网格有无顶点色两种真实管线布局。
        let color_field = if vertex_colors {
            "@location(5) color: vec4<f32>,"
        } else {
            ""
        }; // 无顶点色输入必须真正移除字段，避免掩盖条件分支接线错误。
        let mut source = format!(
            "struct VertexOutput {{ @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, {color_field} }}\n"
        ); // 只以最小同类型IO桩替代Bevy导入，不替代生产片元正文。
        let mut active = true; // 本片元只有一层顶点色条件，未来新增指令应明确更新测试。
        for line in production.lines() {
            // 固定小型Shader在测试中一次展开。
            let directive = line.trim(); // 保留生产表达式内部原样。
            if directive.starts_with("#import ") {
                continue;
            } // 唯一VertexOutput导入已由匹配IO声明提供。
            if directive == "#ifdef VERTEX_COLORS" {
                active = vertex_colors;
                continue;
            } // 使用实际管线顶点色定义控制生产分支。
            if directive == "#endif" {
                active = true;
                continue;
            } // 恢复条件外完整高度编码正文。
            assert!(
                !directive.starts_with('#'),
                "新增Shader预处理指令需纳入真实变体测试：{directive}"
            ); // 防止将未知生产分支静默丢弃。
            if active {
                source.push_str(&line.replace("#{MATERIAL_BIND_GROUP}", "2"));
                source.push('\n');
            } // 仅替换Bevy绑定组插值，保留105号覆盖uniform和所有计算。
        } // 完成实际生产片元变体展开。
        let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| {
            panic!(
                "高度片元VERTEX_COLORS={vertex_colors}语法错误：{}",
                error.emit_to_string(&source)
            )
        }); // 保留完整源码行列定位。
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("高度片元VERTEX_COLORS={vertex_colors}语义错误：{error}")); // 实际检查片元入口、discard、uniform和输出类型。
    } // 完成两个真实顶点色变体。
} // 结束无GPU生产高度片元回归。

fn water_vertex_for_capture(vertex_colors: bool) -> String {
    // 展开真实水顶点入口，仅替换Bevy空间变换函数。
    let mut result = String::new(); // 测试期小型源码拼接不进入帧关键路径。
    let mut importing = false; // 正式顶点文件使用多行导入。
    let mut active = true; // 现有顶点入口的属性条件均不嵌套。
    for line in include_str!("../shaders/vertex.wgsl").lines() {
        // 正文始终取正式入口，避免测试另写波形调用。
        let trimmed = line.trim(); // 只解释预处理指令。
        if importing {
            importing = !trimmed.ends_with('}');
            continue;
        } // 多行导入由生产库和最小Bevy桩提供。
        if trimmed.starts_with("#import ") {
            importing = trimmed.ends_with('{');
            continue;
        } // 单行导入同时跳过。
        if let Some(symbol) = trimmed.strip_prefix("#ifdef ") {
            active = symbol == "VERTEX_COLORS" && vertex_colors;
            continue;
        } // 本回归隔离有无顶点色，其余可选属性关闭。
        if trimmed == "#endif" {
            active = true;
            continue;
        } // 条件结束恢复生产正文。
        assert!(
            !trimmed.starts_with('#'),
            "新顶点预处理指令需更新捕获测试：{trimmed}"
        ); // 未知指令不能静默遗漏。
        if active {
            result.push_str(&line.replace("mesh_functions::", ""));
            result.push('\n');
        } // 只把Bevy命名空间映射到同签名空间变换桩。
    } // 完成真实顶点属性变体展开。
    result // 返回仍调用实际sample_dynamic_water的完整顶点入口。
} // 结束顶点预处理辅助。

#[test] // 捕获必须复用100号真实波形uniform且不要求101至104微法线纹理。
fn capture_vertex_uses_only_wave_uniform_material_binding() {
    // 验证实际水库与实际顶点入口的传递资源使用，不靠字符串猜测。
    let water = include_str!("../shaders/water.wgsl")
        .lines()
        .filter(|line| !line.starts_with("#define_import_path"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("#{MATERIAL_BIND_GROUP}", "2"); // 保留完整水库，包括真实但顶点不使用的纹理声明和函数。
    let stub = r#" // 仅模拟Bevy空间坐标接口，不模拟任何水波算法。
struct Vertex { @builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location(2) uv: vec2<f32>, @location(5) color: vec4<f32>, } // 生产顶点实际读取的输入接口。
struct Globals { time: f32, } // 时间字段保持正式类型。
@group(0) @binding(0) var<uniform> globals: Globals; // 全局时钟属于view组，不属于捕获材质。
fn get_world_from_local(instance: u32) -> mat4x4<f32> { return mat4x4<f32>(); } // 测试只校验类型与资源依赖。
fn mesh_position_local_to_world(world: mat4x4<f32>, position: vec4<f32>) -> vec4<f32> { return world * position; } // 保留空间变换签名。
fn mesh_normal_local_to_world(normal: vec3<f32>, instance: u32) -> vec3<f32> { return normal; } // 不替换实际水波解析法线。
fn position_world_to_clip(position: vec3<f32>) -> vec4<f32> { return vec4<f32>(position, 1.0); } // 与正式clip转换同一输入输出类型。
"#; // 无GPU、无额外测试依赖。
    for vertex_colors in [false, true] {
        // 与高度片元相同的两个关键属性变体。
        let source = format!(
            "{stub}\n{water}\n{}",
            water_vertex_for_capture(vertex_colors)
        ); // 完整真实库与入口组合解析，未删除101至104资源。
        let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| {
            panic!("捕获水顶点语法错误：{}", error.emit_to_string(&source))
        }); // 保留生产源码错误位置。
        let info = wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("捕获水顶点语义错误：{error}")); // Naga分析包含传递函数调用的全局使用集合。
        let entry_index = module
            .entry_points
            .iter()
            .position(|entry| entry.name == "vertex")
            .unwrap(); // 定位实际生产顶点入口。
        let entry = info.get_entry_point(entry_index); // 获取该入口的传递资源使用信息。
        let mut bindings = Vec::new(); // 只收集入口真正使用的材质资源。
        for (handle, variable) in module.global_variables.iter() {
            // 检查Naga真实绑定反射，不仅读声明是否存在。
            if let Some(binding) = &variable.binding {
                if binding.group == 2 && !entry[handle].is_empty() {
                    bindings.push(binding.binding);
                }
            } // view资源独立提供，未使用纹理不进入管线所需绑定。
        } // 完成材质绑定反射。
        assert_eq!(
            bindings,
            vec![100],
            "顶点色={vertex_colors}捕获顶点应仅需要640字节波形uniform"
        ); // 证明101至104无需加入RainHeightMaterial。
    } // 完成有色与无色真实顶点资源校验。
} // 结束捕获材质最小绑定契约回归。

fn app() -> App {
    // 不注册Assets<Mesh>，证明代理生成不依赖CPU几何访问。
    let mut app = App::new(); // 使用无渲染器最小App测试系统行为。
    app.init_resource::<Assets<DynamicWaterMaterial>>() // 源材质使用正常资产身份。
        .init_resource::<Assets<RainHeightMaterial>>() // 捕获资产可独立检查引用释放。
        .init_resource::<RainMaskCache>() // 空缓存代表尚未接管任何水源。
        .add_message::<AssetEvent<DynamicWaterMaterial>>() // 提供生产系统使用的材质变更事件。
        .add_systems(Update, systems::sync_proxies); // 直接执行真实代理维护并在帧尾应用命令。
    app // 返回隔离的无GPU测试环境。
} // 结束当前测试或辅助构造。

fn source(app: &mut App, material: Handle<DynamicWaterMaterial>, bounded: bool) -> Entity {
    // 构造正常水源身份，边界由调用者决定是否已完成。
    let mut entity = app.world_mut().spawn((
        DynamicWaterSurface {
            // 源拥有真实水体组件，进入正式变化查询。
            interaction_footprint: WaterInteractionFootprint::Disabled, // 禁用玩家CPU互动，验证纯视觉水也可被捕获。
            rest_water_level: 0.0,
            flow_direction: Vec2::X,
            flow_speed: 1.0, // 固定基准水位与流向，避免引入无关波形状态。
            player_ripples: Vec::new(),
            last_player_takeoff_sequence: None, // 没有玩家历史时仍需建立捕获。
            previous_player_local_xz: None,
            previous_player_local_y: None, // 初次接管没有前帧玩家位置。
            distance_since_ripple: 0.0,
            player_was_in_water: false,
            jump_cycle: WaterJumpCycle::Idle, // 初始不触发玩家入水或跳跃事件。
        },
        Mesh3d::default(),
        MeshMaterial3d(material),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::Hidden,
    )); // 仅给共享Mesh句柄与完整变换；隐藏源用于核验子代理继承显隐。
    if bounded {
        entity.insert(Aabb::from_min_max(Vec3::splat(-1.0), Vec3::ONE));
    } // 只为已具备真实边界的源声明捕获范围。
    entity.id() // 返回原源身份供定向变更和清理。
} // 结束当前测试或辅助构造。

#[test]
fn water_render_chunks_are_captured_without_duplicate_cpu_interaction() {
    let mut app = app();
    let material = app.world_mut().resource_mut::<Assets<DynamicWaterMaterial>>().add(DynamicWaterMaterial::default());
    let root = source(&mut app, material.clone(), true);
    let child = source(&mut app, material, true);
    app.world_mut().entity_mut(child).remove::<DynamicWaterSurface>()
        .insert((super::super::components::WaterRenderChunk, ChildOf(root)));
    app.update();
    assert_eq!(app.world().resource::<RainMaskCache>().proxies.len(), 2);
    assert!(app.world().get::<DynamicWaterSurface>(child).is_none());
    app.world_mut().entity_mut(child).remove::<super::super::components::WaterRenderChunk>();
    app.update();
    assert_eq!(app.world().resource::<RainMaskCache>().proxies.len(), 1);
}

#[test]
fn clipmap_volume_water_chunks_all_reach_rain_capture_beyond_one_frame() {
    let mut app = App::new();
    app.init_resource::<Assets<ClipmapWaterMaterial>>()
        .init_resource::<Assets<ClipmapRainHeightMaterial>>()
        .init_resource::<RainMaskCache>()
        .add_message::<AssetEvent<ClipmapWaterMaterial>>()
        .add_systems(Update, systems::sync_clipmap_proxies);
    let material = app.world_mut().resource_mut::<Assets<ClipmapWaterMaterial>>()
        .add(test_clipmap_water_material());
    for _ in 0..12 {
        app.world_mut().spawn((super::super::components::WaterRenderChunk,
            Mesh3d::default(), MeshMaterial3d(material.clone()), Transform::IDENTITY,
            GlobalTransform::default(), Visibility::Inherited));
    }
    for _ in 0..3 { app.update(); }
    assert_eq!(app.world().resource::<RainMaskCache>().clipmap_proxies.len(), 12);
    assert_eq!(app.world().resource::<Assets<ClipmapRainHeightMaterial>>().len(), 12); // Clipmap捕获按代理独立拥有覆盖Uniform和材质。
    assert_eq!(app.world_mut().query::<&ClipmapWaterSurface>().iter(app.world()).count(), 0);
}

#[test] // 无窗口回归覆盖生产端生命周期契约。
fn proxies_share_material_and_release_on_replace_and_remove() {
    // 共享引用在材质替换和源消失后必须精确释放。
    let mut app = app(); // 每个场景使用独立资源和事件队列。
    let first = app
        .world_mut()
        .resource_mut::<Assets<DynamicWaterMaterial>>()
        .add(DynamicWaterMaterial::default()); // 创建实际源材质资产供代理引用。
    let second = app
        .world_mut()
        .resource_mut::<Assets<DynamicWaterMaterial>>()
        .add(DynamicWaterMaterial::default()); // 不同资产身份用于验证替换后的独立缓存。
    let a = source(&mut app, first.clone(), true); // 第一个完整源使用共享材质。
    let b = source(&mut app, first.clone(), true); // 第二个源验证同材质只创建一份捕获资产。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    let cache = app.world().resource::<RainMaskCache>(); // 从生产缓存读取维护结果。
    assert_eq!(cache.proxies.len(), 2); // 源与代理一一对应，禁止重复创建。
    assert_eq!(cache.materials.len(), 1); // 共享源材质只保留一个捕获材质。
    assert_eq!(cache.materials[&first.id()].users, 2); // 两个独立源共同持有两份引用。
    let proxy = cache.proxies[&a].entity; // 保存首次代理身份用于稳定帧比较。
    assert_eq!(app.world().get::<ChildOf>(proxy).unwrap().parent(), a); // 捕获代理必须挂在原水源下继承层级变化。
    assert_eq!(
        *app.world().get::<Visibility>(proxy).unwrap(),
        Visibility::Inherited
    ); // 隐藏或移动父层级时沿原层级传播。
    assert_eq!(
        *app.world().get::<RenderLayers>(proxy).unwrap(),
        RenderLayers::layer(feathrise_scene_core::constants::render_layers::RAIN_WATER_MASK_LAYER)
    ); // 独占捕获层，避免进入主相机和小地图。
    assert_eq!(
        app.world().get::<Mesh3d>(proxy).unwrap().0,
        app.world().get::<Mesh3d>(a).unwrap().0
    ); // 必须直接复用相同Mesh资产，禁止复制CPU几何。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert_eq!(
        app.world().resource::<RainMaskCache>().proxies[&a].entity,
        proxy
    ); // 稳定帧不重建代理实体。
    app.world_mut().entity_mut(a).insert(MeshMaterial3d(second)); // 真实替换源材质，触发旧引用回收。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert_eq!(app.world().resource::<RainMaskCache>().materials.len(), 2); // 替换后两个不同源材质各自持有捕获资产。
    assert_eq!(
        app.world().resource::<RainMaskCache>().materials[&first.id()].users,
        1
    ); // 原共享材质仅余第二个源的一份引用。
    app.world_mut().entity_mut(a).remove::<Mesh3d>(); // 撤销源网格后不允许继续绘制旧代理。
    app.world_mut().entity_mut(b).despawn(); // 父源销毁覆盖层级同步回收路径。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(app.world().resource::<RainMaskCache>().proxies.is_empty()); // 全部源撤销后不能残留捕获实体记录。
    assert!(app.world().resource::<RainMaskCache>().materials.is_empty()); // 最后引用归还后清空捕获材质缓存。
    assert_eq!(
        app.world().resource::<Assets<RainHeightMaterial>>().len(),
        0
    ); // 无外部持有者时实际资产也必须释放。
} // 结束当前测试或辅助构造。

#[test]
fn clipmap_proxy_reuses_ring_and_releases_layer_material() {
    let mut app = App::new();
    app.init_resource::<Assets<ClipmapWaterMaterial>>()
        .init_resource::<Assets<ClipmapRainHeightMaterial>>()
        .init_resource::<RainMaskCache>()
        .add_message::<AssetEvent<ClipmapWaterMaterial>>()
        .add_systems(Update, systems::sync_clipmap_proxies);
    let material = app
        .world_mut()
        .resource_mut::<Assets<ClipmapWaterMaterial>>()
        .add(test_clipmap_water_material());
    let source = app
        .world_mut()
        .spawn((
            test_clipmap_water_surface(),
            Mesh3d::default(),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Hidden,
        ))
        .id();
    app.update();
    let cache = app.world().resource::<RainMaskCache>();
    assert_eq!(cache.clipmap_proxies.len(), 1);
    assert!(cache.clipmap_queued.is_empty());
    let proxy = cache.clipmap_proxies[&source].entity;
    let capture = cache.clipmap_proxies[&source].material.clone();
    assert_eq!(app.world().get::<ChildOf>(proxy).unwrap().parent(), source);
    assert_eq!(
        app.world().get::<Mesh3d>(proxy).unwrap().0,
        app.world().get::<Mesh3d>(source).unwrap().0,
    );
    assert!(app.world().get::<NoFrustumCulling>(proxy).is_some());
    assert_eq!(
        *app.world().get::<RenderLayers>(proxy).unwrap(),
        RenderLayers::layer(feathrise_scene_core::constants::render_layers::RAIN_WATER_MASK_LAYER),
    );
    assert!(
        app.world()
            .resource::<Assets<ClipmapRainHeightMaterial>>()
            .get(&capture)
            .is_some()
    );
    app.update();
    assert_eq!(
        app.world().resource::<RainMaskCache>().clipmap_proxies[&source].entity,
        proxy,
    ); // 视角稳定帧不重建代理或R8材质。
    app.world_mut()
        .entity_mut(source)
        .remove::<ClipmapWaterSurface>();
    app.update();
    assert!(
        app.world()
            .resource::<RainMaskCache>()
            .clipmap_proxies
            .is_empty()
    );
    assert_eq!(
        app.world()
            .resource::<Assets<ClipmapRainHeightMaterial>>()
            .len(),
        0,
    );
}

#[test] // 无窗口回归覆盖生产端生命周期契约。
fn absent_bounds_do_not_block_complete_water_and_late_bounds_wake_source() {
    // 合法空网格不能阻塞其他水体的完整捕获。
    let mut app = app(); // 每个场景使用独立资源和事件队列。
    let material = app
        .world_mut()
        .resource_mut::<Assets<DynamicWaterMaterial>>()
        .add(DynamicWaterMaterial::default()); // 创建实际源材质资产供代理引用。
    let empty = source(&mut app, material.clone(), false); // 缺Aabb的合法空水体不制造猜测几何。
    source(&mut app, material, true); // 同时保留一个完整水源验证独立接管。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert_eq!(app.world().resource::<RainMaskCache>().proxies.len(), 1); // 当前仅完整边界水源进入捕获。
    assert!(app.world().resource::<RainMaskCache>().queued.is_empty()); // 空网格不得一直pending阻塞所有有效水体。
    app.world_mut()
        .entity_mut(empty)
        .insert(Aabb::from_min_max(Vec3::ZERO, Vec3::ONE)); // 真实边界迟到时由Changed<Aabb>唤醒。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert_eq!(app.world().resource::<RainMaskCache>().proxies.len(), 2); // 第二个水源完整后加入同一捕获集合。
    app.world_mut().entity_mut(empty).remove::<Aabb>(); // 边界撤销必须同步撤销对应旧代理。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert_eq!(app.world().resource::<RainMaskCache>().proxies.len(), 1); // 当前仅完整边界水源进入捕获。
    assert!(app.world().resource::<RainMaskCache>().queued.is_empty()); // 空网格不得一直pending阻塞所有有效水体。
} // 结束当前测试或辅助构造。

#[test] // 无窗口回归覆盖生产端生命周期契约。
fn capture_requires_water_rain_unique_anchor_and_invalidates_map_switch() {
    // 捕获资格必须与天气、地图和唯一锚点同帧对应。
    let mut app = app(); // 每个场景使用独立资源和事件队列。
    app.init_resource::<RainWaterMask>() // 建立消费者共享的初始无效遮罩。
        .insert_resource(WeatherOutput {
            precipitation: 1.0,
            ..default()
        }) // 降雨达到阈值以测试其他资格守卫。
        .insert_resource(State::new(ScenePhase::Active)) // 从真实可捕获玩法场景开始。
        .insert_resource(feathrise_map_content::ActiveDynamicMap {
            map_id: Some(feathrise_map_content::MapId::parse("MAP-rain-first").unwrap()),
        }) // 共用场景枚举必须配合真实地图身份。
        .add_systems(Update, camera::update.after(systems::sync_proxies)); // 与正式顺序一致，代理维护完成后发布捕获资格。
    app.world_mut().spawn((
        RainMaskCamera,
        Camera::default(),
        Transform::default(),
        GlobalTransform::default(),
    )); // 独立捕获相机没有玩法、天空或植被标记。
    app.world_mut().spawn((
        PrimarySceneView,
        Camera::default(),
        GlobalTransform::from_translation(Vec3::new(10.25, 30.0, 20.75)),
        Projection::Perspective(default()),
    )); // 唯一透视玩法锚点使用非整数位置验证网格对齐。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(!app.world().resource::<RainWaterMask>().active); // 没水时不运行空捕获。
    let material = app
        .world_mut()
        .resource_mut::<Assets<DynamicWaterMaterial>>()
        .add(DynamicWaterMaterial::default()); // 创建实际源材质资产供代理引用。
    source(&mut app, material, true); // 同时保留一个完整水源验证独立接管。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(app.world().resource::<RainWaterMask>().active); // 核验生产端发布给雨丝的本帧有效性。
    assert_eq!(
        app.world().resource::<RainWaterMask>().bounds,
        Vec4::new(-246.0, -236.0, 512.0, 512.0)
    ); // 512米方形范围按世界XZ一米格向下对齐。
    app.world_mut().resource_mut::<feathrise_map_content::ActiveDynamicMap>()
        .select(feathrise_map_content::MapId::parse("MAP-rain-second").unwrap()); // 同一场景枚举内切图也必须撤销上一捕获。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(!app.world().resource::<RainWaterMask>().active); // 切图首帧拒绝旧图。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(app.world().resource::<RainWaterMask>().active); // 核验生产端发布给雨丝的本帧有效性。
    app.world_mut()
        .resource_mut::<WeatherOutput>()
        .precipitation = 0.0; // 雨停必须立即撤销旧高度图采样。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(!app.world().resource::<RainWaterMask>().active); // 核验生产端发布给雨丝的本帧有效性。
    app.world_mut()
        .resource_mut::<WeatherOutput>()
        .precipitation = 1.0; // 恢复降雨后继续核验相机唯一性。
    let duplicate = app
        .world_mut()
        .spawn((
            PrimarySceneView,
            Camera::default(),
            GlobalTransform::default(),
            Projection::Perspective(default()),
        ))
        .id(); // 第二个合法锚点应使捕获失败关闭。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(!app.world().resource::<RainWaterMask>().active); // 重复锚点不随机选取。
    app.world_mut().entity_mut(duplicate).despawn(); // 移除重复锚点后允许下一帧恢复。
    app.update(); // 推进一次真实ECS帧并应用延迟命令。
    assert!(app.world().resource::<RainWaterMask>().active); // 核验生产端发布给雨丝的本帧有效性。
} // 结束当前测试或辅助构造。
