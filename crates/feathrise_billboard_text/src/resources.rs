// resources.rs — Billboard 固定离屏烘焙相机池与等待队列资源。

use std::collections::VecDeque; // FIFO队列保证同优先级的名字按请求顺序烘焙。

use bevy::prelude::*; // Resource、Entity、Handle和Image类型。

use crate::types::BillboardBakeSlot; // 非ECS槽位数据独立放在types.rs。

/// 固定离屏相机池：Billboard数量只增长队列，不再增长Camera2d数量。
#[derive(Resource, Default)] // 插件初始化空池，Startup系统再创建固定相机实体。
pub struct BillboardBakePool {
    pub(crate) waiting: VecDeque<Entity>, // 尚未获得相机槽位的Billboard实体队列。
    pub(crate) slots: Vec<BillboardBakeSlot>, // 固定数量的可复用相机槽位。
    pub(crate) placeholder_image: Handle<Image>, // 空闲相机指向的1×1占位纹理，避免引用已释放目标。
}
