//! 碰撞生命周期与候选失败处理；旧CPU任务实际结束前保持单飞槽。
use feathrise_scene_core::residency::{InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use super::super::resources::{DynamicSceneRuntimePhase, DynamicSceneRuntimeStatus};
use super::resources::LayeredCollisionRuntime; // 此域唯一状态。
use bevy::{prelude::*, tasks::futures::check_ready}; // 只允许非阻塞回收。
use feathrise_scene_core::residency::TerrainCollisionRegion; // 离图撤销活动门。
use feathrise_scene_content::SceneCollisionLodStatus; // 保存与采用状态。 // 首次失败进入明确错误门。
pub(super) fn fail(
    state: &mut LayeredCollisionRuntime,
    feedback: &mut SceneCollisionLodStatus,
    status: &mut DynamicSceneRuntimeStatus,
    error: String,
) {
    // 失败只影响候选。
    state.failed_ticket = Some(state.ticket); // 同一坏输入不重复派发。
    feedback.detail = format!("新碰撞未采用：{error}"); // 保存不等于已经生效。
    status.detail = feedback.detail.clone(); // 统一错误面板。
    if state.resident.is_empty() {
        status.phase = DynamicSceneRuntimePhase::Failed;
    } // 首次失败不能伪装就绪。
} // 有旧版本时继续安全承托。

pub(super) fn reject_input(
    state: &mut LayeredCollisionRuntime,
    feedback: &mut SceneCollisionLodStatus,
    status: &mut DynamicSceneRuntimeStatus,
    error: String,
) {
    // 尚未建立需求时，错误不属于上一个有效候选票据。
    fail(state, feedback, status, error); // 同样向界面报告并保留上一地面。
    state.failed_ticket = None; // 恢复原有效配置后允许继续旧任务或复用旧版，不要求玩家移动才能解锁。
} // 调用方立即return，不会为无效输入派发任务。

pub(super) fn reset(commands: &mut Commands, state: &mut LayeredCollisionRuntime) {
    // 切换场景释放已采用集合，但保留仍在运行的CPU槽。
    let applied = state.applied_layout.take(); // 先移走完整布局，禁止后面赋default时主帧析构规划树。
    super::retirement::applied(state, applied); // 重复reset时此值已为空，不增加历史队列。
    let candidate = state.layout.take(); // 唯一等待IO候选同样转移。
    super::retirement::candidate(state, candidate); // 与已采用布局各占一个固定槽。
    for (_, resident) in std::mem::take(&mut state.resident) {
        commands.entity(resident.entity).try_despawn();
    } // 旧根可能已销毁。
    let task = state.task.take(); // 不丢弃执行中的计算句柄。
    let leases = std::mem::take(&mut state.leases); // 跨场景未完成IO仍占页槽，回收器只释放实际完成的旧请求。
    let ticket = state.ticket.wrapping_add(1); // 新图绝不接纳旧结果。
    let retire_task = state.retire_task.take(); // 后台释放实际结束前保留并发槽。
    let retired = std::mem::take(&mut state.retired); // 固定待释放集合跨场景继续排空。
    *state = LayeredCollisionRuntime {
        task,
        ticket,
        leases,
        retire_task, // 切换场景不能取消正在执行的析构。
        retired,     // 不在主帧释放尚未提交后台的缓存。
        ..default()
    }; // 大页和候选引用随旧状态释放。
} // 没有跨场景形状复用。

pub(crate) fn clear_layered_collision(
    mut commands: Commands,
    mut state: InstanceResMut<LayeredCollisionRuntime>,
    mut region: InstanceResMut<TerrainCollisionRegion>,
    mut feedback: InstanceResMut<SceneCollisionLodStatus>,
) {
    // 离场只归还本域资源。
    deactivate(&mut commands, &mut state, &mut region, &mut feedback); // 离场与同场景改用GLB共用精确清理路径。
} // 不改显示网格。

pub(super) fn deactivate(
    commands: &mut Commands,
    state: &mut LayeredCollisionRuntime,
    region: &mut TerrainCollisionRegion,
    feedback: &mut SceneCollisionLodStatus,
) {
    // 并非所有地形切换都会触发GameScene的OnExit。
    reset(commands, state); // 旧CPU任务稍后被常驻回收系统读取。
    region.enabled = false; // 活动门据此恢复自己拥有的约束。
    region.supported_bounds.clear(); // 禁止跨场景承托。
    region.supported_polygon.clear(); // 凸外环与块范围一同退出，不能残留上一场景的裁圆限制。
    region.supported_rows = None; // 不能沿用上一场景的完整格覆盖。
    region.revision = region.revision.wrapping_add(1); // 缓存必须失效。
    *feedback = default(); // 不显示上一场景采用状态。
} // 不改显示网格。

pub(crate) fn reap_layered_collision(
    mut state: InstanceResMut<LayeredCollisionRuntime>,
    server: Res<AssetServer>,
) {
    // 非动态场景仍回收旧任务槽，避免下一次进图额外并发。
    super::retirement::poll(&mut state); // 离场后也推进固定后台释放槽。
    if state.scene_id.is_none() && !super::retirement::busy(&state) {
        if let Some(result) = state.task.as_mut().and_then(check_ready) {
            // 唯一迟到完成值保留所有权。
            state.task = None; // 任务真正结束才归还CPU槽。
            super::retirement::completion(&mut state, result.completion); // 不以is_some隐式在主帧drop大型完成值。
            super::retirement::poll(&mut state); // 后台立即释放，不要求重新进入场景。
        } // 未完成仍留在单飞槽。
    } // 非阻塞且不采用旧完成值。
    if state.scene_id.is_none() {
        state.leases.retain(|_, lease| {
            matches!(
                server.load_state(lease.handle.id()),
                bevy::asset::LoadState::NotLoaded | bevy::asset::LoadState::Loading
            )
        });
    } // 离场后仅保留真实未结束IO，GLB场景不累积旧页。
} // 不必等待阻塞应用。

#[cfg(test)] // 切换场景和GLB回退均不得遗留物理资格。
mod tests {
    use super::super::{
        resources::{Lease, Resident, SourceStamp},
        types::{ChunkKey, ChunkPlan},
    }; // 构造小型旧集合。
    use super::*; // 正式清理系统。
    use feathrise_scene_content::{SceneId, sparse_height::SparseHeightTileKey}; // 跨场景旧IO身份。

    #[test] // 外部坏配置撤回不应阻塞原有效需求。
    fn input_rejection_does_not_poison_previous_ticket() {
        let mut state = LayeredCollisionRuntime {
            ticket: 42,
            ..default()
        }; // 已有需求代次。
        let mut feedback = SceneCollisionLodStatus::default(); // 独立回显。
        let mut status = DynamicSceneRuntimeStatus::default(); // 启动状态。
        reject_input(&mut state, &mut feedback, &mut status, "非法距离".into()); // 尚未建立坏需求。
        assert_eq!(state.ticket, 42); // 没有伪造一个可发布候选。
        assert_eq!(state.failed_ticket, None); // 原需求可以继续。
        fail(
            &mut state,
            &mut feedback,
            &mut status,
            "真实任务失败".into(),
        ); // 候选自身错误仍需去重。
        assert_eq!(state.failed_ticket, Some(42)); // 不退化为失败任务逐帧重派。
    } // 两类错误使用不同票据处理。

    #[test] // ECS清理不依赖图形或真实IO。
    fn deactivation_removes_ground_and_retains_unfinished_io_lease() {
        let mut app = App::new(); // 无窗口世界。
        let entity = app.world_mut().spawn_empty().id(); // 旧碰撞实体身份。
        let scene_id = SceneId("old-map".into()); // 原场景。
        let key = ChunkKey {
            band: 0,       // 第一个圆形LOD的固定块身份。
            cells_log2: 5, // 固定夹具采用32×32单元管理块。
            level: 0,
            x: 0,
            z: 0,
        }; // 一个旧叶块。
        let mut state = LayeredCollisionRuntime {
            scene_id: Some(scene_id.clone()),
            ticket: 9,
            applied_layout: Some(std::sync::Arc::new(super::super::resources::Layout {
                // 模拟真实旧缓存，验证reset不是只碰巧保留默认None。
                planning_cache: None, // 此夹具仅验证布局清理；真实规划快照在独立生命周期测试覆盖。
                chunks: Default::default(), // 清理只测所有权，不需要顶点。
                tiles: Default::default(), // 没有真实IO依赖。
                supported_polygon: Vec::new(), // 完整格路径为空。
                supported_rows: None, // 不参与几何预检。
                elapsed_ms: 1.0,      // 旧版诊断。
                rebuilt_blocks: 0,    // 空清理夹具。
                reused_blocks: 0,     // 空清理夹具。
            })), // 缓存应随场景身份退出。
            request_started: Some(std::time::Instant::now()), // 旧票据时钟也必须清理。
            ..default()
        }; // 已处于旧图。
        state.resident.insert(
            key,
            Resident {
                entity,
                plan: ChunkPlan {
                    signature: String::new(), // 清理夹具只测试完整固定块生命周期。
                    cells: None,              // 完整块夹具无需显式单元。
                    key,
                    finer_edges: 0,
                },
                stamp: SourceStamp {
                    sources: Vec::new(),
                },
            },
        ); // 无需真正烘焙形状。
        state.leases.insert(
            SparseHeightTileKey { x: 0, z: 0 },
            Lease {
                scene_id,
                hash: "old".into(),
                handle: Handle::default(),
            },
        ); // 清理不伪装取消IO。
        app.insert_resource(state); // 发布测试旧集合。
        app.insert_resource(TerrainCollisionRegion {
            enabled: true,
            revision: 1,
            supported_bounds: vec![(Vec2::ZERO, Vec2::ONE)],
            supported_polygon: vec![Vec2::ZERO, Vec2::X, Vec2::ONE], // 旧场景的裁圆外环同样必须清理。
            ..default()
        }); // 原有承托覆盖。
        app.init_resource::<SceneCollisionLodStatus>(); // 清理公开状态。
        app.add_systems(Update, clear_layered_collision); // 正式入口调用同一deactivate路径。
        app.update(); // 应用完整清理Commands。
        assert!(app.world().get_entity(entity).is_err()); // 旧地面不会留在GLB世界。
        let state = app.world().resource::<LayeredCollisionRuntime>(); // 新请求必须取得新票据。
        assert!(state.scene_id.is_none() && state.resident.is_empty()); // 不残留旧采用状态。
        assert!(state.applied_layout.is_none() && state.request_started.is_none()); // 切换场景同时撤销已采用布局缓存与旧票据延迟起点。
        assert_eq!(state.ticket, 10); // 迟到CPU结果失去发布权。
        assert_eq!(state.leases.len(), 1); // 真正IO完成后再归还槽位。
        let region = app.world().resource::<TerrainCollisionRegion>(); // 下次活动门归还自己的约束。
        assert!(!region.enabled && region.supported_bounds.is_empty()); // 不用旧范围限制GLB角色。
        assert!(region.supported_polygon.is_empty()); // 不让下一场景误用旧几何裁边。
    } // 生命周期回归结束。
} // 测试不请求真实文件。
