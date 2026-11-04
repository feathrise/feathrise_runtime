//! 作者局部遮雨盒；允许直接插入无Mesh、无Collider的场景节点。

use bevy::prelude::*; // ECS组件与局部向量。

#[derive(Component, Clone, Copy, Debug, PartialEq)] // 变更检测覆盖作者修改及热重载。
pub struct RainShelterBox {
    // 盒子应贴合屋顶/桥面，不能把庭院包进整楼包围盒。
    pub center: Vec3,       // Bevy/glTF节点局部中心。
    pub half_extents: Vec3, // 严格正值的局部三轴半径。
} // 完成作者小数据声明。
