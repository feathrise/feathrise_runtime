//! 布局消息、局部与整批重算、失败托底和原子换版回归。

use bevy::prelude::{App, IVec2, Rect, Vec2, World}; // 用真实Bevy调度验证事件触发与原子切换。
use bevy::tasks::{AsyncComputeTaskPool, TaskPoolBuilder}; // 无渲染应用也可真实驱动大批后台worker。

use crate::{
    VegetationCandidateSource, VegetationClaimSampling, VegetationCompetition,
    VegetationCompetitionGroup, VegetationCompetitionGroupId, VegetationCompetitionMode,
    VegetationDensityField, VegetationFootprint, VegetationFootprintScale,
    VegetationFootprintShape, VegetationLayerConfig, VegetationLayerId, VegetationPlugin,
    VegetationPrototypeId, VegetationRealization, VegetationSourceField, VegetationWeight,
}; // 构造最小合法公共配置。

use super::*; // 测试内部系统无需额外公开测试钩子。

fn layer(id: u64, group: u64) -> VegetationLayerConfig {
    VegetationLayerConfig {
        id: VegetationLayerId(id),            // 稳定测试层身份。
        prototype: VegetationPrototypeId(id), // 原型身份可与层相同但并非接口要求。
        source: VegetationCandidateSource::DenseGrid {
            spacing: 1.0, // 每米一个候选。
            seed: id,     // 每层独立种子。
        },
        density: VegetationDensityField {
            world_bounds: Rect::from_corners(Vec2::ZERO, Vec2::new(128.0, 64.0)), // 测试层可覆盖相邻两个64米块。
            gain: 1.0,                                                            // 不改变源密度。
        },
        competition: VegetationCompetition {
            group: VegetationCompetitionGroupId(group), // 显式引用测试组。
            priority: 0,                                // 同级竞争。
            weight: VegetationWeight::default(),        // 等权。
            claim_sampling: VegetationClaimSampling::Point, // 默认点所有权。
            excludes_groups: Vec::new(),                // 本测试不需要跨组足迹。
        },
        footprint: VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 0.1 }, // 合法小足迹。
            scale: VegetationFootprintScale::InheritTransform, // 测试轮廓使用公共默认缩放口径。
            exclusion_margin: 0.0,                             // 无额外净空。
            recovery_width: 0.0,                               // 无软恢复。
        },
        realization: VegetationRealization::GpuOnly, // 只验证布局场。
    }
}

fn configure(world: &mut World, mode: VegetationCompetitionMode) {
    let mut state = world.resource_mut::<VegetationLayoutState>();
    state.tile_budget_per_frame = 1; // 一个测试块在单次update完成。
    state
        .set_configuration(
            vec![VegetationCompetitionGroup {
                id: VegetationCompetitionGroupId(1), // 唯一测试组。
                mode,                                // 调用者选择竞争语义。
                selection_cell_size: 1.0,            // 固定世界格。
                selection_seed: 5,                   // 固定选择种子。
            }],
            vec![layer(10, 1), layer(20, 1)], // 两个无限层不占用GPU四槽。
        )
        .unwrap();
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0));
    state.set_source_field(
        VegetationLayerId(10),
        VegetationSourceField::new(bounds, 65, 65, vec![1.0; 65 * 65]).unwrap(), // 六十五个端点样本形成六十四段一米间距。
    );
    state.set_source_field(
        VegetationLayerId(20),
        VegetationSourceField::new(bounds, 65, 65, vec![1.0; 65 * 65]).unwrap(), // 第二层使用完全相同的端点式输入精度。
    );
}

fn send_dirty(world: &mut World) {
    world
        .resource_mut::<bevy::ecs::message::Messages<crate::MarkVegetationDirty>>()
        .write(crate::MarkVegetationDirty {
            min_tile: IVec2::ZERO, // 只重算世界原点块。
            max_tile: IVec2::ZERO, // 闭区间只有一个块。
        });
}

fn send_dirty_rect(world: &mut World, min_tile: IVec2, max_tile: IVec2) {
    world
        .resource_mut::<bevy::ecs::message::Messages<crate::MarkVegetationDirty>>()
        .write(crate::MarkVegetationDirty {
            min_tile, // 调用者声明本批次最小闭区间块。
            max_tile, // 调用者声明本批次最大闭区间块。
        });
}

fn configure_two_tiles(world: &mut World) {
    let mut state = world.resource_mut::<VegetationLayoutState>();
    state.tile_budget_per_frame = 2; // 整图批次可在一次update完成两个相邻块。
    state
        .set_configuration(
            vec![VegetationCompetitionGroup {
                id: VegetationCompetitionGroupId(1), // 唯一测试组。
                mode: VegetationCompetitionMode::WeightedPartition { capacity: 1.0 }, // 同一纯解析覆盖批次边界。
                selection_cell_size: 1.0, // 固定世界选择格。
                selection_seed: 5,        // 固定选择种子。
            }],
            vec![layer(10, 1), layer(20, 1)], // 两个层跨越两个块。
        )
        .unwrap();
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::new(128.0, 64.0)); // 两个水平64米块共享源范围。
    let first_values = (0..129 * 65)
        .map(|index| if index % 129 < 96 { 1.0 } else { 0.25 })
        .collect::<Vec<_>>(); // 跨块边界使用非均匀值放大坐标或批次差异。
    state.set_source_field(
        VegetationLayerId(10),
        VegetationSourceField::new(bounds, 129, 65, first_values).unwrap(), // 端点式样本保持两轴一米间距。
    );
    state.set_source_field(
        VegetationLayerId(20),
        VegetationSourceField::new(bounds, 129, 65, vec![0.75; 129 * 65]).unwrap(), // 第二层提供固定竞争需求。
    );
}

#[test]
fn dirty_recompute_switches_one_complete_atomic_revision() {
    let mut app = App::new(); // 不安装渲染或默认插件也能运行公共核心。
    app.add_plugins(VegetationPlugin);
    configure(
        app.world_mut(),
        VegetationCompetitionMode::WeightedPartition { capacity: 1.0 },
    );
    send_dirty(app.world_mut());
    app.update(); // 同帧收集并完成唯一脏块。
    let active = app.world().resource::<ActiveVegetationLayout>();
    assert_eq!(active.revision, 1); // 整批只切换一个新版本。
    let commit = app.world().resource::<VegetationLayoutCommit>();
    assert_eq!(commit.revision(), active.revision); // 提交快照与活动布局必须同帧原子对齐。
    assert_eq!(
        commit.configuration_generation(),
        app.world().resource::<VegetationLayoutState>().generation()
    ); // 提交还必须锁定实际完成合成的配置与源字段代次。
    assert_eq!(
        commit.dirty_tiles(),
        &[crate::VegetationDirtyTile(IVec2::ZERO)]
    ); // 即使处理集合已经消费为空，后端仍取得完整原始批次。
    let tile = active
        .tiles
        .get(&crate::VegetationDirtyTile(IVec2::ZERO))
        .unwrap();
    assert_eq!(tile.layers.len(), 2); // 层数量与叶片批次四槽完全无关。
    assert!(tile.layers.iter().all(|layer| {
        layer
            .density
            .iter()
            .all(|density| (*density - 0.5).abs() < 1.0e-6)
    })); // 等权100/100逐纹素得到50/50。
}

#[test]
fn large_async_batch_publishes_active_layout_and_commit_only_once() {
    AsyncComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(2).build()); // 测试进程只初始化一次最小后台池。
    let mut app = App::new(); // 不安装窗口、渲染或默认插件。
    app.add_plugins(VegetationPlugin); // 使用正式布局资源和Update系统。
    send_dirty_rect(app.world_mut(), IVec2::ZERO, IVec2::new(7, 7)); // 恰好64块触发异步门槛。
    app.update(); // 首帧只启动worker，活动布局必须仍为空。
    assert_eq!(app.world().resource::<ActiveVegetationLayout>().revision, 0); // 后台部分结果不可见。
    for _ in 0..10_000 {
        app.update(); // 每帧只非阻塞轮询worker。
        if app.world().resource::<ActiveVegetationLayout>().revision == 1 {
            break; // 完整批次已原子发布。
        }
        std::thread::yield_now(); // 给本地AsyncCompute worker明确执行机会。
    }
    let active = app.world().resource::<ActiveVegetationLayout>(); // 读取最终活动快照。
    let commit = app.world().resource::<VegetationLayoutCommit>(); // 读取同帧提交快照。
    assert_eq!(active.revision, 1); // 整个64块批次只切换一次版本。
    assert_eq!(active.tiles.len(), 64); // 所有块都完成后才可见。
    assert_eq!(commit.revision(), active.revision); // Active与Commit保持原子同版。
    assert_eq!(commit.dirty_tiles().len(), 64); // 提交保留完整原始dirty全集。
    app.update(); // 稳定帧不得重复发布同一结果。
    assert_eq!(app.world().resource::<ActiveVegetationLayout>().revision, 1); // 没有新dirty时revision保持不变。
}

#[test]
fn missing_source_rejects_new_batch_and_keeps_previous_layout() {
    let mut app = App::new();
    app.add_plugins(VegetationPlugin);
    configure(app.world_mut(), VegetationCompetitionMode::Additive);
    send_dirty(app.world_mut());
    app.update(); // 先建立一个有效版本。
    let previous = app.world().resource::<ActiveVegetationLayout>().clone();
    let previous_commit = app.world().resource::<VegetationLayoutCommit>().clone();
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .remove_source_field(VegetationLayerId(20)); // 模拟密度资产尚未上传或卸载。
    send_dirty(app.world_mut());
    app.update(); // 新批次必须原子拒绝。
    assert_eq!(*app.world().resource::<ActiveVegetationLayout>(), previous);
    assert_eq!(
        *app.world().resource::<VegetationLayoutCommit>(),
        previous_commit
    ); // 失败批次不能让GPU误读一份没有对应活动版本的脏块快照。
    assert!(
        app.world()
            .resource::<VegetationLayoutState>()
            .last_error
            .is_some()
    ); // 结构化错误保留给诊断。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0));
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .set_source_field(
            VegetationLayerId(20),
            VegetationSourceField::new(bounds, 65, 65, vec![0.75; 65 * 65]).unwrap(),
        ); // 修复后的新完整源代次必须能覆盖上一拒绝原因。
    assert!(
        app.world()
            .resource::<VegetationLayoutState>()
            .last_error
            .is_none()
    ); // 场景热更适配器不会把过期错误误判为本代失败。
    send_dirty(app.world_mut());
    app.update(); // 新输入重新计算成功后才替换活动布局。
    assert_eq!(
        app.world().resource::<ActiveVegetationLayout>().revision,
        previous.revision + 1
    ); // 失败版本不消费revision，修复版本只推进一次。
    assert!(
        app.world()
            .resource::<VegetationLayoutState>()
            .last_error
            .is_none()
    ); // 成功提交继续保持无错误状态。
}

#[test]
fn local_batches_match_one_full_recompute_byte_for_byte() {
    let mut full = App::new();
    full.add_plugins(VegetationPlugin);
    configure_two_tiles(full.world_mut());
    send_dirty_rect(full.world_mut(), IVec2::ZERO, IVec2::new(1, 0));
    full.update(); // 一个批次解析两个相邻块并原子发布。
    let full_layout = full.world().resource::<ActiveVegetationLayout>().clone();
    assert_eq!(full_layout.revision, 1); // 整图批次只切换一次版本。
    assert_eq!(
        full.world()
            .resource::<VegetationLayoutCommit>()
            .dirty_tiles(),
        &[
            crate::VegetationDirtyTile(IVec2::ZERO),
            crate::VegetationDirtyTile(IVec2::new(1, 0))
        ]
    ); // 整图提交按Y/X稳定顺序保留两个块。

    let mut local = App::new();
    local.add_plugins(VegetationPlugin);
    configure_two_tiles(local.world_mut());
    send_dirty_rect(local.world_mut(), IVec2::ZERO, IVec2::ZERO);
    local.update(); // 先发布左侧局部块。
    send_dirty_rect(local.world_mut(), IVec2::new(1, 0), IVec2::new(1, 0));
    local.update(); // 再发布右侧局部块。
    let local_layout = local.world().resource::<ActiveVegetationLayout>();
    assert_eq!(local_layout.revision, 2); // 两个完整局部批次各切换一次版本。
    assert_eq!(
        local
            .world()
            .resource::<VegetationLayoutCommit>()
            .dirty_tiles(),
        &[crate::VegetationDirtyTile(IVec2::new(1, 0))]
    ); // 最新提交只包含第二次局部更新，供GPU精确上传。
    assert_eq!(
        local_layout.texel_size.to_bits(),
        full_layout.texel_size.to_bits()
    );
    assert_eq!(local_layout.tiles, full_layout.tiles); // 排除版本号后布局块必须逐值、逐顺序完全相同。
}

#[test]
fn generation_change_requeues_the_whole_invalidated_batch() {
    let mut app = App::new(); // 使用无任务池小批路径精确观察dirty集合生命周期。
    app.add_plugins(VegetationPlugin); // 安装与生产相同布局系统。
    configure_two_tiles(app.world_mut()); // 配置覆盖两个相邻块的完整输入。
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .tile_budget_per_frame = 1; // 首帧只完成旧批一个块并留下部分staging。
    send_dirty_rect(app.world_mut(), IVec2::ZERO, IVec2::new(1, 0)); // 旧批原始全集包含两个块。
    app.update(); // 只处理旧批左块，活动布局仍未发布。
    assert_eq!(app.world().resource::<ActiveVegetationLayout>().revision, 0); // 部分旧批不可见。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::new(128.0, 64.0)); // 新源保持同一正式范围。
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .set_source_field(
            VegetationLayerId(10),
            VegetationSourceField::new(bounds, 129, 65, vec![0.5; 129 * 65]).unwrap(),
        ); // 源热更推进generation并使旧staging整批失效。
    send_dirty_rect(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 新事件故意只覆盖左块。
    app.update(); // 系统必须把旧批完整两块与新左块合并后重新开始。
    assert_eq!(app.world().resource::<ActiveVegetationLayout>().revision, 0); // 新批仍只完成一块，不能误发布。
    app.update(); // 完成被旧批保留下来的右块并原子发布。
    let commit = app.world().resource::<VegetationLayoutCommit>(); // 读取最终完整提交证据。
    assert_eq!(
        commit.dirty_tiles(),
        &[
            crate::VegetationDirtyTile(IVec2::ZERO),
            crate::VegetationDirtyTile(IVec2::new(1, 0)),
        ]
    ); // 即使新事件只有左块，失效旧批的右块也不能永久丢失。
}

#[test]
fn zero_budget_pause_keeps_pending_batch_for_atomic_resume() {
    let mut app = App::new(); // 使用小批同步路径锁定公共暂停语义。
    app.add_plugins(VegetationPlugin); // 安装正式布局状态机。
    configure_two_tiles(app.world_mut()); // 配置两个相邻块。
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .tile_budget_per_frame = 1; // 首帧只完成一个块。
    send_dirty_rect(app.world_mut(), IVec2::ZERO, IVec2::new(1, 0)); // 提交完整两块请求。
    app.update(); // 建立不可见部分staging。
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .tile_budget_per_frame = 0; // 显式暂停布局工作。
    app.update(); // 暂停帧不得继续计算或发布。
    assert_eq!(app.world().resource::<ActiveVegetationLayout>().revision, 0); // 暂停不能把部分结果变为活动布局。
    assert!(
        app.world()
            .resource::<VegetationLayoutState>()
            .pending
            .is_some()
    ); // 原始批次和剩余dirty仍被保留。
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .tile_budget_per_frame = 1; // 恢复原预算。
    app.update(); // 完成剩余块并原子发布。
    assert_eq!(
        app.world()
            .resource::<VegetationLayoutCommit>()
            .dirty_tiles(),
        &[
            crate::VegetationDirtyTile(IVec2::ZERO),
            crate::VegetationDirtyTile(IVec2::new(1, 0)),
        ]
    ); // 恢复后的提交仍携带暂停前完整原始全集。
}

#[test]
fn invalid_configuration_keeps_previous_groups_layers_and_generation() {
    let mut app = App::new();
    app.add_plugins(VegetationPlugin);
    configure(app.world_mut(), VegetationCompetitionMode::Additive); // 先写入一份完整合法配置。
    let state = app.world().resource::<VegetationLayoutState>();
    let previous_groups = state.groups().to_vec(); // 保存当前有效待解析组。
    let previous_layers = state.layers().to_vec(); // 保存当前有效待解析层。
    let previous_generation = state.generation(); // 非法写入不得让处理中版本失效。
    let duplicate = VegetationCompetitionGroup {
        id: VegetationCompetitionGroupId(1), // 与自身重复的稳定组ID。
        mode: VegetationCompetitionMode::Additive,
        selection_cell_size: 1.0,
        selection_seed: 5,
    };
    let error = app
        .world_mut()
        .resource_mut::<VegetationLayoutState>()
        .set_configuration(vec![duplicate, duplicate], vec![layer(10, 1)])
        .unwrap_err(); // 重复组必须在写入资源前返回结构化错误。
    assert!(matches!(
        error,
        crate::VegetationConfigError::DuplicateGroup(VegetationCompetitionGroupId(1))
    ));
    let state = app.world().resource::<VegetationLayoutState>();
    assert_eq!(state.groups(), previous_groups); // 上一组配置原样保留。
    assert_eq!(state.layers(), previous_layers); // 上一层配置原样保留。
    assert_eq!(state.generation(), previous_generation); // 失败配置不触发新代次。
}
