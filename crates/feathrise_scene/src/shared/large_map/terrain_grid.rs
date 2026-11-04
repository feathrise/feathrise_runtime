//! F12 当前地形真实三角网格的目标登记与普通 CPU Mesh 线框同步。
//! 固定大地图只在 WorldInstanceReady 时检查有限后代，稳定帧不扫描场景或网格数据。

use bevy::pbr::wireframe::{
    // 复用Bevy原生三角线框，不生成第二份Mesh。
    Wireframe,
    WireframeColor,
    WireframeLineWidth,
    WireframeTopology, // 每目标独立颜色、线宽和拓扑。
}; // 完成普通CPU地形线框组件导入。
use bevy::prelude::*; // Observer、Query、Commands与场景层级遍历。

use crate::{
    // 动态地图公共身份与F12公开预览契约。
    DynamicMapTerrainPrimitive,
    DynamicSparseClipmapPrimitive,
    TerrainGridPreview,
    TerrainGridTarget,
}; // 完成地形网格所需公共类型导入。

/// 统一的暖金色使普通Wireframe与Clipmap专用覆盖层保持一致。
pub(crate) fn terrain_grid_color() -> Color {
    Color::srgb(0.901_960_8, 0.721_568_64, 0.360_784_32) // 精确对应作者方案中的#E6B85C。
} // 完成共享线框颜色构造。

/// 动态GLB显示、Geometry Clipmap显示只凭唯一正式地形身份获得网格目标。
pub(crate) fn mark_dynamic_terrain_grid_target(
    add: On<Add, DynamicMapTerrainPrimitive>, // 正式显示实体生成或换代时触发一次。
    mut commands: Commands,                   // 观察器只插入公共零大小标记。
) {
    commands.entity(add.entity).insert(TerrainGridTarget); // 离屏预热与隐藏GLB输入没有正式显示身份，因而不会命中。
} // 结束动态地形目标登记观察器。

/// 为普通CPU地形安装或移除Bevy原生三角线框组件。
fn set_regular_terrain_grid(
    commands: &mut Commands, // 延迟命令与目标生成/热重载安全共存。
    entity: Entity,          // 当前显式TerrainGridTarget实体。
    visible: bool,           // 当前地图本进程独立网格选择。
) {
    if visible {
        // 开启时只增加渲染辅助组件。
        commands.entity(entity).insert((
            // 同一批提交固定四个Wireframe设置。
            Wireframe, // 启用Bevy独立线框渲染阶段。
            WireframeColor {
                color: terrain_grid_color(),
            }, // 使用统一暖金色#E6B85C。
            WireframeLineWidth { width: 1.0 }, // 一像素屏幕空间线宽避免遮住地表细节。
            WireframeTopology::Triangles, // 显示真实三角索引，不尝试猜测四边形。
        )); // 不替换原材质、Mesh或可见性。
    } else {
        // 关闭时只撤销本功能拥有的辅助组件。
        commands.entity(entity).remove::<(
            // Bevy插件随后回收其派生线框材质组件。
            Wireframe,          // 停止线框阶段绘制。
            WireframeColor,     // 清除本功能颜色覆盖。
            WireframeLineWidth, // 清除本功能线宽覆盖。
            WireframeTopology,  // 清除本功能三角拓扑覆盖。
        )>(); // 原地形全部正式组件保持不变。
    } // 完成当前目标显示状态应用。
} // 结束普通地形线框组件同步helper。

/// 开关真实变化时只遍历已标记地形；稳定帧在入口直接退出。
pub(crate) fn sync_regular_terrain_grid_visibility(
    preview: Res<TerrainGridPreview>, // F12会话发布的当前地图唯一显示状态。
    targets: Query<
        // 普通Mesh目标使用Bevy Wireframe，Clipmap由高度感知专用覆盖层处理。
        Entity, // 只需要实体身份以增删辅助组件。
        (
            With<TerrainGridTarget>,                // 显式目标边界排除所有非地形内容。
            With<Mesh3d>,                           // 没有可见Mesh时不安装空线框。
            Without<DynamicSparseClipmapPrimitive>, // GPU位移平面禁止进入默认线框路径。
        ),
    >, // 完成普通地形目标查询定义。
    mut commands: Commands,           // 批量应用当前开关。
) {
    if !preview.is_changed() {
        // 稳定帧不遍历任何目标实体。
        return; // Mesh换代由下方Changed查询和Wireframe插件共同处理。
    } // 完成资源变化快速门。
    for entity in &targets {
        // 仅在按钮、切图或加载页清零时访问有限目标。
        set_regular_terrain_grid(&mut commands, entity, preview.visible); // 同步当前地图最终选择。
    } // 完成本次开关变化应用。
} // 结束普通地形开关同步。

/// 新目标与Mesh句柄换代时恢复当前选择；无变化帧查询为空。
pub(crate) fn sync_changed_regular_terrain_grid_targets(
    preview: Res<TerrainGridPreview>, // 新实体必须继承当前地图既有选择。
    targets: Query<
        // Added标记与Changed Mesh覆盖初载、作者预览换代和GLB热更。
        Entity, // 当前变化目标实体。
        (
            With<TerrainGridTarget>,                         // 只处理公共显式地形目标。
            With<Mesh3d>,                                    // Mesh已存在才允许进入线框插件。
            Without<DynamicSparseClipmapPrimitive>,          // Clipmap保持专用GPU位移覆盖层。
            Or<(Added<TerrainGridTarget>, Changed<Mesh3d>)>, // 不在稳定帧重复写组件。
        ),
    >, // 完成增量变化查询定义。
    mut commands: Commands,           // 只修改本次新增或换代的有限实体。
) {
    for entity in &targets {
        // 无目标变化时循环为空。
        set_regular_terrain_grid(&mut commands, entity, preview.visible); // Mesh句柄变化不复制或重建资产。
    } // 完成本次新增/换代目标恢复。
} // 结束普通地形增量目标同步。

#[cfg(test)] // 无窗口回归覆盖白名单排除与Clipmap默认平面禁用。
mod tests {
    use super::*; // 直接运行生产目标同步系统与白名单函数。

    #[test] // 动态GLB与Clipmap只凭正式显示身份登记，预热Mesh和普通场景对象保持无标记。
    fn dynamic_grid_target_observer_marks_only_formal_terrain_displays() {
        let mut app = App::new(); // 最小观察器应用不安装渲染插件。
        app.add_observer(mark_dynamic_terrain_grid_target); // 使用生产Add观察器登记公共目标。
        let dynamic_glb = app
            .world_mut()
            .spawn((DynamicMapTerrainPrimitive, Mesh3d(Handle::default())))
            .id(); // 正式动态GLB地形。
        let clipmap = app
            .world_mut()
            .spawn((
                DynamicMapTerrainPrimitive,    // Clipmap同样使用唯一正式地形身份。
                DynamicSparseClipmapPrimitive, // 后续同步将它导向高度感知专用覆盖。
                Mesh3d(Handle::default()),     // 零高度组合Mesh不能使用默认Wireframe。
            ))
            .id(); // 保存正式Clipmap实体。
        let prewarm = app.world_mut().spawn(Mesh3d(Handle::default())).id(); // 离屏预热输入没有正式显示身份。
        let collision_only = app.world_mut().spawn(Transform::IDENTITY).id(); // 玩法碰撞体不携带任何可视地形身份。
        app.update(); // 应用观察器延迟插入命令。
        assert!(app.world().get::<TerrainGridTarget>(dynamic_glb).is_some()); // 正式动态GLB被精确标记。
        assert!(app.world().get::<TerrainGridTarget>(clipmap).is_some()); // 正式Clipmap被精确标记。
        assert!(app.world().get::<TerrainGridTarget>(prewarm).is_none()); // 预热Mesh不因拥有Mesh3d而误入。
        assert!(
            app.world()
                .get::<TerrainGridTarget>(collision_only)
                .is_none()
        ); // 碰撞/逻辑实体不获得渲染标记。
    } // 完成动态正式目标与排除边界回归。

    #[test] // 普通Mesh获得真实三角线框，而Clipmap零高度平面必须留给专用覆盖层。
    fn regular_grid_sync_excludes_sparse_clipmap_and_recovers_mesh_replacement() {
        let mut app = App::new(); // 最小ECS应用不安装实际渲染器。
        app.init_resource::<TerrainGridPreview>() // 默认关闭且不需要settings资源。
            .add_systems(
                // 两个生产增量系统共同覆盖开关与Mesh换代。
                Update, // 无窗口测试在普通Update执行即可。
                (
                    sync_regular_terrain_grid_visibility,      // 处理按钮变化。
                    sync_changed_regular_terrain_grid_targets, // 处理目标与Mesh变化。
                )
                    .chain(), // 保持与正式PostUpdate相同的确定顺序。
            ); // 完成测试应用注册。
        let ordinary = app
            .world_mut()
            .spawn((
                // 构造动态GLB或作者预览等普通CPU地形。
                TerrainGridTarget,         // 显式允许显示网格。
                Mesh3d(Handle::default()), // 无窗口测试只需要稳定句柄组件。
            ))
            .id(); // 保存普通地形实体。
        let clipmap = app
            .world_mut()
            .spawn((
                // 构造GPU位移Clipmap源平面。
                TerrainGridTarget,             // Clipmap同样是公共目标。
                DynamicSparseClipmapPrimitive, // 专用标记必须阻止默认Wireframe。
                Mesh3d(Handle::default()),     // CPU Mesh所有Y仍为零。
            ))
            .id(); // 保存Clipmap实体。
        app.world_mut().resource_mut::<TerrainGridPreview>().visible = true; // 模拟F12开启显示网格。
        app.update(); // 应用预览变化和延迟组件命令。
        assert!(app.world().get::<Wireframe>(ordinary).is_some()); // 普通地形使用Bevy原生线框。
        assert!(app.world().get::<Wireframe>(clipmap).is_none()); // Clipmap平面不产生错误零高度线框。
        app.world_mut().entity_mut(ordinary).remove::<Wireframe>(); // 模拟外部在Mesh换代边界清除了派生标记。
        app.world_mut()
            .entity_mut(ordinary)
            .insert(Mesh3d(Handle::default())); // 重新插入同句柄仍产生Changed tick。
        app.update(); // 增量系统应恢复当前开启状态。
        assert!(app.world().get::<Wireframe>(ordinary).is_some()); // 热重载后线框自动恢复。
        app.world_mut().resource_mut::<TerrainGridPreview>().visible = false; // 模拟F12独立关闭网格。
        app.update(); // 关闭只移除普通地形辅助组件。
        assert!(app.world().get::<Wireframe>(ordinary).is_none()); // 正式Mesh本身仍存在。
    } // 完成普通地形线框生命周期回归。
} // 结束地形网格系统测试。
