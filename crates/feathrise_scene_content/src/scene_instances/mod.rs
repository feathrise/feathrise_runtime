//! 完整子场景的引用协议、稳定实例身份与后台展开；源文件始终由原 SceneId 拥有。

mod graph; // 引用图展开为实例树，允许重复来源并拒绝来源链循环。
mod authoring_io; // 已有保存/删除IO事务共用来源链验证和引用保护。
mod resources; // 实例图发布值与父图草稿覆盖。
mod systems; // 有界快照捕获和后台单飞引用展开。
mod types; // 作者引用与仅平移、绕Y轴旋转的受检变换。
mod validation; // 加载、编辑和保存共用局部引用校验。

pub use graph::{ExpandedSceneInstance, SceneInstanceSource, expand_scene_instances};
pub use authoring_io::{validate_saved_scene_instances, saved_scene_referrers};
pub use resources::{SceneInstanceAuthorPreview, SceneInstanceGraph, SceneInstanceGraphPhase, SceneInstanceRootRequest};

/// 来源树原子发布后才允许运行时准备对应实例。
#[derive(bevy::prelude::SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneInstanceGraphPublishSet;
pub use types::{SceneInstanceId, SceneInstancePath, SceneInstancePlacement, SceneInstanceTransform};
pub use validation::{MAX_SCENE_INSTANCE_DEPTH, MAX_SCENE_INSTANCES, MAX_EXPANDED_SCENE_INSTANCES, validate_scene_instances};

#[cfg(test)]
mod tests;

pub(crate) fn install(app: &mut bevy::prelude::App) {
    use bevy::prelude::*;
    app.init_resource::<SceneInstanceRootRequest>()
        .init_resource::<SceneInstanceAuthorPreview>()
        .init_resource::<SceneInstanceGraph>()
        .init_resource::<resources::SceneInstanceGraphWork>()
        .add_systems(Update, systems::drive_instance_graph.in_set(SceneInstanceGraphPublishSet).after(crate::systems::apply_scene_catalog));
}

pub(crate) fn no_instances(instances: &std::sync::Arc<[SceneInstancePlacement]>) -> bool {
    instances.is_empty()
}
