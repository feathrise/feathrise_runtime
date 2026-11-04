//! 真实Bevy应用中的近场滞回、配置清空和索引失效生命周期回归。

use bevy::prelude::{GlobalTransform, Messages, Transform, Vec3}; // 构造观察者并读取真实消息队列。

use crate::{VegetationLayoutState, VegetationNearFieldExited, VegetationSourceField}; // 驱动配置和源变化。

use super::super::{VegetationNearFieldObserver, VegetationNearFieldRuntime}; // 读取真实近场公开资源。
use super::fixture::{TEST_BOUNDS, TEST_LAYER, setup_app}; // 复用单层合法应用fixture。

#[test] // 锁定进入、滞回保持和越过外圈退出的原行为。
fn hybrid_proxy_uses_enter_exit_hysteresis_without_duplicates() {
    // 空间索引不得改变最终语义。
    let mut app = setup_app(); // 创建两米处唯一显式代理。
    let observer = app // 生成位于世界原点的近场观察者。
        .world_mut() // 取得可变测试世界。
        .spawn((
            // 同时保存本地和世界Transform。
            VegetationNearFieldObserver,             // 标记近场观察者。
            Transform::from_translation(Vec3::ZERO), // 本地位置为原点。
            GlobalTransform::IDENTITY,               // 世界位置为原点。
        )) // 完成观察者生成。
        .id(); // 保存实体ID供后续移动。
    app.update(); // 两米小于三米进入圈。
    assert!(
        app.world()
            .resource::<VegetationNearFieldRuntime>()
            .contains(TEST_LAYER, 0)
    ); // 唯一代理应进入。
    app.world_mut() // 移动观察者形成四米距离。
        .entity_mut(observer) // 取得观察者实体。
        .insert(Transform::from_xyz(-2.0, 0.0, 0.0)) // 更新本地Transform。
        .insert(GlobalTransform::from_translation(Vec3::new(-2.0, 0.0, 0.0))); // 更新世界Transform。
    app.update(); // 四米位于三至五米滞回区。
    assert_eq!(
        app.world()
            .resource::<VegetationNearFieldRuntime>()
            .active_count(),
        1
    ); // 代理继续保持活动。
    app.world_mut() // 再移动观察者形成六米距离。
        .entity_mut(observer) // 取得同一观察者。
        .insert(Transform::from_xyz(-4.0, 0.0, 0.0)) // 更新本地Transform。
        .insert(GlobalTransform::from_translation(Vec3::new(-4.0, 0.0, 0.0))); // 更新世界Transform。
    app.update(); // 六米越过五米退出圈。
    assert_eq!(
        app.world()
            .resource::<VegetationNearFieldRuntime>()
            .active_count(),
        0
    ); // 代理应退出。
    let exits = app
        .world()
        .resource::<Messages<VegetationNearFieldExited>>(); // 读取真实退出消息。
    assert!(!exits.is_empty()); // 消费者必须收到回收通知。
} // 完成近场滞回回归。

#[test] // 配置清空时即使观察者静止也必须释放旧代理。
fn clearing_layers_releases_proxies_without_observer_motion() {
    // 配置代次继续触发原差分语义。
    let mut app = setup_app(); // 创建基础显式近场层。
    app.world_mut().spawn((
        // 观察者始终保持原点。
        VegetationNearFieldObserver,             // 标记近场观察者。
        Transform::from_translation(Vec3::ZERO), // 本地位置为原点。
        GlobalTransform::IDENTITY,               // 世界位置为原点。
    )); // 完成静止观察者生成。
    app.update(); // 首次差分建立唯一代理。
    assert_eq!(
        app.world()
            .resource::<VegetationNearFieldRuntime>()
            .active_count(),
        1
    ); // 确认代理已活动。
    app.world_mut() // 原子清空全部层。
        .resource_mut::<VegetationLayoutState>() // 取得布局配置。
        .set_configuration(Vec::new(), Vec::new()) // 推进独立配置代次。
        .unwrap(); // 空配置合法。
    app.update(); // 观察者和活动布局均未移动。
    assert_eq!(
        app.world()
            .resource::<VegetationNearFieldRuntime>()
            .active_count(),
        0
    ); // 旧代理必须释放。
    assert!(
        !app.world()
            .resource::<Messages<VegetationNearFieldExited>>()
            .is_empty()
    ); // 必须发布退出消息。
} // 完成配置清空回归。

#[test] // 密度源换版必须重算启用集合但不重建显式位置索引。
fn source_field_changes_do_not_rebuild_explicit_indexes() {
    // 独立配置代次限定索引失效边界。
    let mut app = setup_app(); // 配置显式层和初始源。
    app.update(); // 首次采用配置第一代索引。
    let (initial_generation, initial_rebuilds) = {
        // 记录索引身份与真实构造次数。
        let runtime = app.world().resource::<VegetationNearFieldRuntime>(); // 读取运行资源。
        (
            runtime.explicit_indexes.configuration_generation(),
            runtime.explicit_indexes.rebuild_count(),
        ) // 返回初始证据。
    }; // 释放运行资源借用。
    app.world_mut() // 只替换密度源。
        .resource_mut::<VegetationLayoutState>() // 取得布局状态。
        .set_source_field(
            // 仅推进总状态代次。
            TEST_LAYER, // 替换唯一测试层源。
            VegetationSourceField::constant(TEST_BOUNDS, 0.5).unwrap(), // 使用不同常量。
        ); // 完成源字段换版。
    app.update(); // 重新判断候选但复用空间桶。
    let runtime = app.world().resource::<VegetationNearFieldRuntime>(); // 读取换版后运行资源。
    assert_eq!(
        runtime.explicit_indexes.configuration_generation(),
        initial_generation
    ); // 配置身份不变。
    assert_eq!(runtime.explicit_indexes.rebuild_count(), initial_rebuilds); // 不发生额外重建。
    let (groups, layers) = {
        // 等值克隆当前配置用于真实整批替换。
        let state = app.world().resource::<VegetationLayoutState>(); // 读取当前配置。
        (state.groups().to_vec(), state.layers().to_vec()) // 克隆完整组和层。
    }; // 释放配置借用。
    app.world_mut() // 执行set_configuration推进独立配置代次。
        .resource_mut::<VegetationLayoutState>() // 取得布局状态。
        .set_configuration(groups, layers) // 原子替换等值配置。
        .unwrap(); // 等值配置仍合法。
    app.update(); // 新配置代次必须重建索引。
    let runtime = app.world().resource::<VegetationNearFieldRuntime>(); // 读取最终运行资源。
    assert_eq!(
        runtime.explicit_indexes.rebuild_count(),
        initial_rebuilds + 1
    ); // 仅配置替换新增一次重建。
} // 完成索引失效边界回归。
