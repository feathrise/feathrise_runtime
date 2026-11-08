//! 世界 UI retained phase 归还测试；验证正式队列缓存恢复，不需要窗口或 GPU。
use super::{queue::restore_retained_items, types::WorldUi3d}; // 直接运行生产使用的队列归还系统。
use bevy::{
    core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d},
    prelude::*,
    render::{
        render_phase::{DrawFunctionId, PhaseItemExtraIndex, ViewSortedRenderPhases},
        render_resource::CachedRenderPipelineId,
        sync_world::MainEntity,
        view::RetainedViewEntity,
    },
}; // 构造只含稳定身份的 phase 项。

fn item(entity: Entity) -> WorldUi3d {
    // 模拟上一帧完成独立 batching 的一个真实 retained 项。
    WorldUi3d {
        // 原 DrawFunction 与独立 phase 的函数编号故意不同。
        item: Transparent3d {
            // 不创建任何 GPU 资源。
            sorting_info: TransparentSortingInfo3d::AlwaysOnTop, // 排序与本测试的生命周期行为无关。
            distance: 0.0,                                       // 有限排序占位。
            pipeline: CachedRenderPipelineId::INVALID, // 归还阶段不能访问或等待 GPU 管线。
            entity: (Entity::PLACEHOLDER, MainEntity::from(entity)), // 与 Bevy Material queue 使用的键完全一致。
            draw_function: DrawFunctionId(9), // 模拟独立 WorldUi3d 绘制函数。
            batch_range: 100..103,            // 模拟上一帧独立 phase 的旧实例范围，归还时必须清除。
            extra_index: PhaseItemExtraIndex::None, // 测试不依赖具体间接索引存储。
            indexed: true,                    // 保留实际 UI 四边形的索引网格模式。
        }, // 原透明项数据结束。
        original_draw_function: DrawFunctionId(7), // 普通 Transparent3d 的真实绘制函数应被恢复。
        original_pipeline: CachedRenderPipelineId::INVALID, // 测试不创建GPU管线。
    } // 世界 UI 测试项结束。
} // item 结束。

#[test] // 切换回普通模式时必须归还 UI，同时不能在实体删除后再次复活旧项。
fn restored_items_keep_queue_identity_and_do_not_reappear_after_removal() {
    // 验证生产系统与 retained cache 的完整交接。
    let mut app = App::new(); // 纯 ECS，不初始化 renderer。
    app.init_resource::<ViewSortedRenderPhases<Transparent3d>>()
        .init_resource::<ViewSortedRenderPhases<WorldUi3d>>()
        .add_systems(Update, restore_retained_items); // 使用正式队列归还系统。
    let camera = app.world_mut().spawn_empty().id(); // 构造稳定主视图身份。
    let entity = app.world_mut().spawn_empty().id(); // 模拟可见世界文字实体。
    let view = RetainedViewEntity::new(MainEntity::from(camera), None, 0); // 普通主视图，无子视图。
    app.world_mut()
        .resource_mut::<ViewSortedRenderPhases<Transparent3d>>()
        .prepare_for_new_frame(view); // 原主 phase 仍存在。
    app.world_mut()
        .resource_mut::<ViewSortedRenderPhases<WorldUi3d>>()
        .entry(view)
        .or_default()
        .add_retained(item(entity)); // 上帧 world phase 持有该项。
    app.update(); // 本帧 queue 之前先归还旧 UI。
    let original = app
        .world()
        .resource::<ViewSortedRenderPhases<Transparent3d>>(); // 检查正常渲染已经取回同一项。
    let restored = &original[&view].items[&(Entity::PLACEHOLDER, MainEntity::from(entity))]; // 键不可被改成真实 render entity 或其他相机。
    assert_eq!(restored.draw_function, DrawFunctionId(7)); // 还原普通 phase 的函数编号。
    assert_eq!(restored.batch_range, 0..1); // 不能复用另一个 phase 上帧的实例范围。
    assert!(
        app.world().resource::<ViewSortedRenderPhases<WorldUi3d>>()[&view]
            .items
            .is_empty()
    ); // 原项只有一个所有者。
    app.world_mut()
        .resource_mut::<ViewSortedRenderPhases<Transparent3d>>()
        .get_mut(&view)
        .unwrap()
        .remove(Entity::PLACEHOLDER, MainEntity::from(entity)); // 模拟后续 queue 检测到实体删除。
    app.update(); // 再次归还不能从空的 overlay 复活旧对象。
    assert!(
        app.world()
            .resource::<ViewSortedRenderPhases<Transparent3d>>()[&view]
            .items
            .is_empty()
    ); // 销毁与隐藏语义保持原样。
} // retained 项生命周期测试结束。

#[test] // 地图切换销毁原相机时，不得用上一帧 UI 重新创建空壳视图。
fn destroyed_main_view_drops_its_retained_ui_phase() {
    // 验证旧视图来源身份失效的清理路径。
    let mut app = App::new(); // 同样只使用 ECS。
    app.init_resource::<ViewSortedRenderPhases<Transparent3d>>()
        .init_resource::<ViewSortedRenderPhases<WorldUi3d>>()
        .add_systems(Update, restore_retained_items); // 正式归还系统保持一致。
    let entity = app.world_mut().spawn_empty().id(); // 已失效视图曾拥有的 UI。
    let view = RetainedViewEntity::new(MainEntity::from(entity), None, 0); // 原 phase 刻意不存在。
    app.world_mut()
        .resource_mut::<ViewSortedRenderPhases<WorldUi3d>>()
        .entry(view)
        .or_default()
        .add_retained(item(entity)); // 残留上一帧独立 UI 缓存。
    app.update(); // 归还系统必须发现原视图已消失。
    assert!(
        app.world()
            .resource::<ViewSortedRenderPhases<Transparent3d>>()
            .is_empty()
    ); // 不创建已销毁视图。
    assert!(
        app.world()
            .resource::<ViewSortedRenderPhases<WorldUi3d>>()
            .is_empty()
    ); // 同时释放独立 phase 缓存。
} // 已销毁视图测试结束。

#[test] // 对照引擎真实投影，防止把抖动补偿方向写反或遗漏分辨率缩放。
fn output_depth_coordinates_match_jittered_scene_projection() {
    // 覆盖超性能、质量、DLAA和非整数窗口尺寸。
    use bevy::render::camera::TemporalJitter; // 使用当前引擎实现作为独立参照。
    let output = Vec2::new(1905.0, 1080.0); // 非整除窗口能暴露错误的固定档位倍率。
    let projection = bevy::math::proj::perspective_infinite_reverse(0.8, output.x / output.y, 0.1); // 与玩法相机同类反向Z透视。
    for internal in [Vec2::new(635.0, 360.0), Vec2::new(1270.0, 720.0), output] {
        // 真实内部像素尺寸。
        for offset in [Vec2::ZERO, Vec2::new(0.375, -0.25), Vec2::new(-0.5, 0.5)] {
            // 覆盖两轴正负抖动。
            let mut jittered = projection; // 参照真实场景投影。
            TemporalJitter { offset }.jitter_projection(&mut jittered, internal); // 引擎按内部viewport注入抖动。
            for point in [Vec3::new(1.0, 0.5, -5.0), Vec3::new(-2.0, -1.0, -30.0)] {
                // 不同屏幕位置与深度。
                let screen = |matrix: Mat4, size: Vec2| {
                    let ndc = matrix.project_point3(point);
                    (ndc.truncate() * Vec2::new(0.5, -0.5) + Vec2::splat(0.5)) * size
                }; // NDC转左上原点屏幕像素。
                let mapped = screen(projection, output) * internal / output - offset; // 生产shader的输出到源像素映射。
                assert!(mapped.abs_diff_eq(screen(jittered, internal), 0.0002)); // 必须落到场景真实光栅位置。
            } // 场景点结束。
        } // 抖动样本结束。
    } // 档位结束。
} // 投影回归结束。

#[test] // cargo check不会编译WGSL，因此单独验证真实片元代码。
fn depth_mapping_shader_validates() {
    // 仅替换Bevy import，保留生产shader完整计算逻辑。
    let source = include_str!("depth.wesl")
        .lines()
        .filter(|line| !line.starts_with("import "))
        .collect::<Vec<_>>()
        .join("\n"); // 测试夹具允许同步处理固定短文本。
    let source = format!(
        "struct FullscreenVertexOutput {{ @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, }}\n{source}"
    ); // 对应Bevy全屏顶点输出接口。
    let module = wgpu::naga::front::wgsl::parse_str(&source).expect("深度映射WGSL必须可解析"); // 验证真实shader语法。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("深度映射WGSL必须通过类型与绑定验证"); // 包含深度纹理及frag_depth合法性。
} // WGSL验证结束。

#[test] // 系统签名可编译不等于运行期借用合法，验证真实系统初始化。
fn depth_and_color_system_accesses_are_compatible() {
    // 阻止ResMut与DrawMaterial只读World混进同一系统。
    let mut world = World::new(); // 初始化系统访问元数据无需实际GPU设备。
    IntoSystem::into_system(super::depth::draw).initialize(&mut world); // 深度映射允许修改自己的缓存资源。
    IntoSystem::into_system(super::draw::draw_world_ui).initialize(&mut world); // 颜色绘制只读World不与资源写入冲突。
    IntoSystem::into_system(super::depth::configure).initialize(&mut world); // RenderWorld相机用途配置无借用冲突。
} // 调度访问回归结束。
