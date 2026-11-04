//! 分层碰撞活动门独占的平移锁与支持块缓存，不接管外部刚体禁用状态。
use avian3d::prelude::LockedAxes; // 保留进入暂停之前的平移约束。
use bevy::prelude::*; // ECS 组件宏与索引类型。

#[derive(Component)] // 仅暂停期间存在，恢复时删除。
pub(in crate::maps::dynamic_map) struct TerrainActivityLock {
    // 父域注册系统需可见，外部 crate 仍不能接管所有权。
    pub previous_axes: LockedAxes, // 恢复时只归还本系统添加的平移位。
    pub inserted_axes: bool,       // 无 LockedAxes 的通用刚体恢复后归还原来的组件缺省状态。
} // 结束锁所有权。

#[derive(Component, Default)] // 常态保留上次命中的块下标，避免每帧线性扫 512 块。
pub(in crate::maps::dynamic_map) struct TerrainSupportCache {
    // 父域注册系统需可见，每次仍检查发布边界。
    pub source: Option<Entity>, // 缓存不能跨不同地图实例复用。
    pub frame: feathrise_scene_core::residency::MapSourceFrame, // 整体移动/旋转使承托查询失效。
    pub block: Option<usize>, // 无命中时只在位置或发布版本变化后重试。
    pub revision: u64,        // 上次实际检查的完整区域版本。
    pub position: Vec3,       // 不移动且区域不变时不重复检查。
    pub predicted: Vec3,      // 电机速度改变时复核下一物理步落点。
    pub footprint: Vec2,      // 角色/掉落物当前包围盒半宽变化也使缓存失效。
} // 结束轻量承托缓存。
