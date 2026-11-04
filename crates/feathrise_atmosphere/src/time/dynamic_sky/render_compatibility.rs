//! Bevy 0.19.1 大气提取兼容层：相机移除大气设置时，同帧撤销残留的 RenderWorld 门控。

use bevy::{
    // 集中导入主世界相机资格、公开大气组件与RenderWorld提取接口。
    camera::Camera3d,      // 只检查可能承载原生大气的三维相机。
    ecs::query::{Has, Or}, // 同一有界查询区分大气资格，并只锁定确有残留状态的渲染实体。
    pbr::{
        // 复用Bevy 0.19.1公开的大气提取类型，不复制私有渲染实现。
        AtmosphereSettings,    // 主世界相机当前是否仍有资格提交大气天空。
        ExtractedAtmosphere,   // 移除后必须撤销的RenderWorld大气身份门控。
        GpuAtmosphereSettings, // 移除后必须撤销的GPU设置源组件。
        extract_atmosphere,    // 清理系统显式排在Bevy原生提取之后，覆盖同一帧最终状态。
    }, // 结束Bevy大气公开类型导入。
    prelude::{App, Commands, IntoScheduleConfigs, Query, With}, // 插件注册、延迟命令与只读相机查询。
    render::{
        // 兼容层只运行在提取阶段，不进入逐视图渲染节点。
        Extract,
        ExtractSchedule,
        RenderApp, // 从主世界读取最终相机状态并修改RenderWorld。
        extract_component::DynamicUniformIndex, // 显式移除旧uniform索引以阻断残留大气节点。
        sync_world::RenderEntity, // 把主世界相机稳定映射到对应RenderWorld实体。
    }, // 结束RenderWorld接口导入。
}; // 所有依赖均来自项目已经锁定的Bevy 0.19.1。

/// 安装Bevy 0.19.1大气移除兼容层；无RenderApp的纯逻辑测试宿主安全跳过。
pub(in crate::time) fn install_atmosphere_render_state_cleanup(app: &mut App) {
    // 由世界时间插件在渲染启动前调用一次。
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        // 纯ECS宿主没有RenderWorld，也不会提交不匹配的GPU管线。
        return; // 保持现有无窗口单元测试无需安装完整渲染器。
    }; // 正式宿主从这里开始注册提取阶段兼容系统。
    render_app.add_systems(
        // 不新增Render节点，只在既有提取阶段追加一次有界相机清理。
        ExtractSchedule, // 相机HDR目标与大气资格都以本帧主世界最终值为准。
        cleanup_removed_atmosphere_render_state.after(extract_atmosphere), // Bevy原生提取结束后补齐0.19.1遗漏的移除分支。
    ); // 稳定动态天空帧只做相机数量级查询且不产生组件命令。
} // 安装函数结束。

/// 清除已经失去`AtmosphereSettings`相机的公开大气状态与uniform门控。
fn cleanup_removed_atmosphere_render_state(
    // 单独系统便于锁定提取顺序和做最小ECS回归。
    mut commands: Commands, // 所有RenderWorld删除在提取命令边界统一应用。
    cameras: Extract<Query<(RenderEntity, Has<AtmosphereSettings>), With<Camera3d>>>, // 当前全部三维相机通常只有玩法、F12及少量预览视图。
    stale_render_states: Query<
        // RenderWorld查询让稳定帧只读，不重复排入空删除命令。
        (), // 只关心是否至少还有一个公开大气门控，无需借用组件数据。
        Or<(
            // 任一旧门控存在都代表0.19.1原生提取没有完整撤销相机状态。
            With<ExtractedAtmosphere>, // 大气纹理与变换准备仍可能枚举该相机。
            With<GpuAtmosphereSettings>, // uniform准备仍可能为该相机生成索引。
            With<DynamicUniformIndex<GpuAtmosphereSettings>>, // 渲染节点仍可能拿到旧管线入口。
        )>, // 结束公开残留门控的并集过滤。
    >, // 查询规模只覆盖拥有残留大气状态的RenderWorld实体。
) {
    // 开始按主世界最终资格清理旧RenderWorld状态。
    for (render_entity, has_atmosphere_settings) in &cameras {
        // 工作量仅随活跃三维相机数增长，不扫描纹理或场景实体。
        if has_atmosphere_settings || !stale_render_states.contains(render_entity) {
            // 仍启用动态大气或已经清理干净的相机都不产生RenderWorld写入。
            continue; // 禁止误删有效状态，也避免稳定天空盒帧反复排入空命令。
        } // 只有主世界已移除设置且RenderWorld确有残留的相机进入兼容清理。
        commands.entity(render_entity).try_remove::<(
            // 公开组件缺失时保持幂等，不制造稳定帧写入。
            ExtractedAtmosphere, // 阻止后续大气纹理、变换和绑定组准备继续处理该相机。
            GpuAtmosphereSettings, // 撤销可能由旧版原生提取器遗留的GPU设置源。
            DynamicUniformIndex<GpuAtmosphereSettings>, // 直接打断render_sky节点查询，旧HDR管线不能落入LDR目标。
        )>(); // 私有缓存组件可在下次启用时被原生系统覆盖，但本帧已失去全部公开执行门控。
    } // 完成当前有限三维相机集合清理。
} // 提取兼容系统结束。

#[cfg(test)] // 仅测试构造独立MainWorld与RenderWorld，不进入正式二进制。
mod tests {
    // 回归Bevy 0.19.1遗漏的“移除设置但全局大气仍存在”分支。
    use bevy::{
        // 测试只使用轻量ECS、公开大气值与主/渲染世界映射。
        asset::AssetId, // 构造不需要真实GPU资产的失效散射介质身份。
        math::Mat4,     // 构造最小大气世界变换。
        pbr::{AtmosphereSettings, ExtractedAtmosphere, GpuAtmosphereSettings}, // 建立有效与残留大气RenderWorld状态。
        prelude::{App, Camera3d, Update, Vec3}, // 运行一次普通调度并构造两台三维相机。
        render::{MainWorld, sync_world::RenderEntity}, // 在测试App内模拟提取系统读取的双世界关系。
    }; // 结束测试依赖导入。

    use super::cleanup_removed_atmosphere_render_state; // 直接验证正式兼容系统而非复制判断逻辑。

    /// 构造不依赖真实散射资源或GPU设备的RenderWorld大气身份。
    fn extracted_atmosphere() -> ExtractedAtmosphere {
        // 两台相机使用独立值实例避免组件所有权复用。
        ExtractedAtmosphere {
            // 字段只需满足组件构造，测试不执行任何LUT计算。
            inner_radius: 1.0,                   // 使用有限正半径模拟行星表面。
            outer_radius: 2.0,                   // 外半径大于内半径以保持语义合法。
            ground_albedo: Vec3::ONE,            // 中性地表反照率不影响清理判断。
            medium: AssetId::invalid(),          // 清理发生在资源解析前，因此无需登记真实散射介质。
            world_to_atmosphere: Mat4::IDENTITY, // 单位变换足以代表已经提取的旧状态。
        } // 完成最小公开大气组件构造。
    } // 测试夹具结束。

    #[test] // 锁定无设置相机被清理、有效相机保持不变的双向边界。
    fn removed_settings_clear_stale_render_state_without_touching_active_camera() {
        // 精确复现旧提取器遗漏的分支条件。
        let mut app = App::new(); // 当前App世界充当RenderWorld并执行兼容系统。
        let stale_render_entity = app // 先创建即将失去主世界大气资格的渲染实体。
            .world_mut() // 取得测试RenderWorld可变访问。
            .spawn((
                // 模拟上一帧已经完成大气提取与GPU设置准备。
                extracted_atmosphere(), // 旧版提取器会错误保留的公开大气身份。
                GpuAtmosphereSettings::default(), // 旧版同步/提取链遗留的GPU设置源。
            )) // 完成残留RenderWorld组件安装。
            .id(); // 保存主世界RenderEntity映射目标。
        let active_render_entity = app // 再创建本帧仍应保留大气的渲染实体。
            .world_mut() // 继续写入同一测试RenderWorld。
            .spawn((
                // 有效相机使用相同公开组件，区别仅来自主世界设置资格。
                extracted_atmosphere(), // 当前有效动态天空的大气身份。
                GpuAtmosphereSettings::default(), // 当前有效动态天空的GPU设置源。
            )) // 完成有效RenderWorld组件安装。
            .id(); // 保存第二台相机映射目标。
        let mut main_world = MainWorld::default(); // 构造Extract系统读取的独立主世界资源。
        main_world.spawn((
            // 第一台主世界相机没有AtmosphereSettings，代表天空盒已取得所有权。
            Camera3d::default(), // 只有三维相机才可能拥有上述残留大气节点。
            RenderEntity::from(stale_render_entity), // 映射到待清理RenderWorld实体。
        )); // 完成失去资格相机构造。
        main_world.spawn((
            // 第二台主世界相机继续保留动态大气设置。
            Camera3d::default(),           // 使用相同三维相机身份作为对照。
            AtmosphereSettings::default(), // 明确保留大气资格，required HDR也继续有效。
            RenderEntity::from(active_render_entity), // 映射到不得误删的RenderWorld实体。
        )); // 完成有效相机构造。
        app.insert_resource(main_world); // Extract参数初始化前必须提供可读取的主世界。
        app.add_systems(Update, cleanup_removed_atmosphere_render_state); // 普通测试调度复用同一Extract系统参数。

        app.update(); // 运行一次提取兼容清理并在调度末应用Commands。

        assert!(
            // 失去主世界设置后不再保留大气准备身份。
            app.world()
                .get::<ExtractedAtmosphere>(stale_render_entity)
                .is_none() // 后续纹理与绑定准备不会再枚举该相机。
        ); // 完成残留身份断言。
        assert!(
            // 失去主世界设置后不再保留GPU设置源。
            app.world()
                .get::<GpuAtmosphereSettings>(stale_render_entity)
                .is_none() // uniform准备不会再为该相机分配新索引。
        ); // 完成残留设置断言。
        assert!(
            // 仍有主世界设置的相机必须保持现有大气状态。
            app.world()
                .get::<ExtractedAtmosphere>(active_render_entity)
                .is_some() // 兼容层不得破坏正常动态天空。
        ); // 完成有效身份断言。
        assert!(
            // 有效相机的GPU设置同样必须保留。
            app.world()
                .get::<GpuAtmosphereSettings>(active_render_entity)
                .is_some() // Bevy原生后续阶段仍可生成LUT并绘制天空。
        ); // 完成有效设置断言。
    } // 回归用例结束。
} // 测试模块结束。
