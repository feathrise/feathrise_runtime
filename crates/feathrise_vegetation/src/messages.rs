//! 种植布局的外部变更请求与近场代理差分消息。

use bevy::prelude::{IVec2, Message, Transform}; // 消息携带脏块范围、稳定实例和最终变换。

use crate::{VegetationLayerId, VegetationPrototypeId}; // 层和原型身份保持与渲染后端解耦。

/// 请求重新解析一片闭区间脏块；布局处理中继续使用上一有效版本。
#[derive(Clone, Copy, Debug, Message, PartialEq, Eq)]
pub struct MarkVegetationDirty {
    pub min_tile: IVec2, // 受影响的最小64×64布局块坐标。
    pub max_tile: IVec2, // 受影响的最大64×64布局块坐标。
}

/// 一次新布局因非法配置或合成失败被原子拒绝。
#[derive(Clone, Debug, Message, PartialEq, Eq)]
pub struct VegetationLayoutRejected {
    pub reason: String, // 完整错误链供日志、F3或严格烟雾展示。
}

/// 一个稳定候选进入近场代理内圈。
#[derive(Clone, Debug, Message, PartialEq)]
pub struct VegetationNearFieldEntered {
    pub layer: VegetationLayerId,         // 消费者据此选择碰撞或玩法适配器。
    pub prototype: VegetationPrototypeId, // 解析后的内容原型无需类别枚举。
    pub instance_id: u64,                 // 与GPU候选共享的世界稳定实例身份。
    pub transform: Transform,             // 代理与远场GPU实例使用完全相同的变换。
}

/// 一个稳定候选越过近场代理外圈或所属层被移除。
#[derive(Clone, Copy, Debug, Message, PartialEq, Eq)]
pub struct VegetationNearFieldExited {
    pub layer: VegetationLayerId, // 允许项目只回收当前层的适配实体。
    pub instance_id: u64,         // 稳定ID防止不同层相同候选坐标互相误删。
}
