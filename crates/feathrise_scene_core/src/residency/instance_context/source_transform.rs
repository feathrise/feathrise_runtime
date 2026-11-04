//! 从源图内部层级计算地形输入变换，整体移动子图不会被误判为修改地形顶点。

use bevy::{ecs::system::SystemParam, math::Affine3A, prelude::*};
use super::MapRuntimeScope;

#[derive(SystemParam)]
pub struct EntitySourceTransform<'w, 's> {
    transforms: Query<'w, 's, &'static Transform>,
    parents: Query<'w, 's, &'static ChildOf>,
    scope: Option<Res<'w, MapRuntimeScope>>,
}

impl EntitySourceTransform<'_, '_> {
    pub fn affine(&self, entity: Entity, global: &GlobalTransform) -> Affine3A {
        let Some(instance) = self.scope.as_deref().and_then(|scope| scope.instance) else { return global.affine(); };
        let mut cursor = entity;
        let mut local = Affine3A::IDENTITY;
        for _ in 0..256 {
            if cursor == instance { return local; }
            let Ok(transform) = self.transforms.get(cursor) else { break; };
            local = transform.compute_affine() * local;
            let Ok(parent) = self.parents.get(cursor) else { break; };
            cursor = parent.parent();
        }
        // 独立预热探针不属于地形作者源；调用方保留其完整世界姿态。
        global.affine()
    }
}
