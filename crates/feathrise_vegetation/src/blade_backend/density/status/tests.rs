//! 可见叶片窗口采用状态的计数、版本与安全托底回归。

use bevy::prelude::{App, Entity, PostUpdate}; // 最小主世界足以验证PostUpdate观察延迟命令后的组件身份。

use super::*; // 直接验证私有分类器与状态同步系统。

/// 当前版本全部窗口身份匹配前不得宣称解析布局已采用。
#[test]
fn visible_status_requires_every_window_to_match_revision_and_layer() {
    let mut app = App::new(); // 不安装RenderApp，状态系统本身只观察主世界组件。
    app.init_resource::<ActiveVegetationLayout>() // 提供活动布局版本。
        .init_resource::<VegetationBladeDensityCache>() // 提供拒绝记录。
        .init_resource::<VegetationGpuMipCoordinator>() // 提供候选在途状态。
        .init_resource::<VegetationBladeDensityCpuBuild>() // 默认没有后台CPU候选。
        .init_resource::<VegetationBladeAdoptionStatus>() // 提供公开只读快照。
        .add_systems(PostUpdate, sync_blade_adoption_status); // 使用与生产插件相同调度阶段。
    app.world_mut()
        .resource_mut::<ActiveVegetationLayout>()
        .revision = 7; // 模拟核心已经原子发布第七版布局。
    let grass_layer = VegetationLayerId(11); // 第一层模拟同层两个LOD窗口。
    let wheat_layer = VegetationLayerId(12); // 第二层模拟独立内容窗口。
    let first = app
        .world_mut()
        .spawn(VegetationBladeLayer(grass_layer))
        .id(); // 第一个普通草窗口尚未采用。
    let second = app
        .world_mut()
        .spawn(VegetationBladeLayer(grass_layer))
        .id(); // 第二个LOD共享同一层。
    let third = app
        .world_mut()
        .spawn(VegetationBladeLayer(wheat_layer))
        .id(); // 麦田窗口形成第二个去重层。
    app.update(); // 发布初始托底快照。
    let fallback = *app.world().resource::<VegetationBladeAdoptionStatus>(); // 冻结资源避免后续可变借用冲突。
    assert_eq!(
        fallback.phase(),
        VegetationBladeAdoptionPhase::FallbackRetained
    ); // 绑定出现后仍保留各自直接密度资产。
    assert_eq!(fallback.active_revision(), 7); // 快照明确对应第七版。
    assert_eq!(fallback.bound_layer_count(), 2); // 两个草LOD只计一个层。
    assert_eq!(fallback.bound_window_count(), 3); // 三个可见窗口全部进入分母。
    assert_eq!(fallback.adopted_window_count(), 0); // 尚无私有采用身份。

    insert_applied_revision(&mut app, first, 7, grass_layer); // 第一个草窗采用当前层。
    insert_applied_revision(&mut app, second, 7, grass_layer); // 第二个草窗采用当前层。
    insert_applied_revision(&mut app, third, 6, wheat_layer); // 麦田仍故意停留在旧版本。
    app.update(); // 重新统计部分采用。
    let partial = *app.world().resource::<VegetationBladeAdoptionStatus>(); // 读取部分完成快照。
    assert_eq!(
        partial.phase(),
        VegetationBladeAdoptionPhase::FallbackRetained
    ); // 任一窗口未采用当前版都不能宣称完成。
    assert_eq!(partial.adopted_window_count(), 2); // 旧版本麦田不计入当前采用。
    assert!(!partial.is_current_layout_fully_adopted()); // 公共布尔门保持关闭。

    insert_applied_revision(&mut app, third, 7, wheat_layer); // 最后窗口采用当前版和正确层。
    app.update(); // 发布完整采用快照。
    let adopted = *app.world().resource::<VegetationBladeAdoptionStatus>(); // 读取最终快照。
    assert_eq!(
        adopted.phase(),
        VegetationBladeAdoptionPhase::CurrentLayoutAdopted
    ); // 三窗全部匹配后才进入当前布局采用态。
    assert_eq!(adopted.adopted_window_count(), 3); // 分子与分母完全一致。
    assert!(adopted.is_current_layout_fully_adopted()); // 公共便利门只在非空全采用时为真。
}

/// 当前版本拒绝必须可观察，窗口离场后状态必须回到未绑定。
#[test]
fn rejection_keeps_fallback_visible_and_despawn_resets_status() {
    let mut app = App::new(); // 创建独立状态生命周期测试。
    app.init_resource::<ActiveVegetationLayout>() // 提供活动版本。
        .init_resource::<VegetationBladeDensityCache>() // 测试直接写入当前拒绝身份。
        .init_resource::<VegetationGpuMipCoordinator>() // 无在途候选。
        .init_resource::<VegetationBladeDensityCpuBuild>() // 默认没有后台CPU候选。
        .init_resource::<VegetationBladeAdoptionStatus>() // 提供公开快照。
        .add_systems(PostUpdate, sync_blade_adoption_status); // 使用生产同步系统。
    app.world_mut()
        .resource_mut::<ActiveVegetationLayout>()
        .revision = 9; // 模拟第九版布局已经活动。
    let layer = VegetationLayerId(21); // 任意可见叶片层。
    let window = app.world_mut().spawn(VegetationBladeLayer(layer)).id(); // 窗口保留自身直接密度。
    app.world_mut()
        .resource_mut::<VegetationBladeDensityCache>()
        .rejected
        .insert((9, layer)); // 模拟当前层GPU字节门失败。
    app.update(); // 发布拒绝托底快照。
    let rejected = *app.world().resource::<VegetationBladeAdoptionStatus>(); // 读取拒绝状态。
    assert_eq!(
        rejected.phase(),
        VegetationBladeAdoptionPhase::RejectedKeepingFallback
    ); // F3必须明确显示失败而不是无限等待。
    assert_eq!(rejected.adopted_window_count(), 0); // 拒绝从不伪造窗口采用身份。

    app.world_mut().despawn(window); // 模拟场景离场回收可见叶片窗。
    app.update(); // 发布离场后的空快照。
    let unbound = *app.world().resource::<VegetationBladeAdoptionStatus>(); // 读取离场状态。
    assert_eq!(unbound.phase(), VegetationBladeAdoptionPhase::Unbound); // 后台拒绝记录不冒充可见绑定。
    assert_eq!(unbound.bound_layer_count(), 0); // 离场清零可见层数。
    assert_eq!(unbound.bound_window_count(), 0); // 离场清零可见窗口数。
}

/// 阶段优先级必须区分在途校验、拒绝托底和空世界。
#[test]
fn phase_classifier_preserves_safe_priority() {
    assert_eq!(
        classify_adoption_phase(4, 2, 0, true, false),
        VegetationBladeAdoptionPhase::Validating
    ); // 当前批次在途时明确报告校验中。
    assert_eq!(
        classify_adoption_phase(4, 2, 0, true, true),
        VegetationBladeAdoptionPhase::RejectedKeepingFallback
    ); // 当前拒绝优先于可能残留的在途诊断。
    assert_eq!(
        classify_adoption_phase(4, 0, 0, true, true),
        VegetationBladeAdoptionPhase::Unbound
    ); // 没有可见窗口时任何后台状态都不能冒充采用。
    assert_eq!(
        classify_adoption_phase(4, 2, 2, false, true),
        VegetationBladeAdoptionPhase::CurrentLayoutAdopted
    ); // 全窗真实组件身份优先于可清理的历史拒绝记录。
}

/// 给指定窗口写入后端私有采用身份。
fn insert_applied_revision(
    app: &mut App,            // 修改测试主世界。
    entity: Entity,           // 目标可见窗口。
    revision: u64,            // 模拟已采用版本。
    layer: VegetationLayerId, // 模拟已采用层身份。
) {
    app.world_mut()
        .entity_mut(entity)
        .insert(VegetationBladeLayoutRevision { revision, layer }); // 使用生产组件形状锁定双身份比较。
}
