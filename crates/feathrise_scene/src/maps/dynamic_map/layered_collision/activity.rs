//! 逐实体地面活动门：缺地面只暂停对应刚体，绝不使用全世界暂停或刚体启停。
mod sources; // 实际命中、来源坐标与独立区域借用。
use super::activity_components::{TerrainActivityLock, TerrainSupportCache};
use avian3d::prelude::{
    AngularVelocity, ColliderAabb, LinearVelocity, LockedAxes, RigidBody, RigidBodyDisabled,
}; // 直接约束速度，避免 Avian 岛重建问题。
use bevy::prelude::*; // ECS 调度与实体组件。
use crate::SceneRuntimeObserver;
use feathrise_scene_core::residency::{TerrainActivityPaused, TerrainCollisionRegion}; // 共享玩法合同。 // 物理所有权仅本域可写。
mod helpers; // 有界承托查询与锁位运算保持纯函数。
use helpers::{
    activity_reason, constrain_map_prediction, constrain_map_velocity, footprint_crosses_map_edge,
    footprint_supported, lock_translation, restore_translation,
}; // ECS 只协调组件生命周期。

type ActivityData = (
    // 扫描物理参与者，不把地面静态块或纯显示对象当作活动角色。
    Entity,
    &'static Transform,
    &'static RigidBody,
    Option<&'static mut LockedAxes>, // 缺省无轴锁的通用刚体也必须被保护。
    Option<&'static mut LinearVelocity>,
    Option<&'static mut AngularVelocity>, // 可选速度兼容出生安装帧。
    Option<&'static TerrainActivityLock>,
    Option<&'static TerrainActivityPaused>, // 保存本域所有权与公开原因。
    Option<&'static mut TerrainSupportCache>, // 常态检查保持 O(1) 的上次支持块缓存。
    Option<&'static ColliderAabb>,
    Has<RigidBodyDisabled>,
    Option<&'static crate::SceneCollisionParticipant>, // 实际碰撞足迹和外部禁用所有权。
); // 结束参与者查询。

pub(in crate::maps::dynamic_map) fn update_terrain_activity(
    // 在 PreUpdate 与首次候选发布后调用，早于行为施法。
    mut commands: Commands,
    mut root_region: ResMut<TerrainCollisionRegion>,
    mut bodies: Query<ActivityData>, // 不读取作者高度，只有已采用物理块决定承托。
    player: Query<&Transform, With<SceneRuntimeObserver>>,
    fixed_time: Option<Res<Time<Fixed>>>, // PreUpdate/物理步及时同步传送中心，无须等 Update 驱动。
    mut status: Option<ResMut<feathrise_map_content::MapCollisionLodStatus>>, // 只发布暂停数，不碰运行时保存/采用版本。
    mut sources: sources::Sources,
    cached_entities: Query<Entity, With<TerrainSupportCache>>, // 组件过滤使非 Clipmap 场景无需扫描物理参与者。
) {
    // 开始区域差量判断。
    if !root_region.enabled && cached_entities.is_empty() && !sources.has_sources() {
        // 未启用且没有本域恢复工作时直接返回。
        if let Some(status) = status.as_mut()
            && status.paused_entities != 0
        {
            status.paused_entities = 0;
        } // 切图的轻量诊断归零。
        return; // GLB 与旧场景不承担每次物理步的参与者检查。
    } // 结束非分层地图快速路径。
    if root_region.enabled
        && let Ok(player) = player.single()
        && root_region.center != player.translation.xz()
    {
        // 镜头完全不参与碰撞中心。
        root_region.center = player.translation.xz(); // 传送在本次物理步之前立即更新活动半径。
        root_region.revision = root_region.revision.wrapping_add(1); // 中心变化使区域资格缓存失效。
    } // 结束玩家中心同步。
    let dt = fixed_time
        .as_ref()
        .map_or(1.0 / 60.0, |time| time.delta_secs().max(1.0 / 60.0)); // 未安装时间资源的测试保留固定安全预测窗口。
    let mut outside_count = 0; // 复用已有必要的参与者检查计数，不单独扫描或让多调度 Local 相互覆盖。
    for (
        entity,
        transform,
        body,
        mut axes,
        mut velocity, // 地图真实边界只约束向外分量，不冻结整个实体。
        angular,
        owned,
        paused,
        cache,
        aabb,
        disabled,
        identity,
    ) in &mut bodies
    {
        // 工作量仅与当前物理参与者相关。
        if *body == RigidBody::Static {
            continue;
        } // 静态建筑与地形不参与暂停。
        let source = sources.resolve(transform.translation, identity, cache.as_ref().and_then(|cache| cache.source));
        let player_position = player.single().ok().map(|player| player.translation);
        let position = source.frame.to_local(transform.translation);
        let world_footprint = aabb
            .map_or(Vec2::splat(0.05), |bounds| bounds.size().xz().abs() * 0.5)
            .max(Vec2::splat(0.05));
        let footprint = source.frame.world_to_source.transform_vector3(Vec3::X * world_footprint.x).xz().abs()
            + source.frame.world_to_source.transform_vector3(Vec3::Z * world_footprint.y).xz().abs();
        let raw_velocity = velocity.as_ref().map_or(Vec3::ZERO, |value| value.0);
        let raw_predicted = if owned.is_some() && raw_velocity == Vec3::ZERO {
            cache.as_ref().filter(|cache| cache.position == position && cache.source == source.instance)
                .map_or(transform.translation, |cache| source.frame.to_world(cache.predicted))
        } else { transform.translation + raw_velocity * dt };
        let near_edge = sources.has_sources() && {
            let region = sources.region(source, &mut root_region, player_position);
            footprint_crosses_map_edge(region, position, footprint)
                || footprint_crosses_map_edge(region, source.frame.to_local(raw_predicted), footprint)
        };
        let transfer = near_edge && sources.transfer_supported(source, transform.translation, raw_predicted,
            world_footprint, &mut root_region, player_position);
        let region = sources.region(source, &mut root_region, player_position);
        if disabled && owned.is_none() {
            // 外部禁用刚体由原系统管理，不冒领恢复所有权。
            if !region.enabled && cache.is_some() {
                commands.entity(entity).remove::<TerrainSupportCache>();
            } // 退出模式仍清理自己的纯缓存，防止非 Clipmap 常态扫描。
            continue; // 从不移除外部 RigidBodyDisabled。
        } // 结束外部所有权门。
        if !region.enabled && owned.is_none() && cache.is_none() {
            continue;
        } // 旧模式没有组件和配置副作用。
        if !transfer && let Some(velocity) = velocity.as_mut() {
            // 必须先于缓存快路径执行，连续输入不能绕过地图边界。
            let local_velocity = source.frame.world_to_source.transform_vector3(velocity.0);
            velocity.0 = source.frame.source_to_world.transform_vector3(constrain_map_velocity(&region, position, local_velocity, dt)); // 沿边、向内和垂直运动保持原值。
        } // 不移动实体位置，不补建地图外碰撞。
        let step_velocity = velocity.as_ref().map_or(Vec3::ZERO, |value| source.frame.world_to_source.transform_vector3(value.0)); // 最迟物理前读取 Tnua 已写入的实际下一步速度。
        let predicted = if owned.is_some() && step_velocity == Vec3::ZERO {
            cache
                .as_ref()
                .filter(|cache| cache.position == position)
                .map_or(position, |cache| cache.predicted)
        } else {
            position + step_velocity * dt
        }; // 冻结后不因自己清零速度而立即误判可恢复，继续等待此前所需承托。
        let predicted = if transfer { source.frame.to_local(raw_predicted) }
            else { constrain_map_prediction(&region, position, predicted) }; // 真实相邻地图承托允许跨边；外侧缺页仍沿用原地图保护。
        if region.enabled
            && let Some(cache) = cache.as_ref()
            && cache.source == source.instance && cache.frame == source.frame
            && cache.revision == region.revision
            && cache.position == position
            && cache.predicted == predicted
            && cache.footprint == footprint
            && !((owned.is_some() || near_edge)
                && (footprint_crosses_map_edge(&region, position, footprint)
                    || footprint_crosses_map_edge(&region, predicted, footprint)))
        // 旧版即使缓存零预测，也必须复核地图边缘足迹才能解除遗留等待。
        {
            outside_count += usize::from(paused == Some(&TerrainActivityPaused::OutsideRange));
            continue;
        } // 重复调度不会反复检查相同区域/运动输入。
        let cached_block = cache.as_ref().and_then(|value| value.block); // 命中缓存每次都对当前边界复验。
        let (mut reason, block) = activity_reason(&region, position, cached_block); // 缓存失效最多扫描 512 个已采用块。
        if region.enabled
            && reason.is_none()
            && !transfer
            && (!footprint_supported(&region, position, footprint, block)
                || !footprint_supported(&region, predicted, footprint, block))
        {
            reason = Some(TerrainActivityPaused::AwaitingSupport);
        } // 角色边缘或下一步缺地面时先停再等。
        let next_cache = TerrainSupportCache {
            source: source.instance, frame: source.frame,
            block,
            revision: region.revision,
            position,
            predicted,
            footprint,
        }; // 记录这次检查输入。
        if let Some(mut cache) = cache {
            *cache = next_cache;
        } else if region.enabled {
            commands.entity(entity).insert(next_cache);
        } // 原位缓存，不反复结构性修改。
        if let Some(reason) = reason {
            // 范围外与等地面均保留原实体。
            if owned.is_none() {
                commands.entity(entity).insert(TerrainActivityLock {
                    previous_axes: axes.as_ref().map_or(LockedAxes::default(), |axes| **axes),
                    inserted_axes: axes.is_none(),
                });
            } // 首次记录原轴锁及组件是否原本存在。
            if paused != Some(&reason) {
                commands.entity(entity).insert(reason);
            } // 原因变更不覆盖首次所有权。
            if let Some(axes) = axes.as_mut() {
                **axes = lock_translation(**axes);
            } else {
                commands
                    .entity(entity)
                    .insert(LockedAxes::TRANSLATION_LOCKED);
            } // 缺省轴锁也受保护，但恢复时会归还缺省状态。
            if let Some(mut velocity) = velocity {
                velocity.0 = Vec3::ZERO;
            } // 清除会在下一物理步继续推动的线速度。
            if let Some(mut angular) = angular {
                angular.0 = Vec3::ZERO;
            } // 暂停期间也不保留旋转冲量。
        } else if let Some(owned) = owned {
            // 已有当前版本地面，或已退出新模式。
            if let Some(axes) = axes.as_mut() {
                // 外部移除轴锁时不重新强加旧配置。
                let restored = restore_translation(**axes, owned.previous_axes); // 只归还自己的平移位。
                if owned.inserted_axes && restored.to_bits() == LockedAxes::default().to_bits() {
                    commands.entity(entity).remove::<LockedAxes>();
                } else {
                    **axes = restored;
                } // 新增的外部旋转锁必须保留。
            } // 结束可选轴锁恢复。
            commands
                .entity(entity)
                .remove::<(TerrainActivityLock, TerrainActivityPaused)>(); // 不销毁实体或修改任何库存、战斗身份。
        } // 结束该实体状态转换。
        if !region.enabled {
            commands.entity(entity).remove::<TerrainSupportCache>();
        } // 旧模式恢复后清理缓存。
        outside_count += usize::from(reason == Some(TerrainActivityPaused::OutsideRange)); // 只计真正范围外，不把首次承托等待算进远场。
    } // 结束当前物理参与者检查。
    if let Some(status) = status.as_mut()
        && status.paused_entities != outside_count
    {
        status.paused_entities = outside_count;
    } // 无变化不争写作者界面诊断；切换后归零。
} // 结束活动门。

pub(in crate::maps::dynamic_map) fn freeze_terrain_activity(
    // 物理步前最后执行，防止 Tnua 或外部写速度绕过暂停。
    mut bodies: Query<
        (
            &mut LockedAxes,
            Option<&mut LinearVelocity>,
            Option<&mut AngularVelocity>,
        ),
        With<TerrainActivityPaused>,
    >, // 只访问实际暂停者。
) {
    // 无碰撞形状构建或全场层级遍历。
    for (mut axes, velocity, angular) in &mut bodies {
        // 遍历暂停子集。
        *axes = lock_translation(*axes); // 不反复切换 RigidBodyDisabled。
        if let Some(mut velocity) = velocity {
            velocity.0 = Vec3::ZERO;
        } // 消除本步驱动。
        if let Some(mut angular) = angular {
            angular.0 = Vec3::ZERO;
        } // 消除本步转动。
    } // 结束冻结子集。
} // 结束物理前冻结。

#[cfg(test)] // 无窗口活动规则与 ECS 生命周期回归按职责拆分。
mod tests; // 测试位于 activity/tests/，不与帧系统混排。
