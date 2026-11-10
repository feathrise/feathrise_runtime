//! 同一父世界内的独立场景上下文；选择只切换借用地址，不搬运或覆盖资源槽。

mod params; // 正式域系统按当前实例借用自身状态，原父图仍借用既有资源。
mod registry; // 固定插件类型集合的上下文创建与所有权退役。
mod resources; // 上下文地址、源坐标变换和按实体索引的资源容器。
mod components; // 迟到实体事件携带自己的上下文归属。
mod world_access; // 独占清理同样按当前上下文借用。
mod query; // 场景自己的实体不进入另一个实例的域查询。
mod ownership; // 生成根与迟到GLB层级继承稳定上下文。
mod source_transform; // 地形捕获只包含源图内部变换。
mod frame; // 全局观察位置进入独立源坐标。
pub(crate) mod asset_journal; // 固定容量的轮转调度通知。

pub use params::{InstanceRes, InstanceResMut};
pub use registry::{SceneInstanceContextAppExt, initialize_instance_context, retire_instance_context};
pub use resources::{InstanceResourceTable, SceneRuntimeScope, SceneSourceFrame};
pub use components::{SceneInstanceOwner, SceneInstanceDetached, SceneDetachedRoots, SceneTerrainOwner};
pub use world_access::{instance_resource, instance_resource_mut};
pub use query::InstanceQuery;
pub use ownership::EntitySceneScope;
pub use source_transform::EntitySourceTransform;
pub use frame::InstanceFrame;
pub use asset_journal::{InstanceAssetJournal, InstanceAssetJournalEpoch, install_instance_asset_journal};

#[cfg(test)]
mod tests;
