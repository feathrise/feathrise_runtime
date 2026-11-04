//! 地面吸附 ECS 系统：在地形碰撞就绪后为待处理实体执行一次向下射线。

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter}; // 使用 Avian 实时物理世界查询真实地表碰撞。
use bevy::ecs::system::SystemParam; // 把两阶段冲突查询封装为单一地面吸附参数。
use bevy::prelude::*; // 引入 ECS 查询、Transform、Entity 和日志宏。

use crate::{GroundSnap, GroundSnapFilter, GroundSnapSource}; // 显式请求及应用发布的掩码/来源，不读取角色身份。
use feathrise_scene_core::residency::TerrainCollisionRegion;

/// 第二阶段读取名称并原位写入解算高度与贴地状态的稳定查询数据。
type PendingGroundSnapData = (
    Entity,
    Option<&'static Name>,
    Option<&'static GroundSnapFilter>,
    &'static mut Transform,
    &'static mut GroundSnap,
);

/// ParamSet 内部的两阶段查询契约；别名避免系统参数暴露嵌套泛型细节。
type GroundSnapParamQueries<'w, 's> = (
    Query<'w, 's, (Entity, &'static GroundSnap)>,
    Query<'w, 's, PendingGroundSnapData>,
);

/// 地面吸附系统独占的两阶段查询参数。
#[derive(SystemParam)]
pub(crate) struct GroundSnapQueries<'w, 's> {
    set: ParamSet<'w, 's, GroundSnapParamQueries<'w, 's>>, // ParamSet串行共享/可变GroundSnap访问。
}

/// 只查询所有权和禁用状态，不借用变换；允许水花系统同时更新自己的GlobalTransform。
#[derive(SystemParam)]
pub struct ActiveGroundColliders<'w, 's> {
    owners: Query<'w, 's, &'static feathrise_scene_core::residency::MapInstanceOwner>,
    parents: Query<'w, 's, &'static ChildOf>,
    disabled: Query<'w, 's, (), With<avian3d::prelude::RigidBodyDisabled>>,
}

impl ActiveGroundColliders<'_, '_> {
    pub fn is_active(&self, entity: Entity) -> bool { self.owner(entity).is_some() }
    /// 外层None表示禁用/异常层级，内层None表示父地图；不得把两者混为同一来源。
    pub fn owner(&self, entity: Entity) -> Option<Option<Entity>> {
        let mut cursor = entity;
        let mut owner = None;
        for _ in 0..256 {
            if self.disabled.contains(cursor) { return None; }
            if owner.is_none() { owner = self.owners.get(cursor).ok().map(|owner| owner.0); }
            let Ok(parent) = self.parents.get(cursor) else { return Some(owner); };
            cursor = parent.parent();
        }
        None
    }
}

/// 先确定真实命中的来源，再验证覆盖；父图的边界不能拒绝位于外侧的子地图。
#[derive(SystemParam)]
pub struct GroundSurfaceFilter<'w, 's> {
    terrain_owners: Query<'w, 's, &'static feathrise_scene_core::residency::MapTerrainOwner>,
    active: ActiveGroundColliders<'w, 's>,
    frames: Query<'w, 's, &'static GlobalTransform>,
    regions: Option<Res<'w, feathrise_scene_core::residency::InstanceResourceTable<TerrainCollisionRegion>>>,
}

impl GroundSurfaceFilter<'_, '_> {
    pub fn is_active(&self, entity: Entity) -> bool { self.collider_source(entity).is_some() }
    /// 全局玩法查询按实际命中取得源框架；隐藏候选或缺少框架时没有兜底父图。
    pub fn source(&self, entity: Entity) -> Option<(Option<Entity>, feathrise_scene_core::residency::MapSourceFrame)> {
        let owner = self.collider_source(entity)?;
        let frame = if let Some(owner) = owner {
            feathrise_scene_core::residency::MapSourceFrame::from_transform(self.frames.get(owner).ok()?.compute_transform())
        } else { default() };
        Some((owner, frame))
    }
    fn parent_frame(&self, entity: Entity) -> Mat4 {
        self.active.parents.get(entity).ok().and_then(|parent| self.frames.get(parent.parent()).ok())
            .map_or(Mat4::IDENTITY, GlobalTransform::to_matrix)
    }
    fn collider_source(&self, entity: Entity) -> Option<Option<Entity>> {
        self.active.owner(entity)
    }

    fn supports(&self, subject: Entity, hit: Entity, position: Vec3,
        source: GroundSnapSource,
        root: Option<&TerrainCollisionRegion>) -> bool {
        let Some(owner) = self.collider_source(hit) else { return false; };
        if let Some(expected) = self.collider_source(subject).flatten() {
            if owner != Some(expected) && !owner.is_some_and(|owner| self.terrain_owners.get(owner)
                .is_ok_and(|terrain| terrain.map_instance == Some(expected))) { return false; }
        }
        match source {
            GroundSnapSource::Any => {},
            GroundSnapSource::Unavailable => return false,
            GroundSnapSource::Instance(expected) => {
                // 显式实例的承托未就绪时继续等待，不能落到父图或另一份相同来源。
                if owner != Some(expected) && !owner.is_some_and(|owner|
                    self.terrain_owners.get(owner).is_ok_and(|terrain| terrain.map_instance == Some(expected))) { return false; }
            },
        }
        if let Some(owner) = owner {
            let Some(region) = self.regions.as_ref().and_then(|table| table.values.get(&owner)) else { return false; };
            let Ok(frame) = self.frames.get(owner) else { return false; };
            return region.supports(frame.affine().inverse().transform_point3(position));
        }
        root.is_none_or(|region| region.supports(position))
    }
}

/// 把所有待吸附实体移动到当前 X/Z 下方的最近物理地表。
pub(crate) fn snap_pending_entities_to_ground(
    // 系统保留实体 X/Z 和旋转，仅修正世界 Y 坐标。
    spatial_query: SpatialQuery, // Avian 空间查询会读取已完成构建的地形 Collider。
    mut queries: GroundSnapQueries, // 先收集排除实体，再可变更新待吸附实体。
    mut query_filter: Local<SpatialQueryFilter>, // 复用排除集合容量，避免地形尚未就绪时持续申请内存。
    terrain_region: Option<Res<TerrainCollisionRegion>>, // 只消费实际采用的活动碰撞覆盖；角色和场景资格由上层负责。
    sources: GroundSurfaceFilter,
) {
    // 开始处理当前帧所有仍在等待地面的实体。
    if !queries
        .set
        .p0()
        .iter()
        .any(|(_, ground_snap)| ground_snap.is_pending())
    {
        // 常态帧通常没有待吸附实体，只做一次无分配布尔扫描便结束。
        return;
    } // 结束待处理快速路径。
    query_filter.excluded_entities.clear(); // 保留上一帧哈希表容量供碰撞尚未就绪时重试。
    query_filter
        .excluded_entities
        .extend(queries.set.p0().iter().map(|(entity, _)| entity)); // 射线只命中地形、建筑等非贴地碰撞体。
    let mut pending_entities = queries.set.p1(); // 第一阶段快照已释放，现在可以安全可变遍历组件。
    for (entity, name, filter, mut transform, mut ground_snap) in &mut pending_entities {
        // 每个实体独立解算，单个未命中不会阻塞其他实体。
        if !ground_snap.is_pending() {
            // 已成功吸附的实体保留组件供后续重新请求，但不重复做射线。
            continue; // 跳过已解算实体，把常态开销降为一次布尔判断。
        } // 结束待解算筛选。
        if !sources.is_active(entity) { continue; }
        let filter = filter.copied().unwrap_or_default();
        if filter.source == GroundSnapSource::Unavailable { continue; } // 显式未就绪来源不能被最近的其它地面兜底。
        query_filter.mask = filter.mask; // 不带应用过滤输入的通用实体保持全层查询。
        let parent_frame = sources.parent_frame(entity);
        let world_position = parent_frame.transform_point3(transform.translation); // 子地图传送点等根仍保存源局部Transform。
        let (ray_start_y, ray_distance) = ground_snap.ray_window(world_position.y); // 新地图角色保留承托楼层，旧入口保持原射线范围。
        let ray_origin = Vec3::new(
            // 射线起点完全不依赖实体原始 Y，因此地下出生也能恢复。
            world_position.x, // 保留设计或随机生成的水平 X 坐标。
            ray_start_y,             // 按当前实体声明的承托语义选择高度。
            world_position.z, // 保留设计或随机生成的水平 Z 坐标。
        ); // 完成当前实体垂直射线起点。
        let Some(hit) = spatial_query.cast_ray_predicate(
            // 查找当前 X/Z 下方的第一个物理表面。
            ray_origin,    // 使用足够高的统一起点。
            Dir3::NEG_Y,   // 沿世界垂直向下的编译期单位方向。
            ray_distance,  // 局部锚点不得继续穿到其他楼层。
            true,          // 如果起点意外在实心碰撞内，仍把它视为有效命中。
            &query_filter, // 排除玩家、怪物、NPC、掉落物等其他贴地实体。
            &|entity| sources.collider_source(entity).is_some(),
        ) else {
            // 当前 X/Z 下方暂时没有可用物理地表。
            if ground_snap.should_report_miss() {
                // 同一次请求只输出一条警告，后续帧会继续静默重试。
                let display_name = name.map_or("<未命名实体>", Name::as_str); // 尽可能提供世界检查器可查找的实体名称。
                warn!(
                    "地面吸附未命中：{display_name} ({entity:?})，X/Z=({:.2}, {:.2})",
                    transform.translation.x, transform.translation.z
                ); // 输出实体与水平坐标供排查地图缺口。
                ground_snap.mark_miss_reported(); // 标记本次请求已报告，防止日志每帧刷屏。
            } // 结束首次未命中诊断。
            continue; // 保留 pending=true，地形碰撞稍后可用时自动再试。
        }; // 结束物理射线命中解构。
        if !sources.supports(entity, hit.entity, world_position, filter.source, terrain_region.as_deref()) {
            continue; // 最近表面未获得本来源的正式覆盖，保留请求而不穿透到下方父图。
        }
        let ground_y = ray_origin.y - hit.distance; // 由射线起点和命中距离还原真实地表世界 Y。
        let snapped_y = ground_y + ground_snap.ground_offset(); // 在地表上叠加实体类型专属的胶囊、中心或悬浮偏移。
        transform.translation = parent_frame.inverse().transform_point3(Vec3::new(world_position.x, snapped_y, world_position.z)); // 世界贴地结果写回父层局部坐标，不能二次乘实例变换。
        ground_snap.resolve(snapped_y); // 保存结果并停止常态重复射线。
    } // 结束当前帧待吸附实体遍历。
} // 结束统一地面吸附系统。

#[cfg(test)]
mod tests;
#[cfg(test)]
mod neutral_tests;
