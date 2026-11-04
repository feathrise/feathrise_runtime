//! 叶片窗口到通用植被层的绑定组件。

use bevy::prelude::Component; // 绑定与已采用版本都保存在主世界叶片窗口实体上。

use crate::VegetationLayerId; // 内容无关层ID是布局核心与叶片后端之间的唯一身份。

/// 将一个GPU叶片窗口绑定到内容无关的无限布局层。
///
/// 未挂该组件的既有窗口继续直接使用自己的密度图；挂载后只在候选GPU完整Mip通过
/// 逐字节回读时替换密度图，处理中或失败时持续使用上一有效版本。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq)]
pub struct VegetationBladeLayer(pub VegetationLayerId); // 消费项目自行决定该层表示普通草、作物或其他叶片内容。

/// 窗口已经采用的活动布局版本。
#[derive(Clone, Copy, Debug, Component, PartialEq, Eq)]
pub(super) struct VegetationBladeLayoutRevision {
    pub(super) revision: u64, // 私有版本避免稳定布局每帧重复修改图片资产。
    pub(super) layer: VegetationLayerId, // 同版本切换绑定层时也必须重新采用正确共享图。
}
